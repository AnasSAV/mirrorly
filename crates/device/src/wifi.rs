//! Wi-Fi transport: a RemotePairing record created once over USB, Bonjour
//! discovery of `_remotepairing._tcp`, and a TLS-PSK CDTunnel over TCP that
//! ends in the same userspace stack (and the same [`Tunnel`]) as USB.
//!
//! Flow (idevice `remote_pairing`, same as pymobiledevice3 / Pane):
//! 1. USB, once: `com.apple.dt.remotepairingdeviced.lockdown` pair-setup. The
//!    USB link is already trusted, so iOS shows no prompt. The record holds our
//!    keys and the device's `altIRK`.
//! 2. Wi-Fi: browse `_remotepairing._tcp.local.`; an answer is ours only if its
//!    `authTag` = SipHash(altIRK, identifier). Nothing is sent to other devices.
//! 3. TCP to the advertised port, pair-*verify* (never pair-setup: a stale
//!    record must fail, not raise a PIN prompt), `createListener`, TLS-PSK to
//!    the listener port, CDTunnel handshake, userspace TCP/UDP, RSD.
//!
//! Limitation: the tunnel is TCP, so the video's UDP rides inside TCP. Wi-Fi
//! loss becomes retransmission delay (head-of-line blocking) rather than a
//! dropped frame. Apple's QUIC tunnel avoids that; idevice 0.1.68 has no QUIC.

use std::{
    net::{IpAddr, SocketAddr},
    path::PathBuf,
    time::{Duration, Instant},
};

use anyhow::{Context, Result, anyhow, bail};
use idevice::{
    IdeviceService, ReadWrite,
    provider::IdeviceProvider,
    remote_pairing::{
        PeerDevice, RemotePairingClient, RemotePairingLockdownService, RpPairingFile, RpPairingSocket,
        connect_tls_psk_tunnel_native,
    },
    rsd::RsdHandshake,
};

use crate::{conn::Tunnel, session::DeviceInfo};

const SERVICE_TYPE: &str = "_remotepairing._tcp.local.";

/// Name this PC gives itself in the pairing record.
pub fn hostname() -> String {
    let pc = std::env::var("COMPUTERNAME").unwrap_or_else(|_| "pc".into());
    format!("iphone-mirror-{pc}")
}

fn store_dir() -> PathBuf {
    let base = std::env::var_os("LOCALAPPDATA").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
    base.join("iphone-mirror").join("wifi")
}

fn record_path(udid: &str) -> PathBuf {
    store_dir().join(format!("{udid}.rppairing"))
}

fn info_path(udid: &str) -> PathBuf {
    store_dir().join(format!("{udid}.info"))
}

/// A device we can reach over Wi-Fi: its pairing record and the device
/// details last seen over USB (lockdown is not available over Wi-Fi).
pub struct Paired {
    pub record: RpPairingFile,
    pub info: DeviceInfo,
}

pub fn has_record(udid: &str) -> bool {
    record_path(udid).exists()
}

/// Every device with a stored pairing record.
pub async fn load_paired() -> Vec<Paired> {
    let Ok(dir) = std::fs::read_dir(store_dir()) else { return Vec::new() };
    let mut out = Vec::new();
    for e in dir.flatten() {
        let path = e.path();
        if path.extension().is_none_or(|x| x != "rppairing") {
            continue;
        }
        let Some(udid) = path.file_stem().and_then(|s| s.to_str()).map(str::to_string) else { continue };
        let Ok(record) = RpPairingFile::read_from_file(&path).await else { continue };
        let Some(info) = read_info(&udid) else { continue };
        out.push(Paired { record, info });
    }
    out
}

/// Remember the USB-side device details for later Wi-Fi sessions.
pub fn save_info(info: &DeviceInfo) -> Result<()> {
    std::fs::create_dir_all(store_dir())?;
    let text = format!(
        "name={}\nproduct_type={}\nios_version={}\nscreen_w={}\nscreen_h={}\n",
        info.name, info.product_type, info.ios_version, info.screen_w, info.screen_h
    );
    std::fs::write(info_path(&info.udid), text)?;
    Ok(())
}

fn read_info(udid: &str) -> Option<DeviceInfo> {
    let text = std::fs::read_to_string(info_path(udid)).ok()?;
    let get = |k: &str| text.lines().find_map(|l| l.strip_prefix(&format!("{k}="))).map(str::to_string);
    Some(DeviceInfo {
        udid: udid.to_string(),
        name: get("name")?,
        product_type: get("product_type").unwrap_or_default(),
        ios_version: get("ios_version").unwrap_or_default(),
        screen_w: get("screen_w")?.parse().ok()?,
        screen_h: get("screen_h")?.parse().ok()?,
        transport: crate::session::Transport::Wifi,
    })
}

