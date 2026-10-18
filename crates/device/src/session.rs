//! Connection lifecycle: a dedicated network thread that connects over USB,
//! streams, executes input, and reconnects when anything drops.

use std::{
    sync::{Arc, Mutex},
    time::Duration,
};

use anyhow::{Context, Result, anyhow, bail};
use idevice::{
    IdeviceService, RsdService,
    core_device::{RtpPacket, is_rtcp},
    lockdown::LockdownClient,
    provider::IdeviceProvider,
    tcp::handle::UdpSocketHandle,
};
use input::HidEvent;
use tokio::sync::{mpsc, oneshot};

use crate::{
    conn::{NoDelayUsbmux, open_tunnel, usb_device},
    ddi,
    feedback::{self, Feedback, RctlMode},
    hid::{self, HidStats, Indigo, Stamped, Uhs},
    media, wifi,
};

const DISPLAY_SERVICE: &str = "com.apple.coredevice.displayservice";
const VIDEO_PT: u8 = 100;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Transport {
    Usb,
    Wifi,
}

#[derive(Debug, Clone)]
pub struct DeviceInfo {
    pub udid: String,
    pub name: String,
    pub product_type: String,
    pub ios_version: String,
    /// Panel size in pixels (portrait), from lockdown (cached for Wi-Fi).
    pub screen_w: u32,
    pub screen_h: u32,
    pub transport: Transport,
}

#[derive(Debug, Clone)]
pub enum DeviceEvent {
    /// Human-readable progress ("Connecting…", "Installing Developer Disk Image…").
    Status(String),
    Connected(DeviceInfo),
    Disconnected(String),
}

/// Receives the video RTP stream on the network thread. Must be quick:
/// it runs inline in the receive loop.
pub trait MediaSink: Send + 'static {
    fn session_started(&mut self, info: &DeviceInfo, keyframes: KeyframeRequester);
    /// One video RTP packet. Return `true` if the stream is broken and needs
    /// a keyframe (loss, corrupt access unit).
    fn rtp(&mut self, seq: u16, ts: u32, marker: bool, payload: &[u8], rx_us: u64) -> bool;
    fn session_ended(&mut self);
}

/// Asks the current session's encoder for a keyframe; a no-op while
/// disconnected. Cheap to clone; safe from any thread.
#[derive(Clone, Default)]
pub struct KeyframeRequester(Arc<Mutex<Option<Feedback>>>);

impl KeyframeRequester {
    pub fn request(&self) {
        if let Some(fb) = self.0.lock().unwrap().as_ref() {
            fb.request_keyframe();
        }
    }
}

#[derive(Debug, Clone)]
pub struct Options {
    /// Install the Developer Disk Image automatically when it is missing.
    pub auto_install_ddi: bool,
    /// Use Wi-Fi when no USB device is attached (and create the Wi-Fi
    /// pairing during the first USB session). USB is always preferred.
    pub wifi: bool,
    pub rctl: RctlMode,
    /// Wait after the media stream starts before input is accepted (iOS's
    /// HID auth gate re-matches asynchronously; Phase 0 used 500 ms).
    pub hid_settle: Duration,
    /// Clock shared with the app, µs.
    pub now_us: fn() -> u64,
}

pub struct DeviceHandle {
    hid_tx: mpsc::UnboundedSender<Stamped>,
    pub keyframes: KeyframeRequester,
    pub hid_stats: Arc<HidStats>,
    _thread: std::thread::JoinHandle<()>,
}

impl DeviceHandle {
    /// Queue an input event; never blocks.
    pub fn send(&self, t_us: u64, ev: HidEvent) {
        let _ = self.hid_tx.send((t_us, ev));
    }

    /// A cloneable sender for other threads (e.g. a latency probe).
    pub fn sender(&self) -> InputSender {
        InputSender(self.hid_tx.clone())
    }
}

#[derive(Clone)]
pub struct InputSender(mpsc::UnboundedSender<Stamped>);

impl InputSender {
    pub fn send(&self, t_us: u64, ev: HidEvent) {
        let _ = self.0.send((t_us, ev));
    }
}

pub fn start<S: MediaSink>(
    sink: S,
    on_event: impl Fn(DeviceEvent) + Send + Sync + 'static,
    opts: Options,
) -> Result<DeviceHandle> {
    let (hid_tx, hid_rx) = mpsc::unbounded_channel();
    let keyframes = KeyframeRequester::default();
    let hid_stats = Arc::new(HidStats::default());
    let (kf, stats) = (keyframes.clone(), hid_stats.clone());
    let thread = std::thread::Builder::new().name("network".into()).spawn(move || {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name("network-rt")
            .enable_all()
            .build()
            .expect("tokio runtime");
        rt.block_on(session_loop(sink, on_event, opts, hid_rx, kf, stats));
    })?;
    Ok(DeviceHandle { hid_tx, keyframes, hid_stats, _thread: thread })
}

