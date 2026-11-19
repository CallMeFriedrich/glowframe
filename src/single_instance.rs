//! Single instance via a named mutex (SPEC §4). A second start signals the
//! running instance through a named event to open its settings and exits.

use windows::Win32::Foundation::{
    CloseHandle, ERROR_ALREADY_EXISTS, GetLastError, HANDLE, WAIT_OBJECT_0,
};
use windows::Win32::System::Threading::{
    CreateEventW, CreateMutexW, EVENT_MODIFY_STATE, INFINITE, OpenEventW, SetEvent,
    WaitForSingleObject,
};
use windows::core::w;

pub enum Instance {
    /// We are the first instance. Keep this alive for the whole runtime.
    Primary(PrimaryGuard),
    /// Another instance is already running.
    Secondary,
}

pub struct PrimaryGuard {
    mutex: HANDLE,
    event: HANDLE,
}

// SAFETY: the handles are kernel objects that may be used from any thread.
unsafe impl Send for PrimaryGuard {}
unsafe impl Sync for PrimaryGuard {}

impl Drop for PrimaryGuard {
    fn drop(&mut self) {
        // SAFETY: both handles were created by us and are closed exactly once.
        unsafe {
            let _ = CloseHandle(self.event);
            let _ = CloseHandle(self.mutex);
        }
    }
}

/// Acquires the instance mutex. On failure to create kernel objects we run as
/// primary without single-instance protection rather than refusing to start.
pub fn acquire() -> Option<Instance> {
    // SAFETY: plain Win32 calls with static, null-terminated names.
    unsafe {
        let mutex = match CreateMutexW(None, false, w!("Local\\Glowframe.Instance")) {
            Ok(h) => h,
            Err(e) => {
                tracing::error!(error = %e, "CreateMutexW failed");
                return None;
            }
        };
        if GetLastError() == ERROR_ALREADY_EXISTS {
            let _ = CloseHandle(mutex);
            return Some(Instance::Secondary);
        }
        // Auto-reset event: one wake-up per SetEvent.
        match CreateEventW(None, false, false, w!("Local\\Glowframe.ShowSettings")) {
            Ok(event) => Some(Instance::Primary(PrimaryGuard { mutex, event })),
            Err(e) => {
                tracing::error!(error = %e, "CreateEventW failed");
                let _ = CloseHandle(mutex);
                None
            }
        }
    }
}

/// Called by a secondary instance: asks the primary to show its settings.
pub fn signal_primary() -> bool {
    // SAFETY: the handle is closed right after use.
    unsafe {
        match OpenEventW(
            EVENT_MODIFY_STATE,
            false,
            w!("Local\\Glowframe.ShowSettings"),
        ) {
            Ok(event) => {
                let ok = SetEvent(event).is_ok();
                let _ = CloseHandle(event);
                ok
            }
            Err(e) => {
                tracing::warn!(error = %e, "could not open ShowSettings event");
                false
            }
        }
    }
}

impl PrimaryGuard {
    /// Spawns a thread that calls `on_signal` whenever a second instance starts.
    pub fn listen(&self, on_signal: impl Fn() + Send + 'static) {
        let event = self.event.0 as usize;
        let spawned = std::thread::Builder::new()
            .name("instance-signal".into())
            .spawn(move || {
                let event = HANDLE(event as *mut _);
                loop {
                    // SAFETY: the guard (and thus the handle) lives until process exit.
                    let r = unsafe { WaitForSingleObject(event, INFINITE) };
                    if r != WAIT_OBJECT_0 {
                        tracing::warn!(?r, "instance signal wait ended");
                        break;
                    }
                    on_signal();
                }
            });
        if let Err(e) = spawned {
            tracing::error!(error = %e, "could not spawn instance-signal thread");
        }
    }
}