/// One-time pairing over the trusted USB link (no prompt on the phone).
pub async fn pair_over_usb(provider: &dyn IdeviceProvider, udid: &str) -> Result<()> {
    let host = hostname();
    let service = RemotePairingLockdownService::connect(provider)
        .await
        .context("remotepairingdeviced lockdown service")?;
    let mut client = service.into_client(&host)?;
    let mut record = RpPairingFile::generate(&host);
    // Over USB the existing Trust covers consent and the device sends no PIN,
    // but SRP still needs one: idevice's own `remotepairing_lockdown` tool
    // uses "000000" for this promptless path (the code takes its first 6
    // bytes, so it must not be shorter).
    client
        .connect(&mut record, async || "000000".to_string())
        .await
        .context("RemotePairing pair-setup over USB")?;
    if record.alt_irk().is_none() {
        bail!("pairing completed without an altIRK; Bonjour matching would be impossible");
    }
    std::fs::create_dir_all(store_dir())?;
    record.write_to_file(record_path(udid)).await?;
    tracing::info!("Wi-Fi pairing record saved for {udid}");
    Ok(())
}

/// Find a paired phone on the local network. Returns its address, the
/// RemotePairing port, and which record matched.
pub fn discover(paired: &[Paired], timeout: Duration) -> Result<Option<(IpAddr, u16, usize)>> {
    let mdns = mdns_sd::ServiceDaemon::new().context("start mDNS (is UDP 5353 blocked?)")?;
    let rx = mdns.browse(SERVICE_TYPE)?;
    let deadline = Instant::now() + timeout;
    let mut found = None;
    while found.is_none() {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            break;
        }
        let Ok(ev) = rx.recv_timeout(left) else { break };
        let mdns_sd::ServiceEvent::ServiceResolved(svc) = ev else { continue };
        let (Some(identifier), Some(tag)) = (svc.get_property_val_str("identifier"), svc.get_property_val_str("authTag"))
        else {
            continue;
        };
        let Some(idx) = paired.iter().position(|p| {
            p.record.alt_irk().is_some_and(|irk| PeerDevice::validate_auth_tag(irk, identifier, tag))
        }) else {
            continue;
        };
        if let Some(ip) = svc.get_addresses_v4().into_iter().next() {
            found = Some((IpAddr::V4(ip), svc.get_port(), idx));
        }
    }
    let _ = mdns.stop_browse(SERVICE_TYPE);
    let _ = mdns.shutdown();
    Ok(found)
}

async fn tcp(addr: SocketAddr) -> Result<tokio::net::TcpStream> {
    let s = tokio::time::timeout(Duration::from_secs(5), tokio::net::TcpStream::connect(addr))
        .await
        .map_err(|_| anyhow!("timed out connecting to {addr}"))??;
    s.set_nodelay(true)?;
    Ok(s)
}

/// Open the CoreDevice tunnel over Wi-Fi.
pub async fn open_tunnel(ip: IpAddr, port: u16, record: &mut RpPairingFile) -> Result<Tunnel> {
    let mut timings = Vec::new();
    let t = Instant::now();
    let stream: Box<dyn ReadWrite> = Box::new(tcp(SocketAddr::new(ip, port)).await?);
    let mut client = RemotePairingClient::new(RpPairingSocket::new(stream), &hostname());
    client.attempt_pair_verify().await.context("RemotePairing handshake")?;
    client
        .validate_pairing(record)
        .await
        .context("pair-verify failed: the phone no longer accepts this PC's Wi-Fi pairing; plug in USB once to re-pair")?;
    timings.push(("pair-verify over Wi-Fi", ms(t)));

    let t = Instant::now();
    let listener = client.create_tcp_listener().await.context("createListener")?;
    let key = client.encryption_key().to_vec();
    let tls = tcp(SocketAddr::new(ip, listener)).await?;
    let tunnel = connect_tls_psk_tunnel_native(tls, &key).await.context("TLS-PSK tunnel")?;
    timings.push(("TLS-PSK CDTunnel handshake", ms(t)));

    let info = tunnel.info.clone();
    let our_ip: IpAddr = info.client_address.parse()?;
    let their_ip: IpAddr = info.server_address.parse()?;
    let mtu = info.mtu;
    let stream = tunnel.into_inner();
    let mut adapter = idevice::tcp::adapter::Adapter::new(Box::new(stream), our_ip, their_ip);
    adapter.set_mss((mtu as usize).saturating_sub(60));
    let mut adapter = adapter.to_async_handle();

    let t = Instant::now();
    let rsd = adapter.connect(info.server_rsd_port).await.context("TCP connect to RSD over the Wi-Fi tunnel")?;
    let handshake = RsdHandshake::new(rsd).await.context("RSD handshake")?;
    timings.push(("RSD connect + handshake", ms(t)));

    Ok(Tunnel {
        host_ip: adapter.host_ip().to_string(),
        device_ip: adapter.peer_ip().to_string(),
        adapter,
        handshake,
        mtu,
        timings,
    })
}

fn ms(since: Instant) -> f64 {
    since.elapsed().as_secs_f64() * 1000.0
}
