//! s3: live input over CoreDevice HID while the display stream holds the HID
//! auth gate open.
//!
//!   s3_input tap 32768 32768
//!   s3_input drag 32768 50000 32768 15000 --ms 400 --hz 120
//!   s3_input button home
//!   s3_input type "hello" --via indigo|uhs
//!   s3_input live            (window: mouse = finger, right click = Home, keys)
//!   s3_input latency X Y -n 10
//!   s3_input list

use std::time::Duration;

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand, ValueEnum};
use idevice::{
    ReadWrite, RsdService,
    core_device::{
        ButtonState, IndigoHidClient, KeyboardUsage, TOUCHSCREEN_STATE_CONTACT as CONTACT,
        TOUCHSCREEN_STATE_RELEASE as RELEASE, UniversalHidServiceClient,
    },
};
use spike::{
    conn::{NoDelayUsbmux, open_tunnel, usb_device},
    feedback::RctlMode,
    media, now_us, watch,
};
use tokio::sync::mpsc;

type Uhs = UniversalHidServiceClient<Box<dyn ReadWrite>>;
type Indigo = IndigoHidClient<Box<dyn ReadWrite>>;

#[derive(Parser)]
struct Args {
    #[arg(long, value_enum, default_value_t = RctlMode::Pmd3)]
    rctl: RctlMode,
    /// Wait after the media stream starts before sending input (auth gate).
    #[arg(long, default_value_t = 500)]
    settle_ms: u64,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Tap at normalized coordinates (0..65535).
    Tap { x: u16, y: u16 },
    /// Finger down, live moves sampled in real time, finger up.
    Drag {
        x1: u16,
        y1: u16,
        x2: u16,
        y2: u16,
        #[arg(long, default_value_t = 400)]
        ms: u64,
        #[arg(long, default_value_t = 120)]
        hz: u32,
        /// Report timestamp units: idevice default (wall-clock ns), host
        /// monotonic ns, or 24 MHz ticks (the A13's mach_absolute_time rate).
        #[arg(long, value_enum, default_value_t = TsMode::Default)]
        ts: TsMode,
    },
    /// Edge-swipe system gesture via IndigoDigitizerEvent (edge field set).
    /// Coordinates in 0..65535, converted to `--units` before sending.
    EdgeSwipe {
        #[arg(long, value_enum, default_value_t = Edge::Bottom)]
        edge: Edge,
        #[arg(long, value_enum, default_value_t = Units::Norm)]
        units: Units,
        #[arg(long, default_value_t = 32768)]
        x1: u16,
        #[arg(long, default_value_t = 65535)]
        y1: u16,
        #[arg(long, default_value_t = 32768)]
        x2: u16,
        #[arg(long, default_value_t = 30000)]
        y2: u16,
        #[arg(long, default_value_t = 200)]
        ms: u64,
        /// Hold still at the end point this long before End (App Switcher).
        #[arg(long, default_value_t = 0)]
        hold_ms: u64,
    },
    /// home | lock | volume-up | volume-down | mute | siri
    Button {
        name: String,
        #[arg(long)]
        hold_ms: Option<u64>,
    },
    /// Type ASCII text into the focused field.
    Type {
        text: String,
        #[arg(long, value_enum, default_value_t = KbPath::Indigo)]
        via: KbPath,
    },
    /// Send key chords in order via hid.indigo, e.g. `keys gui+a gui+c down enter gui+v`.
    Keys { chords: Vec<String> },
    /// Window that forwards the mouse and keyboard to the phone live.
    Live {
        #[arg(long, default_value_t = 390.0)]
        width: f64,
        #[arg(long, default_value_t = 844.0)]
        height: f64,
    },
    /// Tap on an idle screen and time until the first changed frame arrives.
    Latency {
        x: u16,
        y: u16,
        #[arg(short, default_value_t = 10)]
        n: u32,
        #[arg(long, default_value_t = 80)]
        hold_ms: u64,
        /// Fixed "changed frame" size threshold in bytes (default: 3x idle p95).
        #[arg(long)]
        threshold: Option<usize>,
        /// Print every frame for 400 ms after each CONTACT.
        #[arg(long)]
        trace: bool,
        /// Drag mode: finger down, wait until quiet, then move by this many
        /// units (sign alternates per trial) and time from the first move.
        #[arg(long, default_value_t = 0, allow_hyphen_values = true)]
        drag_dx: i32,
    },
    /// List the device's registered HID surfaces.
    List,
}

