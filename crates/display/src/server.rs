//! The iPad side is a web page in Safari: this serves it, streams H.264 to
//! it over a WebSocket (one access unit per binary message) and the mouse
//! pointer as small text messages (see `cursor`). The page sends "k" for a
//! keyframe (after a decoder reset) and log lines.
//!
//! Latency first: each viewer may have only a few frames queued. When
//! Wi-Fi can't keep up, frames are skipped until the next keyframe (which
//! is requested) instead of queueing, so delay never builds up. Pointer
//! messages are tiny and always go.

use std::{
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering::Relaxed},
        mpsc::{Receiver, Sender, channel},
    },
    thread,
};

use anyhow::{Context, Result};

/// Frames a viewer may have queued before it is considered behind.
const QUEUE: usize = 3;

const PAGE: &str = include_str!("viewer.html");

/// What goes out to a viewer, already framed.
enum Out {
    Frame(Arc<Vec<u8>>),
    Pointer(Arc<Vec<u8>>),
}

struct Viewer {
    tx: Sender<Out>,
    /// Frames handed over but not yet written to the socket.
    queued: Arc<AtomicUsize>,
    /// To hang up when the display stops.
    conn: TcpStream,
    /// Behind: skip frames until a keyframe.
    waiting_key: bool,
}

/// The pointer as last sent: shape and position messages, for viewers
/// that join later.
#[derive(Default)]
struct PointerState {
    shape: Option<Arc<Vec<u8>>>,
    position: Option<Arc<Vec<u8>>>,
}

#[derive(Clone)]
pub struct Hub {
    viewers: Arc<Mutex<Vec<Viewer>>>,
    pointer: Arc<Mutex<PointerState>>,
    /// Set when a viewer needs a keyframe (joined, fell behind, asked).
    pub want_key: Arc<AtomicBool>,
}

impl Hub {
    pub fn viewer_count(&self) -> usize {
        self.viewers.lock().unwrap().len()
    }

    /// Hang up on every viewer (the display is stopping): the pages then
    /// say they are waiting for Mirrorly.
    pub fn close_all(&self) {
        for v in self.viewers.lock().unwrap().drain(..) {
            let _ = v.conn.shutdown(std::net::Shutdown::Both);
        }
    }

    /// Hand one access unit to every viewer.
    pub fn broadcast(&self, au: &[u8], keyframe: bool) {
        let mut vs = self.viewers.lock().unwrap();
        if vs.is_empty() {
            return;
        }
        let msg = Arc::new(ws_frame(0x2, au));
        vs.retain_mut(|v| {
            if v.waiting_key && !keyframe {
                return true;
            }
            if v.queued.load(Relaxed) >= QUEUE {
                v.waiting_key = true;
                self.want_key.store(true, Relaxed);
                return true;
            }
            v.queued.fetch_add(1, Relaxed);
            v.waiting_key = false;
            v.tx.send(Out::Frame(msg.clone())).is_ok()
        });
    }

    /// A new pointer shape (`cursor::shape_message`).
    pub fn pointer_shape(&self, text: &str) {
        let msg = Arc::new(ws_frame(0x1, text.as_bytes()));
        self.pointer.lock().unwrap().shape = Some(msg.clone());
        self.send_pointer(msg);
    }

    /// The pointer's new position (`cursor::position_message`).
    pub fn pointer_position(&self, text: &str) {
        let msg = Arc::new(ws_frame(0x1, text.as_bytes()));
        self.pointer.lock().unwrap().position = Some(msg.clone());
        self.send_pointer(msg);
    }

    fn send_pointer(&self, msg: Arc<Vec<u8>>) {
        self.viewers.lock().unwrap().retain(|v| v.tx.send(Out::Pointer(msg.clone())).is_ok());
    }
}

/// A server frame (unmasked).
fn ws_frame(opcode: u8, payload: &[u8]) -> Vec<u8> {
    let n = payload.len();
    let mut f = Vec::with_capacity(n + 10);
    f.push(0x80 | opcode);
    if n < 126 {
        f.push(n as u8);
    } else if n < 65536 {
        f.push(126);
        f.extend_from_slice(&(n as u16).to_be_bytes());
    } else {
        f.push(127);
        f.extend_from_slice(&(n as u64).to_be_bytes());
    }
    f.extend_from_slice(payload);
    f
}

/// Sec-WebSocket-Accept for a client key (RFC 6455).
fn accept_key(key: &str) -> String {
    let digest = ring::digest::digest(
        &ring::digest::SHA1_FOR_LEGACY_USE_ONLY,
        format!("{key}258EAFA5-E914-47DA-95CA-C5AB0DC85B11").as_bytes(),
    );
    base64(digest.as_ref())
}

