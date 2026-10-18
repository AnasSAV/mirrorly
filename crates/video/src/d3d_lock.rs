//! One recursive lock around the D3D11 immediate context, shared by FFmpeg's
//! d3d11va hwaccel (through `AVD3D11VADeviceContext.lock/unlock`) and the
//! present thread. FFmpeg requires a recursive lock; a Win32 critical
//! section is one.

use std::{cell::UnsafeCell, ffi::c_void};

use windows::Win32::System::Threading::{
    CRITICAL_SECTION, DeleteCriticalSection, EnterCriticalSection, InitializeCriticalSection,
    LeaveCriticalSection,
};

pub struct D3dLock {
    // Boxed so the address handed to FFmpeg never moves.
    cs: Box<UnsafeCell<CRITICAL_SECTION>>,
}

// SAFETY: a critical section is designed to be used from many threads; we
// only ever touch it through the Win32 API.
unsafe impl Send for D3dLock {}
unsafe impl Sync for D3dLock {}

impl Default for D3dLock {
    fn default() -> Self {
        let cs = Box::new(UnsafeCell::new(CRITICAL_SECTION::default()));
        unsafe { InitializeCriticalSection(cs.get()) };
        Self { cs }
    }
}

impl D3dLock {
    pub fn lock(&self) -> D3dGuard<'_> {
        unsafe { EnterCriticalSection(self.cs.get()) };
        D3dGuard(self)
    }

    /// Context pointer for the FFmpeg callbacks below.
    pub(crate) fn raw(&self) -> *mut c_void {
        self.cs.get().cast()
    }
}

impl Drop for D3dLock {
    fn drop(&mut self) {
        unsafe { DeleteCriticalSection(self.cs.get()) };
    }
}

pub struct D3dGuard<'a>(&'a D3dLock);

impl Drop for D3dGuard<'_> {
    fn drop(&mut self) {
        unsafe { LeaveCriticalSection(self.0.cs.get()) };
    }
}

pub(crate) unsafe extern "C" fn ffmpeg_lock(ctx: *mut c_void) {
    unsafe { EnterCriticalSection(ctx.cast()) };
}

pub(crate) unsafe extern "C" fn ffmpeg_unlock(ctx: *mut c_void) {
    unsafe { LeaveCriticalSection(ctx.cast()) };
}