#[derive(Clone, Copy, ValueEnum)]
enum Edge {
    None,
    Top,
    Left,
    Bottom,
    Right,
}

/// Unknown which space IndigoDigitizerEvent points use; try each.
#[derive(Clone, Copy, ValueEnum)]
enum Units {
    /// 0.0 ..= 1.0
    Norm,
    /// iPhone 11 pixels, 828 x 1792
    Px,
    /// iPhone 11 points, 414 x 896
    Pt,
}

impl Units {
    fn map(self, x: u16, y: u16) -> (f64, f64) {
        let (fx, fy) = (x as f64 / 65535.0, y as f64 / 65535.0);
        match self {
            Units::Norm => (fx, fy),
            Units::Px => (fx * 828.0, fy * 1792.0),
            Units::Pt => (fx * 414.0, fy * 896.0),
        }
    }
}

#[derive(Clone, Copy, ValueEnum)]
enum TsMode {
    Default,
    Ns,
    Mach24,
}

impl TsMode {
    fn stamp(self) -> Option<u64> {
        let ns = spike::epoch().elapsed().as_nanos() as u64 + 1_000_000_000;
        let mask = (1u64 << 48) - 1;
        match self {
            TsMode::Default => None,
            TsMode::Ns => Some(ns & mask),
            TsMode::Mach24 => Some((ns * 3 / 125) & mask),
        }
    }
}

#[derive(Clone, Copy, ValueEnum)]
enum KbPath {
    /// IndigoKeyboardButtonEvent (com.apple.coredevice.hid.indigo)
    Indigo,
    /// Host-registered virtual keyboard on universalhidservice
    Uhs,
}

