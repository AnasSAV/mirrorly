//! The iPad side is a web page in Safari: this serves it, streams H.264 to
//! it over a WebSocket (one access unit per binary message). The page sends
//! "k" for a keyframe (after a decoder reset) and log lines.
//!
//! Latency first: each viewer has a short queue. When Wi-Fi can't keep up,
//! frames are skipped until the next keyframe (which is requested) instead
//! of queueing, so delay never builds up.

use std::{
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering::Relaxed},
        mpsc::{Receiver, SyncSender, TrySendError, sync_channel},
    },
    thread,
};

use anyhow::{Context, Result};

/// Frames a viewer may have queued before it is considered behind.
const QUEUE: usize = 3;

const PAGE: &str = include_str!("viewer.html");

struct Viewer {
    tx: SyncSender<Arc<Vec<u8>>>,
    /// To hang up when the display stops.
    conn: TcpStream,
    /// Behind: skip frames until a keyframe.
    waiting_key: bool,
}

#[derive(Clone)]
pub struct Hub {
    viewers: Arc<Mutex<Vec<Viewer>>>,
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
            match v.tx.try_send(msg.clone()) {
                Ok(()) => {
                    v.waiting_key = false;
                    true
                }
                Err(TrySendError::Full(_)) => {
                    v.waiting_key = true;
                    self.want_key.store(true, Relaxed);
                    true
                }
                Err(TrySendError::Disconnected(_)) => false,
            }
        });
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

fn base64(data: &[u8]) -> String {
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

/// Listen on `port` (all interfaces: the iPad comes over Wi-Fi).
pub fn start(port: u16, stop: Arc<AtomicBool>) -> Result<Hub> {
    let listener = TcpListener::bind(("0.0.0.0", port)).with_context(|| format!("listen on port {port}"))?;
    listener.set_nonblocking(true)?;
    let hub = Hub { viewers: Arc::default(), want_key: Arc::new(AtomicBool::new(false)) };
    let h = hub.clone();
    thread::Builder::new().name("display-server".into()).spawn(move || {
        while !stop.load(Relaxed) {
            match listener.accept() {
                Ok((conn, peer)) => {
                    let h = h.clone();
                    let _ = thread::Builder::new().name("display-conn".into()).spawn(move || {
                        if let Err(e) = handle(conn, &h) {
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

fn handle(mut conn: TcpStream, hub: &Hub) -> Result<()> {
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

    match (path, key) {
        ("/stream", Some(key)) => {
            let head = format!(
                "HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: {}\r\n\r\n",
                accept_key(&key)
            );
            conn.write_all(head.as_bytes())?;
            viewer(conn, hub)
        }
        _ => {
            let body = PAGE.as_bytes();
            let head = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nCache-Control: no-store\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            conn.write_all(head.as_bytes())?;
            conn.write_all(body)?;
            Ok(())
        }
    }
}

fn viewer(conn: TcpStream, hub: &Hub) -> Result<()> {
    let (tx, rx): (SyncSender<Arc<Vec<u8>>>, Receiver<Arc<Vec<u8>>>) = sync_channel(QUEUE);
    hub.viewers.lock().unwrap().push(Viewer { tx, conn: conn.try_clone()?, waiting_key: true });
    hub.want_key.store(true, Relaxed);
    tracing::info!("iPad viewer connected ({})", conn.peer_addr().map(|a| a.to_string()).unwrap_or_default());

    // Writer: frames out, in order.
    let mut out = conn.try_clone()?;
    let writer = thread::Builder::new().name("display-send".into()).spawn(move || {
        while let Ok(msg) = rx.recv() {
            if out.write_all(&msg).is_err() {
                return;
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
    fn frame_lengths() {
        assert_eq!(&ws_frame(2, &[1, 2, 3])[..2], &[0x82, 3]);
        assert_eq!(&ws_frame(2, &vec![0; 300])[..4], &[0x82, 126, 1, 44]);
        assert_eq!(ws_frame(2, &vec![0; 70000])[1], 127);
    }
}
