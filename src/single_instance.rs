// SPDX-License-Identifier: GPL-3.0-or-later
//! Single-instance через именованный mutex.

use windows::Win32::Foundation::{CloseHandle, ERROR_ALREADY_EXISTS, GetLastError, HANDLE};
use windows::Win32::System::Threading::CreateMutexW;
use windows::core::w;

pub struct InstanceGuard(HANDLE);

impl Drop for InstanceGuard {
    fn drop(&mut self) {
        // SAFETY: handle получен из CreateMutexW и закрывается один раз.
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}

/// `Some` — мы единственный экземпляр (держать guard до выхода), `None` — уже запущен другой.
pub fn acquire() -> Option<InstanceGuard> {
    // SAFETY: обычный вызов Win32 с константным именем.
    unsafe {
        match CreateMutexW(None, true, w!("Local\\Hubless.SingleInstance")) {
            Ok(handle) if GetLastError() == ERROR_ALREADY_EXISTS => {
                let _ = CloseHandle(handle);
                None
            }
            Ok(handle) => Some(InstanceGuard(handle)),
            // Не смогли создать mutex — не блокируем запуск.
            Err(e) => {
                tracing::warn!("CreateMutexW: {e}");
                Some(InstanceGuard(HANDLE::default()))
            }
        }
    }
}