fn main() -> Result<()> {
    spike::init_logging();
    let args = Args::parse();
    let rt = tokio::runtime::Builder::new_multi_thread().enable_all().build()?;

    // Everything except the winit loop runs on the runtime.
    let setup = rt.block_on(async {
        let dev = usb_device().await?;
        let mut t = open_tunnel(&NoDelayUsbmux::new(&dev)).await?;
        let m = media::start(&mut t, 1).await?;
        eprintln!("media stream up in {:.0} ms", m.start_ms);
        let (aus, fb) = watch::spawn(&m, args.rctl);
        if fb.is_none() {
            eprintln!("WARNING: no RTCP port in answer; stream may stall");
        }
        tokio::time::sleep(Duration::from_millis(args.settle_ms)).await;
        let uhs = Uhs::connect_rsd(&mut t.adapter, &mut t.handshake)
            .await
            .context("connect hid.universalhidservice")?;
        let indigo = Indigo::connect_rsd(&mut t.adapter, &mut t.handshake)
            .await
            .context("connect hid.indigo")?;
        anyhow::Ok((t, m, aus, uhs, indigo))
    })?;
    // `t` owns the last AdapterHandle; dropping it would close the tunnel.
    let (_t, m, aus, mut uhs, mut indigo) = setup;

    match args.cmd {
        Cmd::Live { width, height } => live(&rt, width, height, uhs, indigo)?,
        cmd => rt.block_on(async {
            match cmd {
                Cmd::Tap { x, y } => {
                    let a = now_us();
                    uhs.send_touchscreen(CONTACT, x, y, None).await?;
                    let b = now_us();
                    tokio::time::sleep(Duration::from_millis(60)).await;
                    uhs.send_touchscreen(RELEASE, x, y, None).await?;
                    println!("tapped ({x},{y}); contact send took {} us", b - a);
                }
                Cmd::Drag { x1, y1, x2, y2, ms, hz, ts } => drag(&mut uhs, (x1, y1), (x2, y2), ms, hz, ts).await?,
                Cmd::Button { name, hold_ms } => button(&mut indigo, &name, hold_ms).await?,
                Cmd::EdgeSwipe { edge, units, x1, y1, x2, y2, ms, hold_ms } => {
                    edge_swipe(&mut indigo, edge, units, (x1, y1), (x2, y2), ms, hold_ms).await?
                }
                Cmd::Type { text, via } => type_text(&mut uhs, &mut indigo, &text, via).await?,
                Cmd::Keys { chords } => {
                    for c in &chords {
                        chord(&mut indigo, c).await?;
                        println!("sent {c}");
                        tokio::time::sleep(Duration::from_millis(300)).await;
                    }
                }
                Cmd::Latency { x, y, n, hold_ms, threshold, trace, drag_dx } => {
                    latency(&mut uhs, aus, x, y, n, hold_ms, threshold, trace, drag_dx).await?
                }
                Cmd::List => {
                    for s in uhs.list_connected_services().await? {
                        println!(
                            "[{}] {} (usage page {} / usage {})",
                            s.service_id,
                            s.product.as_deref().unwrap_or("(unnamed)"),
                            s.primary_usage_page.unwrap_or(0),
                            s.primary_usage.unwrap_or(0)
                        );
                    }
                }
                Cmd::Live { .. } => unreachable!(),
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
            anyhow::Ok(())
        })?,
    }

    rt.block_on(async {
        let mut client = m.client;
        let _ = client.stop_media_stream().await;
    });
    Ok(())
}

/// A drag whose samples are generated and sent in real time: each CONTACT
/// leaves the host the moment its sample time arrives, like a mouse would.
async fn drag(uhs: &mut Uhs, from: (u16, u16), to: (u16, u16), ms: u64, hz: u32, ts: TsMode) -> Result<()> {
    let period = Duration::from_secs_f64(1.0 / hz as f64);
    let mut tick = tokio::time::interval(period);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let start = now_us();
    let mut sends = Vec::new();
    let mut stamps = Vec::new();
    loop {
        tick.tick().await;
        let elapsed = (now_us() - start) as f64 / 1000.0;
        let f = (elapsed / ms as f64).min(1.0);
        let x = lerp(from.0, to.0, f);
        let y = lerp(from.1, to.1, f);
        let a = now_us();
        uhs.send_touchscreen(CONTACT, x, y, ts.stamp()).await?;
        sends.push(now_us() - a);
        stamps.push(a);
        if f >= 1.0 {
            break;
        }
    }
    uhs.send_touchscreen(RELEASE, to.0, to.1, ts.stamp()).await?;
    let span = (stamps.last().unwrap() - stamps[0]) as f64 / 1e6;
    println!(
        "drag: {} live CONTACT samples over {:.0} ms ({:.0} Hz achieved), then RELEASE",
        sends.len(),
        span * 1000.0,
        (sends.len() - 1) as f64 / span
    );
    print_pct("per-sample send call", &mut sends);
    Ok(())
}

async fn edge_swipe(
    indigo: &mut Indigo,
    edge: Edge,
    units: Units,
    from: (u16, u16),
    to: (u16, u16),
    ms: u64,
    hold_ms: u64,
) -> Result<()> {
    use idevice::core_device::{DigitizerEdge as E, DigitizerEventType as T, DigitizerTarget};
    let e = match edge {
        Edge::None => E::None,
        Edge::Top => E::Top,
        Edge::Left => E::Left,
        Edge::Bottom => E::Bottom,
        Edge::Right => E::Right,
    };
    let send = |p: (u16, u16)| units.map(p.0, p.1);
    let mut tick = tokio::time::interval(Duration::from_millis(8));
    let steps = (ms / 8).max(2);
    indigo.send_digitizer(send(from), None, T::Start, e, DigitizerTarget::MainScreen).await?;
    for i in 1..steps {
        tick.tick().await;
        let f = i as f64 / steps as f64;
        let p = (lerp(from.0, to.0, f), lerp(from.1, to.1, f));
        indigo.send_digitizer(send(p), None, T::Position, e, DigitizerTarget::MainScreen).await?;
    }
    // Holding still: keep reporting the same position, as a resting finger would.
    let hold_until = tokio::time::Instant::now() + Duration::from_millis(hold_ms);
    while tokio::time::Instant::now() < hold_until {
        tick.tick().await;
        indigo.send_digitizer(send(to), None, T::Position, e, DigitizerTarget::MainScreen).await?;
    }
    tick.tick().await;
    indigo.send_digitizer(send(to), None, T::End, e, DigitizerTarget::MainScreen).await?;
    let (a, b) = (send(from), send(to));
    println!(
        "IndigoDigitizerEvent edge swipe: start ({:.3},{:.3}) -> end ({:.3},{:.3}), {} positions over ~{ms} ms, held {hold_ms} ms",
        a.0, a.1, b.0, b.1, steps - 1
    );
    Ok(())
}

fn lerp(a: u16, b: u16, f: f64) -> u16 {
    (a as f64 + (b as f64 - a as f64) * f).round() as u16
}

/// Named buttons: (usage page, usage, default hold). Usages from idevice's
/// hid tool and pymobiledevice3 (Consumer page 0x0C); they agree.
fn named_button(name: &str) -> Option<(u64, u64, u64)> {
    Some(match name {
        "home" => (0x0C, 0x40, 80),
        "lock" | "power" => (0x0C, 0x30, 200),
        "volume-up" => (0x0C, 0xE9, 80),
        "volume-down" => (0x0C, 0xEA, 80),
        "mute" => (0x0C, 0xE2, 80),
        "siri" => (0x0C, 0xCF, 1200),
        _ => return None,
    })
}

async fn button(indigo: &mut Indigo, name: &str, hold: Option<u64>) -> Result<()> {
    let Some((page, usage, default_hold)) = named_button(name) else {
        bail!("unknown button {name}");
    };
    let hold = hold.unwrap_or(default_hold);
    let a = now_us();
    indigo.send_button(page, usage, ButtonState::Down).await?;
    let b = now_us();
    tokio::time::sleep(Duration::from_millis(hold)).await;
    indigo.send_button(page, usage, ButtonState::Up).await?;
    println!("pressed {name} (held {hold} ms); down send took {} us", b - a);
    Ok(())
}

const LEFT_SHIFT: u16 = 0xE1;

async fn type_text(uhs: &mut Uhs, indigo: &mut Indigo, text: &str, via: KbPath) -> Result<()> {
    let mut kb = match via {
        KbPath::Uhs => Some(uhs.create_main_keyboard().await.map_err(|e| anyhow::anyhow!("{e}"))?),
        KbPath::Indigo => None,
    };
    if kb.is_some() {
        // Nothing signals when iOS has attached the new keyboard.
        tokio::time::sleep(Duration::from_millis(300)).await;
    }
    let mut sends = Vec::new();
    for ch in text.chars() {
        let Some((usage, shift)) = ascii_usage(ch) else {
            eprintln!("skipping {ch:?}");
            continue;
        };
        let mut seq = Vec::new();
        if shift {
            seq.push((LEFT_SHIFT, true));
        }
        seq.push((usage, true));
        seq.push((usage, false));
        if shift {
            seq.push((LEFT_SHIFT, false));
        }
        for (u, down) in seq {
            let a = now_us();
            match kb.as_mut() {
                None => {
                    let st = if down { ButtonState::Down } else { ButtonState::Up };
                    indigo.send_keyboard(u as u64, st).await?;
                }
                Some(svc) => {
                    let ku = KeyboardUsage::new(u).map_err(|e| anyhow::anyhow!("{e}"))?;
                    let r = if down {
                        uhs.main_keyboard_key_down(svc, ku).await
                    } else {
                        uhs.main_keyboard_key_up(svc, ku).await
                    };
                    r.map_err(|e| anyhow::anyhow!("{e}"))?;
                }
            }
            sends.push(now_us() - a);
        }
        tokio::time::sleep(Duration::from_millis(12)).await;
    }
    if let Some(mut svc) = kb {
        let _ = uhs.remove_main_keyboard(&mut svc).await;
    }
    println!("typed {:?} via {}", text, match via { KbPath::Indigo => "indigo", KbPath::Uhs => "uhs" });
    print_pct("per-report send call", &mut sends);
    Ok(())
}

/// Press modifiers, tap the key, release modifiers. `gui+a`, `shift+tab`, `down`.
async fn chord(indigo: &mut Indigo, spec: &str) -> Result<()> {
    let mut parts: Vec<&str> = spec.split('+').collect();
    let key = parts.pop().unwrap_or_default();
    let mut mods = Vec::new();
    for m in parts {
        mods.push(match m {
            "gui" | "cmd" => 0xE3u16,
            "ctrl" => 0xE0,
            "shift" => 0xE1,
            "alt" | "opt" => 0xE2,
            other => bail!("unknown modifier {other}"),
        });
    }
    let usage = match key {
        "enter" => 0x28,
        "esc" => 0x29,
        "backspace" => 0x2A,
        "tab" => 0x2B,
        "space" => 0x2C,
        "right" => 0x4F,
        "left" => 0x50,
        "down" => 0x51,
        "up" => 0x52,
        k if k.chars().count() == 1 => ascii_usage(k.chars().next().unwrap())
            .map(|(u, _)| u)
            .ok_or_else(|| anyhow::anyhow!("no usage for {k}"))?,
        other => bail!("unknown key {other}"),
    };
    for &m in &mods {
        indigo.send_keyboard(m as u64, ButtonState::Down).await?;
    }
    indigo.send_keyboard(usage as u64, ButtonState::Down).await?;
    indigo.send_keyboard(usage as u64, ButtonState::Up).await?;
    for &m in mods.iter().rev() {
        indigo.send_keyboard(m as u64, ButtonState::Up).await?;
    }
    Ok(())
}

/// US-layout ASCII -> (HID keyboard usage, needs shift).
fn ascii_usage(c: char) -> Option<(u16, bool)> {
    const SHIFTED_DIGITS: &str = "!@#$%^&*()";
    Some(match c {
        'a'..='z' => (0x04 + (c as u16 - 'a' as u16), false),
        'A'..='Z' => (0x04 + (c as u16 - 'A' as u16), true),
        '1'..='9' => (0x1E + (c as u16 - '1' as u16), false),
        '0' => (0x27, false),
        '\n' => (0x28, false),
        '\t' => (0x2B, false),
        ' ' => (0x2C, false),
        c if SHIFTED_DIGITS.contains(c) => (0x1E + SHIFTED_DIGITS.find(c).unwrap() as u16, true),
        '-' => (0x2D, false),
        '_' => (0x2D, true),
        '=' => (0x2E, false),
        '+' => (0x2E, true),
        '[' => (0x2F, false),
        '{' => (0x2F, true),
        ']' => (0x30, false),
        '}' => (0x30, true),
        '\\' => (0x31, false),
        '|' => (0x31, true),
        ';' => (0x33, false),
        ':' => (0x33, true),
        '\'' => (0x34, false),
        '"' => (0x34, true),
        '`' => (0x35, false),
        '~' => (0x35, true),
        ',' => (0x36, false),
        '<' => (0x36, true),
        '.' => (0x37, false),
        '>' => (0x37, true),
        '/' => (0x38, false),
        '?' => (0x38, true),
        _ => return None,
    })
}

/// Rough input-to-encoder latency: on an idle screen, send CONTACT and time
/// until the first access unit that is clearly larger than idle frames starts
/// arriving. Covers iOS input handling + render + encode + transport; not our
/// decode/present (s4 measures decode).
async fn latency(
    uhs: &mut Uhs,
    mut aus: mpsc::UnboundedReceiver<watch::AuEvent>,
    x: u16,
    y: u16,
    n: u32,
    hold_ms: u64,
    fixed_threshold: Option<usize>,
    trace: bool,
    drag_dx: i32,
) -> Result<()> {
    // Baseline: what does an idle screen send?
    let mut idle_sizes = Vec::new();
    let until = tokio::time::Instant::now() + Duration::from_secs(2);
    while let Ok(Some(au)) = tokio::time::timeout_at(until, aus.recv()).await {
        idle_sizes.push(au.bytes as u64);
    }
    idle_sizes.sort_unstable();
    let p95 = idle_sizes.get(idle_sizes.len().saturating_sub(1) * 95 / 100).copied().unwrap_or(0);
    let threshold = fixed_threshold.unwrap_or((p95 * 3).max(1500) as usize);
    println!(
        "idle baseline: {} AUs in 2 s, p95 {} B -> 'changed frame' threshold {} B",
        idle_sizes.len(),
        p95,
        threshold
    );

    let mut from_contact = Vec::new();
    let mut from_contact_last = Vec::new();
    for i in 1..=n {
        // Wait for the screen to go quiet (no large frame for 700 ms).
        let quiet_deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        loop {
            match tokio::time::timeout(Duration::from_millis(700), aus.recv()).await {
                Err(_) => break,
                Ok(None) => bail!("video stream ended"),
                Ok(Some(au)) if au.bytes > threshold => {}
                Ok(Some(_)) => {}
            }
            if tokio::time::Instant::now() > quiet_deadline {
                break;
            }
        }
        while aus.try_recv().is_ok() {}

        // In drag mode the finger lands first; the timed event is the move.
        let mut end = (x, y);
        if drag_dx != 0 {
            uhs.send_touchscreen(CONTACT, x, y, None).await?;
            // Touch-down can start its own animation (icon press, cell
            // highlight). Only time the move once nothing large has arrived
            // for 250 ms; otherwise the trial is invalid.
            let cap = tokio::time::Instant::now() + Duration::from_secs(2);
            let mut quiet_until = tokio::time::Instant::now() + Duration::from_millis(250);
            let mut quiet = false;
            while tokio::time::Instant::now() < cap {
                match tokio::time::timeout_at(quiet_until, aus.recv()).await {
                    Err(_) => {
                        quiet = true;
                        break;
                    }
                    Ok(None) => bail!("video stream ended"),
                    // Small frames are the encoder's idle cadence: still quiet.
                    Ok(Some(au)) if au.bytes > threshold => {
                        quiet_until = tokio::time::Instant::now() + Duration::from_millis(250);
                    }
                    Ok(Some(_)) => {}
                }
            }
            if !quiet {
                uhs.send_touchscreen(RELEASE, x, y, None).await?;
                println!("trial {i:>2}: INVALID (screen never went quiet after touch-down)");
                tokio::time::sleep(Duration::from_millis(500)).await;
                continue;
            }
        }
        let t0 = now_us();
        if drag_dx != 0 {
            let dx = if i % 2 == 1 { drag_dx } else { -drag_dx };
            // Three samples 8 ms apart so the motion clears the touch slop.
            for k in 1..=3 {
                let nx = (x as i32 + dx * k / 3).clamp(0, 65535) as u16;
                uhs.send_touchscreen(CONTACT, nx, y, None).await?;
                end = (nx, y);
                if k < 3 {
                    tokio::time::sleep(Duration::from_millis(8)).await;
                }
            }
        } else {
            uhs.send_touchscreen(CONTACT, x, y, None).await?;
        }
        let (x, y) = end;
        let release_at = tokio::time::Instant::now() + Duration::from_millis(hold_ms);
        let deadline = tokio::time::Instant::now() + Duration::from_millis(1500);
        let mut released = false;
        let mut hit = None;
        loop {
            tokio::select! {
                au = aus.recv() => {
                    let Some(au) = au else { bail!("video stream ended") };
                    if trace && au.first_pkt_us > t0 && au.first_pkt_us - t0 < 400_000 {
                        println!(
                            "    +{:>6.1} ms  {:>6} B{}",
                            (au.first_pkt_us - t0) as f64 / 1e3,
                            au.bytes,
                            if au.key { " key" } else { "" }
                        );
                    }
                    if au.first_pkt_us > t0 && au.bytes > threshold && !au.corrupt {
                        hit = Some(au);
                        break;
                    }
                }
                _ = tokio::time::sleep_until(release_at), if !released => {
                    uhs.send_touchscreen(RELEASE, x, y, None).await?;
                    released = true;
                }
                _ = tokio::time::sleep_until(deadline) => break,
            }
        }
        if !released {
            tokio::time::sleep_until(release_at).await;
            uhs.send_touchscreen(RELEASE, x, y, None).await?;
        }
        match hit {
            Some(au) => {
                let a = au.first_pkt_us - t0;
                let b = au.last_pkt_us - t0;
                println!(
                    "trial {i:>2}: first pkt +{:.1} ms, frame complete +{:.1} ms ({} B{}){}",
                    a as f64 / 1e3,
                    b as f64 / 1e3,
                    au.bytes,
                    if au.key { ", keyframe" } else { "" },
                    if a > hold_ms * 1000 { "  [after release]" } else { "" }
                );
                from_contact.push(a);
                from_contact_last.push(b);
            }
            None => println!("trial {i:>2}: no changed frame within 1.5 s"),
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    print_pct("CONTACT -> first packet of changed frame", &mut from_contact);
    print_pct("CONTACT -> changed frame complete", &mut from_contact_last);
    Ok(())
}

fn print_pct(name: &str, v: &mut [u64]) {
    if v.is_empty() {
        println!("{name}: no samples");
        return;
    }
    v.sort_unstable();
    let p = |q: f64| v[((v.len() - 1) as f64 * q).round() as usize] as f64 / 1000.0;
    println!(
        "{name}: min {:.2} p50 {:.2} p95 {:.2} max {:.2} ms (n={})",
        p(0.0),
        p(0.5),
        p(0.95),
        p(1.0),
        v.len()
    );
}

// ---------------------------------------------------------------- live mode

enum Ev {
    Down(u16, u16),
    Move(u16, u16),
    Up(u16, u16),
    Home,
    Key(u16, bool),
}

fn live(rt: &tokio::runtime::Runtime, width: f64, height: f64, mut uhs: Uhs, mut indigo: Indigo) -> Result<()> {
    use winit::{
        application::ApplicationHandler,
        dpi::{LogicalSize, PhysicalSize},
        event::{ElementState, MouseButton, WindowEvent},
        event_loop::{ActiveEventLoop, EventLoop},
        keyboard::PhysicalKey,
        window::{Window, WindowId},
    };

    let (tx, mut rx) = mpsc::unbounded_channel::<(u64, Ev)>();

    // Input sender: never waits on video. Moves are coalesced only when a
    // backlog has built up (several queued moves -> keep the newest).
    let sender = rt.spawn(async move {
        let mut queue_us = Vec::new();
        let mut send_us = Vec::new();
        let mut coalesced = 0u64;
        while let Some(first) = rx.recv().await {
            let mut batch = vec![first];
            while let Ok(e) = rx.try_recv() {
                batch.push(e);
            }
            // A move is superseded if a newer move or the release is queued
            // right behind it (both carry a fresher position).
            let superseded: Vec<bool> = (0..batch.len())
                .map(|i| {
                    matches!(batch[i].1, Ev::Move(..))
                        && matches!(batch.get(i + 1).map(|e| &e.1), Some(Ev::Move(..) | Ev::Up(..)))
                })
                .collect();
            for (i, (t_ev, ev)) in batch.into_iter().enumerate() {
                if superseded[i] {
                    coalesced += 1;
                    continue;
                }
                let a = now_us();
                let r = match ev {
                    Ev::Down(x, y) | Ev::Move(x, y) => uhs.send_touchscreen(CONTACT, x, y, None).await,
                    Ev::Up(x, y) => uhs.send_touchscreen(RELEASE, x, y, None).await,
                    Ev::Home => {
                        let r = indigo.send_button(0x0C, 0x40, ButtonState::Down).await;
                        tokio::time::sleep(Duration::from_millis(80)).await;
                        r.and(indigo.send_button(0x0C, 0x40, ButtonState::Up).await)
                    }
                    Ev::Key(u, down) => {
                        let st = if down { ButtonState::Down } else { ButtonState::Up };
                        indigo.send_keyboard(u as u64, st).await
                    }
                };
                if let Err(e) = r {
                    eprintln!("send failed: {e:?}");
                }
                queue_us.push(a - t_ev);
                send_us.push(now_us() - a);
            }
        }
        println!("\nlive session: {} events sent, {coalesced} moves coalesced", send_us.len());
        print_pct("event -> send start (queueing)", &mut queue_us);
        print_pct("send call", &mut send_us);
    });

    struct App {
        tx: mpsc::UnboundedSender<(u64, Ev)>,
        window: Option<Window>,
        size: PhysicalSize<u32>,
        cursor: (f64, f64),
        down: bool,
        logical: (f64, f64),
    }
    impl App {
        fn norm(&self) -> (u16, u16) {
            let w = self.size.width.max(1) as f64;
            let h = self.size.height.max(1) as f64;
            let x = (self.cursor.0 / w * 65535.0).clamp(0.0, 65535.0) as u16;
            let y = (self.cursor.1 / h * 65535.0).clamp(0.0, 65535.0) as u16;
            (x, y)
        }
        fn send(&self, ev: Ev) {
            let _ = self.tx.send((now_us(), ev));
        }
    }
    impl ApplicationHandler for App {
        fn resumed(&mut self, el: &ActiveEventLoop) {
            if self.window.is_none() {
                let attrs = Window::default_attributes()
                    .with_title("s3 live input (no video) - drag = finger, right click = Home")
                    .with_inner_size(LogicalSize::new(self.logical.0, self.logical.1));
                let w = el.create_window(attrs).expect("create window");
                self.size = w.inner_size();
                self.window = Some(w);
            }
        }
        fn window_event(&mut self, el: &ActiveEventLoop, _: WindowId, event: WindowEvent) {
            match event {
                WindowEvent::CloseRequested => el.exit(),
                WindowEvent::Resized(s) => self.size = s,
                WindowEvent::CursorMoved { position, .. } => {
                    self.cursor = (position.x, position.y);
                    if self.down {
                        let (x, y) = self.norm();
                        self.send(Ev::Move(x, y));
                    }
                }
                WindowEvent::CursorLeft { .. } if self.down => {
                    self.down = false;
                    let (x, y) = self.norm();
                    self.send(Ev::Up(x, y));
                }
                WindowEvent::MouseInput { state, button, .. } => {
                    let (x, y) = self.norm();
                    match (button, state) {
                        (MouseButton::Left, ElementState::Pressed) => {
                            self.down = true;
                            self.send(Ev::Down(x, y));
                        }
                        (MouseButton::Left, ElementState::Released) if self.down => {
                            self.down = false;
                            self.send(Ev::Up(x, y));
                        }
                        (MouseButton::Right, ElementState::Pressed) => self.send(Ev::Home),
                        _ => {}
                    }
                }
                WindowEvent::KeyboardInput { event, .. } if !event.repeat => {
                    if let PhysicalKey::Code(code) = event.physical_key
                        && let Some(u) = hid_usage(code)
                    {
                        self.send(Ev::Key(u, event.state == ElementState::Pressed));
                    }
                }
                _ => {}
            }
        }
    }

    let event_loop = EventLoop::new()?;
    let mut app = App {
        tx,
        window: None,
        size: PhysicalSize::new(1, 1),
        cursor: (0.0, 0.0),
        down: false,
        logical: (width, height),
    };
    println!("live: drag in the window; close it to finish and print timings");
    event_loop.run_app(&mut app)?;
    drop(app);
    rt.block_on(sender)?;
    Ok(())
}

/// winit physical key -> HID keyboard usage. Ctrl maps to GUI (Command), so
/// Ctrl+C/V/X/A/Z arrive as their Command equivalents.
fn hid_usage(code: winit::keyboard::KeyCode) -> Option<u16> {
    use winit::keyboard::KeyCode as K;
    let letters = [
        K::KeyA, K::KeyB, K::KeyC, K::KeyD, K::KeyE, K::KeyF, K::KeyG, K::KeyH, K::KeyI, K::KeyJ,
        K::KeyK, K::KeyL, K::KeyM, K::KeyN, K::KeyO, K::KeyP, K::KeyQ, K::KeyR, K::KeyS, K::KeyT,
        K::KeyU, K::KeyV, K::KeyW, K::KeyX, K::KeyY, K::KeyZ,
    ];
    if let Some(i) = letters.iter().position(|k| *k == code) {
        return Some(0x04 + i as u16);
    }
    let digits = [
        K::Digit1, K::Digit2, K::Digit3, K::Digit4, K::Digit5, K::Digit6, K::Digit7, K::Digit8,
        K::Digit9, K::Digit0,
    ];
    if let Some(i) = digits.iter().position(|k| *k == code) {
        return Some(0x1E + i as u16);
    }
    Some(match code {
        K::Enter => 0x28,
        K::Escape => 0x29,
        K::Backspace => 0x2A,
        K::Tab => 0x2B,
        K::Space => 0x2C,
        K::Minus => 0x2D,
        K::Equal => 0x2E,
        K::BracketLeft => 0x2F,
        K::BracketRight => 0x30,
        K::Backslash => 0x31,
        K::Semicolon => 0x33,
        K::Quote => 0x34,
        K::Backquote => 0x35,
        K::Comma => 0x36,
        K::Period => 0x37,
        K::Slash => 0x38,
        K::Delete => 0x4C,
        K::ArrowRight => 0x4F,
        K::ArrowLeft => 0x50,
        K::ArrowDown => 0x51,
        K::ArrowUp => 0x52,
        K::ControlLeft => 0xE3,  // -> Left GUI (Command)
        K::ControlRight => 0xE7, // -> Right GUI
        K::ShiftLeft => 0xE1,
        K::ShiftRight => 0xE5,
        K::AltLeft => 0xE2,
        K::AltRight => 0xE6,
        _ => return None,
    })
}