pub(crate) fn base64(data: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut s = String::new();
    for c in data.chunks(3) {
        let b = [c[0], *c.get(1).unwrap_or(&0), *c.get(2).unwrap_or(&0)];
        let n = (b[0] as u32) << 16 | (b[1] as u32) << 8 | b[2] as u32;
        for i in 0..4 {
            if i <= c.len() {
                s.push(T[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                s.push('=');
            }
        }
    }
    s
}

/// Letters for the address's secret: no 0/o, 1/l look-alikes.
const KEY_CHARS: &[u8; 32] = b"abcdefghijkmnpqrstuvwxyz23456789";

/// The secret part of the display's address (8 characters, 40 bits),
/// made once and kept, so a Home Screen shortcut keeps working. Without
/// it the server shows nothing: anyone else on the network could
/// otherwise watch the screen.
pub fn key() -> String {
    let base = std::env::var_os("LOCALAPPDATA").map(std::path::PathBuf::from).unwrap_or_else(std::env::temp_dir);
    let file = base.join("Mirrorly").join("display-key.txt");
    if let Ok(k) = std::fs::read_to_string(&file)
        && valid_key(k.trim())
    {
        return k.trim().to_string();
    }
    let mut bytes = [0u8; 8];
    ring::rand::SecureRandom::fill(&ring::rand::SystemRandom::new(), &mut bytes).expect("system random numbers");
    let k: String = bytes.iter().map(|b| KEY_CHARS[(b & 31) as usize] as char).collect();
    if let Err(e) = std::fs::create_dir_all(file.parent().unwrap_or(&file)).and_then(|_| std::fs::write(&file, &k)) {
        tracing::warn!("cannot save the display address: {e}");
    }
    k
}

fn valid_key(k: &str) -> bool {
    k.len() == 8 && k.bytes().all(|c| KEY_CHARS.contains(&c))
}

/// What a request path asks for, given the secret.
#[derive(Debug, PartialEq)]
enum Route {
    Page,
    Stream,
    Unknown,
}

fn route(path: &str, key: &str) -> Route {
    let path = path.split('?').next().unwrap_or("");
    let Some(rest) = path.strip_prefix('/').and_then(|p| p.strip_prefix(key)) else { return Route::Unknown };
    match rest {
        "" | "/" => Route::Page,
        "/stream" => Route::Stream,
        _ => Route::Unknown,
    }
}

/// Listen on `port` (all interfaces: the iPad comes over Wi-Fi).
pub fn start(port: u16, stop: Arc<AtomicBool>) -> Result<Hub> {
    let key = Arc::new(key());
    let listener = TcpListener::bind(("0.0.0.0", port)).with_context(|| format!("listen on port {port}"))?;
    listener.set_nonblocking(true)?;
    let hub = Hub { viewers: Arc::default(), pointer: Arc::default(), want_key: Arc::new(AtomicBool::new(false)) };
    let h = hub.clone();
    thread::Builder::new().name("display-server".into()).spawn(move || {
        while !stop.load(Relaxed) {
            match listener.accept() {
                Ok((conn, peer)) => {
                    let (h, key) = (h.clone(), key.clone());
                    let _ = thread::Builder::new().name("display-conn".into()).spawn(move || {
                        if let Err(e) = handle(conn, &h, &key) {
                            tracing::info!("viewer {peer}: {e:#}");
                        }
                    });
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => thread::sleep(std::time::Duration::from_millis(50)),
                Err(e) => tracing::warn!("accept: {e}"),
            }
        }
    })?;
    Ok(hub)
}

fn handle(mut conn: TcpStream, hub: &Hub, secret: &str) -> Result<()> {
    conn.set_nonblocking(false)?;
    conn.set_nodelay(true)?;
    let mut req = Vec::new();
    let mut buf = [0u8; 4096];
    while !req.windows(4).any(|w| w == b"\r\n\r\n") {
        let n = conn.read(&mut buf)?;
        if n == 0 || req.len() > 16384 {
            return Ok(());
        }
        req.extend_from_slice(&buf[..n]);
    }
    let head = String::from_utf8_lossy(&req);
    let path = head.split_whitespace().nth(1).unwrap_or("/");
    let key = head
        .lines()
        .find_map(|l| l.split_once(':').filter(|(k, _)| k.trim().eq_ignore_ascii_case("sec-websocket-key")).map(|(_, v)| v.trim().to_string()));

    let (status, body) = match (route(path, secret), key) {
        (Route::Stream, Some(key)) => {
            let head = format!(
                "HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: {}\r\n\r\n",
                accept_key(&key)
            );
            conn.write_all(head.as_bytes())?;
            return viewer(conn, hub);
        }
        (Route::Page, _) => ("200 OK", PAGE),
        _ => {
            tracing::info!("display: refused a request without the address's key");
            ("404 Not Found", NOT_FOUND)
        }
    };
    let head = format!(
        "HTTP/1.1 {status}\r\nContent-Type: text/html; charset=utf-8\r\nCache-Control: no-store\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    conn.write_all(head.as_bytes())?;
    conn.write_all(body.as_bytes())?;
    Ok(())
}

const NOT_FOUND: &str = "<!doctype html><meta name=viewport content=\"width=device-width\"><title>Mirrorly</title>\
<p style=\"font:17px -apple-system,sans-serif;padding:24px\">Open the full address that Mirrorly shows on the PC.</p>";

fn viewer(conn: TcpStream, hub: &Hub) -> Result<()> {
    let (tx, rx): (Sender<Out>, Receiver<Out>) = channel();
    // The pointer as it is now, before any frame.
    {
        let p = hub.pointer.lock().unwrap();
        for msg in [&p.shape, &p.position].into_iter().flatten() {
            let _ = tx.send(Out::Pointer(msg.clone()));
        }
    }
    let queued = Arc::new(AtomicUsize::new(0));
    hub.viewers.lock().unwrap().push(Viewer { tx, queued: queued.clone(), conn: conn.try_clone()?, waiting_key: true });
    hub.want_key.store(true, Relaxed);
    tracing::info!("iPad viewer connected ({})", conn.peer_addr().map(|a| a.to_string()).unwrap_or_default());

    // Writer: frames out, in order.
    let mut out = conn.try_clone()?;
    let writer = thread::Builder::new().name("display-send".into()).spawn(move || {
        while let Ok(msg) = rx.recv() {
            let (bytes, frame) = match &msg {
                Out::Frame(b) => (b, true),
                Out::Pointer(b) => (b, false),
            };
            if out.write_all(bytes).is_err() {
                return;
            }
            if frame {
                queued.fetch_sub(1, Relaxed);
            }
        }
    })?;

    // Reader: keyframe requests in.
    let r = read_messages(conn, hub);
    tracing::info!("iPad viewer left");
    // Dropping our sender (via retain on the next broadcast) ends the writer.
    drop(writer);
    r
}

fn read_exact(conn: &mut TcpStream, n: usize) -> std::io::Result<Vec<u8>> {
    let mut b = vec![0u8; n];
    conn.read_exact(&mut b)?;
    Ok(b)
}

fn read_messages(mut conn: TcpStream, hub: &Hub) -> Result<()> {
    loop {
        let h = read_exact(&mut conn, 2)?;
        let (op, masked, mut n) = (h[0] & 0x0F, h[1] & 0x80 != 0, (h[1] & 0x7F) as u64);
        if n == 126 {
            n = u16::from_be_bytes(read_exact(&mut conn, 2)?.try_into().unwrap()) as u64;
        } else if n == 127 {
            n = u64::from_be_bytes(read_exact(&mut conn, 8)?.try_into().unwrap());
        }
        if n > 1 << 16 {
            anyhow::bail!("message too large");
        }
        let mask = if masked { read_exact(&mut conn, 4)? } else { vec![0; 4] };
        let mut data = read_exact(&mut conn, n as usize)?;
        for (i, b) in data.iter_mut().enumerate() {
            *b ^= mask[i % 4];
        }
        match op {
            0x8 => {
                // Close: a status code, then an optional reason.
                let code = data.get(..2).map(|c| u16::from_be_bytes([c[0], c[1]]));
                tracing::info!("the iPad closed the stream (code {code:?}, {:?})", String::from_utf8_lossy(data.get(2..).unwrap_or_default()));
                return Ok(());
            }
            0x1 if data == b"k" => hub.want_key.store(true, Relaxed),
            // Anything else the page says is for the log.
            0x1 => tracing::info!("iPad page: {}", String::from_utf8_lossy(&data)),
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn websocket_accept_key_from_rfc_6455() {
        assert_eq!(accept_key("dGhlIHNhbXBsZSBub25jZQ=="), "s3pPLMBiTxaQ9kYGzzhZRbK+xOo=");
    }

    #[test]
    fn only_the_secret_address_is_served() {
        let k = "abcd2345";
        assert_eq!(route("/abcd2345", k), Route::Page);
        assert_eq!(route("/abcd2345/", k), Route::Page);
        assert_eq!(route("/abcd2345?x=1", k), Route::Page);
        assert_eq!(route("/abcd2345/stream", k), Route::Stream);
        assert_eq!(route("/", k), Route::Unknown);
        assert_eq!(route("/stream", k), Route::Unknown);
        assert_eq!(route("/abcd2346/stream", k), Route::Unknown);
        assert_eq!(route("/abcd2345x", k), Route::Unknown);
        assert!(valid_key(k) && !valid_key("abc") && !valid_key("abcd234O"));
    }

    #[test]
    fn frame_lengths() {
        assert_eq!(&ws_frame(2, &[1, 2, 3])[..2], &[0x82, 3]);
        assert_eq!(&ws_frame(2, &vec![0; 300])[..4], &[0x82, 126, 1, 44]);
        assert_eq!(ws_frame(2, &vec![0; 70000])[1], 127);
    }
}
