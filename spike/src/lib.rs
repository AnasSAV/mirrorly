//! Shared plumbing for the Phase 0 spike binaries. Throwaway code: correctness
//! and measurement over structure.

pub mod conn;
pub mod ddi;
pub mod depack;
pub mod feedback;
pub mod media;
pub mod watch;

use std::time::Instant;

/// Process-wide monotonic epoch. `Instant` is backed by QueryPerformanceCounter
/// on Windows, so every timing below is QPC-derived.
pub fn epoch() -> Instant {
    static EPOCH: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
    *EPOCH.get_or_init(Instant::now)
}

/// Microseconds since [`epoch`].
pub fn now_us() -> u64 {
    epoch().elapsed().as_micros() as u64
}

#[cfg(windows)]
#[link(name = "winmm")]
unsafe extern "system" {
    fn timeBeginPeriod(period_ms: u32) -> u32;
}

/// Raise the system timer resolution to 1 ms for this process. Without it,
/// sleeps and tokio timers on Windows round up to ~15.6 ms.
pub fn hires_timer() {
    #[cfg(windows)]
    unsafe {
        timeBeginPeriod(1);
    }
}

pub fn init_logging() {
    let _ = epoch();
    hires_timer();
    let _ = tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "warn".into()),
        )
        .with_writer(std::io::stderr)
        .try_init();
}
