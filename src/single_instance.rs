// SPDX-License-Identifier: GPL-3.0-or-later
//! Single instance through a named mutex.

use windows::Win32::Foundation::{CloseHandle, ERROR_ALREADY_EXISTS, GetLastError, HANDLE};
use windows::Win32::System::Threading::CreateMutexW;
use windows::core::w;

pub struct InstanceGuard(HANDLE);

impl Drop for InstanceGuard {
    fn drop(&mut self) {
        // SAFETY: the handle came from CreateMutexW and is closed once.
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}

/// `Some`: we are the only instance (keep the guard until exit); `None`: another one is already running.
pub fn acquire() -> Option<InstanceGuard> {
    // SAFETY: a plain Win32 call with a constant name.
    unsafe {
        match CreateMutexW(None, true, w!("Local\\Hubless.SingleInstance")) {
            Ok(handle) if GetLastError() == ERROR_ALREADY_EXISTS => {
                let _ = CloseHandle(handle);
                None
            }
            Ok(handle) => Some(InstanceGuard(handle)),
            // Couldn't create the mutex: don't block the startup.
            Err(e) => {
                tracing::warn!("CreateMutexW: {e}");
                Some(InstanceGuard(HANDLE::default()))
            }
        }
    }
}
