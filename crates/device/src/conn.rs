//! usbmuxd access with TCP_NODELAY, and the CoreDeviceProxy tunnel.
//!
//! idevice's own `UsbmuxdAddr::to_socket` connects to 127.0.0.1:27015 without
//! TCP_NODELAY. Every byte we exchange with the phone (video in, HID out) rides
//! that one loopback socket, so small HID writes must not wait on Nagle.

use std::{future::Future, pin::Pin, time::Instant};

use anyhow::{Context, Result, anyhow, bail};
use idevice::{
    Idevice, IdeviceError, IdeviceService,
    core_device_proxy::CoreDeviceProxy,
    pairing_file::PairingFile,
    provider::IdeviceProvider,
    rsd::RsdHandshake,
    tcp::handle::AdapterHandle,
    usbmuxd::{Connection, UsbmuxdConnection, UsbmuxdDevice},
};

pub const USBMUXD_ADDR: &str = "127.0.0.1:27015";
const LABEL: &str = "iphone-mirror";

pub async fn usbmuxd() -> Result<UsbmuxdConnection, IdeviceError> {
    let socket = tokio::net::TcpStream::connect(USBMUXD_ADDR).await?;
    // MIRROR_NAGLE=1 leaves Nagle on, only to measure what NODELAY buys.
    socket.set_nodelay(std::env::var_os("MIRROR_NAGLE").is_none())?;
    Ok(UsbmuxdConnection::new(Box::new(socket), 0))
}

/// Said when more than one iPhone is plugged in and none was chosen.
pub const SEVERAL_ON_USB: &str = "iPhones are plugged in; choose one";

/// Every USB-attached device.
pub async fn usb_devices() -> Result<Vec<UsbmuxdDevice>> {
    let mut mux = usbmuxd()
        .await
        .context("cannot reach Apple Mobile Device Service on 127.0.0.1:27015")?;
    Ok(mux.get_devices().await?.into_iter().filter(|d| d.connection_type == Connection::Usb).collect())
}

/// The USB-attached device to use: the chosen one (`target`), or the only
/// one plugged in.
pub async fn usb_device_for(target: Option<&str>) -> Result<UsbmuxdDevice> {
    let usb = usb_devices().await?;
    match target {
        Some(udid) => usb
            .into_iter()
            .find(|d| d.udid == udid)
            .ok_or_else(|| anyhow!("no iPhone on USB: the chosen iPhone is not plugged in")),
        None => match usb.len() {
            0 => bail!("no iPhone on USB: plug it in, unlock it and tap Trust"),
            1 => Ok(usb.into_iter().next().unwrap()),
            n => bail!("{n} {SEVERAL_ON_USB}"),
        },
    }
}

/// The single USB-attached device, or an error explaining what is wrong.
pub async fn usb_device() -> Result<UsbmuxdDevice> {
    usb_device_for(None).await
}

/// `IdeviceProvider` over usbmuxd whose sockets all have TCP_NODELAY set.
#[derive(Debug, Clone)]
pub struct NoDelayUsbmux {
    pub udid: String,
    pub device_id: u32,
}

impl NoDelayUsbmux {
    pub fn new(dev: &UsbmuxdDevice) -> Self {
        Self {
            udid: dev.udid.clone(),
            device_id: dev.device_id,
        }
    }
}

impl IdeviceProvider for NoDelayUsbmux {
    fn connect(
        &self,
        port: u16,
    ) -> Pin<Box<dyn Future<Output = Result<Idevice, IdeviceError>> + Send>> {
        let device_id = self.device_id;
        Box::pin(async move {
            usbmuxd()
                .await?
                .connect_to_device(device_id, port, LABEL)
                .await
        })
    }

    fn label(&self) -> &str {
        LABEL
    }

    /// Mirrorly's own record for the device if it keeps one (made when
    /// Apple's service would not store a re-pairing), else Apple's.
    fn get_pairing_file(
        &self,
    ) -> Pin<Box<dyn Future<Output = Result<PairingFile, IdeviceError>> + Send>> {
        let udid = self.udid.clone();
        Box::pin(async move {
            if let Some(p) = crate::pairing::own_record(&udid) {
                return Ok(p);
            }
            usbmuxd().await?.get_pair_record(&udid).await
        })
    }
}

/// An open CoreDeviceProxy tunnel with the userspace TCP/UDP stack running and
/// the RSD service list fetched.
pub struct Tunnel {
    pub adapter: AdapterHandle,
    pub handshake: RsdHandshake,
    pub host_ip: String,
    pub device_ip: String,
    pub mtu: u16,
    /// (stage, milliseconds) for the setup sequence.
    pub timings: Vec<(&'static str, f64)>,
}

pub async fn open_tunnel(provider: &NoDelayUsbmux) -> Result<Tunnel> {
    let mut timings = Vec::new();
    let t = Instant::now();
    let proxy = CoreDeviceProxy::connect(provider)
        .await
        .context("CoreDeviceProxy failed (needs Trust, Developer Mode, unlocked)")?;
    timings.push(("lockdown + CoreDeviceProxy handshake", ms(t)));
    let info = proxy.tunnel_info().clone();

    let t = Instant::now();
    let mut adapter = proxy.create_software_tunnel()?.to_async_handle();
    let rsd = adapter
        .connect(info.server_rsd_port)
        .await
        .context("TCP connect to RSD over the tunnel")?;
    timings.push(("userspace TCP connect to RSD", ms(t)));

    let t = Instant::now();
    let handshake = RsdHandshake::new(rsd).await.context("RSD handshake")?;
    timings.push(("RSD handshake", ms(t)));

    Ok(Tunnel {
        host_ip: adapter.host_ip().to_string(),
        device_ip: adapter.peer_ip().to_string(),
        adapter,
        handshake,
        mtu: info.mtu,
        timings,
    })
}

pub fn ms(since: Instant) -> f64 {
    since.elapsed().as_secs_f64() * 1000.0
}