async fn session_loop<S: MediaSink>(
    sink: S,
    on_event: impl Fn(DeviceEvent),
    opts: Options,
    hid_rx: mpsc::UnboundedReceiver<Stamped>,
    keyframes: KeyframeRequester,
    stats: Arc<HidStats>,
) {
    let hid_rx = Arc::new(tokio::sync::Mutex::new(hid_rx));
    let mut sink = sink;
    loop {
        on_event(DeviceEvent::Status("Connecting…".into()));
        let (s, reason) = run_once(sink, &on_event, &opts, &hid_rx, &keyframes, &stats).await;
        sink = s;
        *keyframes.0.lock().unwrap() = None;
        tracing::warn!("session ended: {reason}");
        on_event(DeviceEvent::Disconnected(reason));
        // Input typed while disconnected must not replay on reconnect.
        while hid_rx.lock().await.try_recv().is_ok() {}
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
}

/// Everything a streaming session needs before video and input start.
struct Prepared {
    tunnel: crate::conn::Tunnel,
    media: media::MediaSession,
    fb: Feedback,
    info: DeviceInfo,
}

/// One connection, start to finish. The sink always comes back, with the
/// reason the session ended.
async fn run_once<S: MediaSink>(
    mut sink: S,
    on_event: &impl Fn(DeviceEvent),
    opts: &Options,
    hid_rx: &Arc<tokio::sync::Mutex<mpsc::UnboundedReceiver<Stamped>>>,
    keyframes: &KeyframeRequester,
    stats: &Arc<HidStats>,
) -> (S, String) {
    let Prepared { mut tunnel, media: m, fb, info } = match prepare(on_event, opts).await {
        Ok(Ok(p)) => p,
        Ok(Err(reason)) => return (sink, reason),
        Err(e) => return (sink, format!("{e:#}")),
    };
    *keyframes.0.lock().unwrap() = Some(fb.clone());

    // The sink moves into the receive task for the session and comes back
    // when that task ends.
    sink.session_started(&info, keyframes.clone());
    let (cancel_tx, cancel_rx) = oneshot::channel();
    let mut rx_task = tokio::spawn(rx_loop(sink, m.video.clone(), fb.clone(), opts.now_us, cancel_rx));

    let hid = connect_hid(&mut tunnel, opts.hid_settle).await;
    let (sink, reason) = match hid {
        Err(e) => {
            let _ = cancel_tx.send(());
            match rx_task.await {
                Ok((s, _)) => (s, format!("{e:#}")),
                Err(_) => panic!("receive task panicked"),
            }
        }
        Ok((uhs, indigo)) => {
            let (rx_q, stats2, now_us) = (hid_rx.clone(), stats.clone(), opts.now_us);
            // Over Wi-Fi, iOS ignores universalhidservice touch reports but
            // accepts Indigo digitizer events (measured 2026-10-18: a tap on
            // Settings did nothing via UHS, opened it via Indigo; drags work).
            // USB keeps the UHS path verified in Phases 0-1.
            let touch_path = match std::env::var("MIRROR_TOUCH").as_deref() {
                Ok("indigo") => hid::TouchPath::Indigo,
                Ok("uhs") => hid::TouchPath::Uhs,
                _ if info.transport == Transport::Wifi => hid::TouchPath::Indigo,
                _ => hid::TouchPath::Uhs,
            };
            tracing::info!("touch path: {touch_path:?}");
            let mut hid_task = tokio::spawn(async move {
                let mut rx = rx_q.lock().await;
                // Drop anything queued before input was possible.
                while rx.try_recv().is_ok() {}
                hid::run(uhs, indigo, &mut rx, now_us, stats2, touch_path).await
            });
            let on_wifi = info.transport == Transport::Wifi;
            on_event(DeviceEvent::Connected(info));
            tokio::select! {
                r = &mut rx_task => {
                    hid_task.abort();
                    r.expect("receive task panicked")
                }
                // USB is preferred: leave Wi-Fi as soon as a cable appears.
                _ = usb_appears(), if on_wifi => {
                    hid_task.abort();
                    let _ = cancel_tx.send(());
                    let (s, _) = rx_task.await.expect("receive task panicked");
                    (s, "USB connected; switching to USB".into())
                }
                r = &mut hid_task => {
                    let _ = cancel_tx.send(());
                    let (s, _) = rx_task.await.expect("receive task panicked");
                    let reason = match r {
                        Ok(Err(e)) => format!("input channel failed: {e:#}"),
                        _ => "input task ended".into(),
                    };
                    (s, reason)
                }
            }
        }
    };
    let mut sink = sink;
    sink.session_ended();

    let mut client = m.client;
    let _ = tokio::time::timeout(Duration::from_secs(1), client.stop_media_stream()).await;
    drop(tunnel);
    (sink, reason)
}

/// Resolves once a USB-attached iPhone shows up in usbmuxd.
async fn usb_appears() {
    loop {
        tokio::time::sleep(Duration::from_secs(2)).await;
        if usb_device().await.is_ok() {
            return;
        }
    }
}

async fn connect_hid(t: &mut crate::conn::Tunnel, settle: Duration) -> Result<(Uhs, Indigo)> {
    // iOS needs a moment before the HID surfaces accept injected input.
    tokio::time::sleep(settle).await;
    let uhs = Uhs::connect_rsd(&mut t.adapter, &mut t.handshake).await.context("hid.universalhidservice")?;
    let indigo = Indigo::connect_rsd(&mut t.adapter, &mut t.handshake).await.context("hid.indigo")?;
    Ok((uhs, indigo))
}

/// Connect, check prerequisites and start the media stream. `Ok(Err(..))`
/// is a planned early end (e.g. the DDI was just installed).
async fn prepare(on_event: &impl Fn(DeviceEvent), opts: &Options) -> Result<Result<Prepared, String>> {
    let (mut t, info) = match usb_device().await {
        Ok(dev) => {
            let provider = NoDelayUsbmux::new(&dev);
            let info = read_info(&provider, &dev.udid).await?;
            if opts.wifi {
                if let Err(e) = wifi::save_info(&info) {
                    tracing::warn!("could not cache device info for Wi-Fi: {e:#}");
                }
                if !wifi::has_record(&dev.udid) {
                    on_event(DeviceEvent::Status("Setting up Wi-Fi (one time)…".into()));
                    // Own task: a panic in third-party pairing code must not
                    // take the network thread (and reconnects) down with it.
                    let (p, udid) = (provider.clone(), dev.udid.clone());
                    match tokio::spawn(async move { wifi::pair_over_usb(&p, &udid).await }).await {
                        Ok(Ok(())) => {}
                        Ok(Err(e)) => tracing::warn!("Wi-Fi pairing failed (USB still works): {e:#}"),
                        Err(e) => tracing::warn!("Wi-Fi pairing crashed (USB still works): {e}"),
                    }
                }
            }
            (open_tunnel(&provider).await?, info)
        }
        Err(usb_err) if opts.wifi => {
            let mut paired = wifi::load_paired().await;
            if paired.is_empty() {
                return Err(usb_err.context("no Wi-Fi pairing yet: connect the iPhone by USB once"));
            }
            on_event(DeviceEvent::Status("Looking for the iPhone on Wi-Fi…".into()));
            let found = {
                let snapshot: Vec<wifi::Paired> = std::mem::take(&mut paired);
                let (found, snapshot) = tokio::task::spawn_blocking(move || {
                    let r = wifi::discover(&snapshot, Duration::from_secs(4));
                    (r, snapshot)
                })
                .await?;
                paired = snapshot;
                found?
            };
            let Some((ip, port, idx)) = found else {
                bail!(
                    "no iPhone on USB, and none found on Wi-Fi (same network? phone unlocked? Windows network set to Private?)"
                );
            };
            on_event(DeviceEvent::Status(format!("Connecting over Wi-Fi ({ip})…")));
            let p = paired.swap_remove(idx);
            let info = p.info;
            let mut record = p.record;
            let tunnel = tokio::spawn(async move { wifi::open_tunnel(ip, port, &mut record).await })
                .await
                .map_err(|e| anyhow!("Wi-Fi tunnel setup crashed: {e}"))??;
            (tunnel, info)
        }
        Err(usb_err) => return Err(usb_err),
    };
    for (stage, ms) in &t.timings {
        tracing::info!("{:?} tunnel: {stage} {ms:.0} ms", info.transport);
    }
    if !t.handshake.services.contains_key(DISPLAY_SERVICE) {
        if ddi::installed(&mut t).await?.is_some() {
            bail!("the phone offers no display service even with the Developer Disk Image installed");
        }
        if !opts.auto_install_ddi {
            bail!("Developer Disk Image not installed (run with --install-ddi)");
        }
        on_event(DeviceEvent::Status("Installing the Developer Disk Image…".into()));
        ddi::install(&mut t).await.context("installing the Developer Disk Image")?;
        // The RSD service list is fixed at handshake time: reconnect.
        return Ok(Err("Developer Disk Image installed; reconnecting".into()));
    }

    let m = media::start(&mut t, 1).await?;
    let rtcp_port = m.rtcp_port.ok_or_else(|| anyhow!("device answer has no RTCP port"))?;
    let fb = feedback::spawn(m.video.clone(), rtcp_port, m.our_ssrc, opts.rctl, true);
    tracing::info!("media stream up in {:.0} ms", m.start_ms);
    Ok(Ok(Prepared { tunnel: t, media: m, fb, info }))
}

async fn rx_loop<S: MediaSink>(
    mut sink: S,
    video: Arc<UdpSocketHandle>,
    fb: Feedback,
    now_us: fn() -> u64,
    mut cancel: oneshot::Receiver<()>,
) -> (S, String) {
    use std::sync::atomic::Ordering::Relaxed;
    let mut pkts_in_frame = 0u32;
    loop {
        let dg = tokio::select! {
            _ = &mut cancel => return (sink, "stopped".into()),
            r = tokio::time::timeout(Duration::from_secs(5), video.recv()) => match r {
                Err(_) => return (sink, "no video for 5 s (is the phone locked?)".into()),
                Ok(Err(e)) => return (sink, format!("video socket closed: {e}")),
                Ok(Ok(dg)) => dg,
            },
        };
        let rx = now_us();
        if is_rtcp(&dg.data) {
            continue;
        }
        let Some(p) = RtpPacket::parse(&dg.data) else { continue };
        if p.payload_type != VIDEO_PT {
            continue;
        }
        fb.rx_state.on_packet(p.ssrc, p.sequence_number, p.timestamp);
        pkts_in_frame += 1;
        if p.marker {
            fb.frame_ack(p.timestamp);
            fb.rx_state.frames.fetch_add(1, Relaxed);
            fb.rx_state.last_frame_pkts.store(pkts_in_frame, Relaxed);
            pkts_in_frame = 0;
        }
        if sink.rtp(p.sequence_number, p.timestamp, p.marker, p.payload, rx) {
            fb.request_keyframe();
        }
    }
}

async fn read_info(provider: &NoDelayUsbmux, udid: &str) -> Result<DeviceInfo> {
    let pairing = provider.get_pairing_file().await.context("no pairing record: unlock the phone and tap Trust")?;
    let mut lockdown = LockdownClient::connect(provider).await?;
    lockdown.start_session(&pairing).await.context("lockdown session (is the phone trusted?)")?;
    let s = |v: Result<plist::Value, idevice::IdeviceError>| match v {
        Ok(plist::Value::String(s)) => s,
        _ => String::new(),
    };
    let name = s(lockdown.get_value(Some("DeviceName"), None).await);
    let product_type = s(lockdown.get_value(Some("ProductType"), None).await);
    let ios_version = s(lockdown.get_value(Some("ProductVersion"), None).await);
    let int = |v: Result<plist::Value, idevice::IdeviceError>| match v {
        Ok(plist::Value::Integer(i)) => i.as_unsigned().map(|u| u as u32),
        _ => None,
    };
    let screen_w = int(lockdown.get_value(Some("ScreenWidth"), Some("com.apple.mobile.iTunes")).await);
    let screen_h = int(lockdown.get_value(Some("ScreenHeight"), Some("com.apple.mobile.iTunes")).await);
    let dev_mode = lockdown
        .get_value(Some("DeveloperModeStatus"), Some("com.apple.security.mac.amfi"))
        .await;
    if matches!(dev_mode, Ok(plist::Value::Boolean(false))) {
        // The switch is hidden in Settings until a developer tool asks iOS to
        // show it. This changes no setting; the user still turns it on.
        if let Ok(mut amfi) = idevice::amfi::AmfiClient::connect(provider).await {
            let _ = amfi.reveal_developer_mode_option_in_ui().await;
        }
        bail!("Developer Mode is off: Settings > Privacy & Security > Developer Mode");
    }
    let (screen_w, screen_h) = match (screen_w, screen_h) {
        (Some(w), Some(h)) => (w, h),
        _ => bail!("lockdown did not report the screen size"),
    };
    Ok(DeviceInfo { udid: udid.to_string(), name, product_type, ios_version, screen_w, screen_h, transport: Transport::Usb })
}
