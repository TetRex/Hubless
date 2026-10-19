// SPDX-License-Identifier: GPL-3.0-or-later
//! Уведомления через «balloon» иконки в трее (`Shell_NotifyIcon` + `NIF_INFO`).
//!
//! Windows 10/11 показывает их как обычные toast-уведомления от имени приложения и кладёт
//! в центр уведомлений. В отличие от WinRT-toast, не требуется ярлык в меню «Пуск» с AUMID.

use std::cell::Cell;

use windows::Win32::Foundation::HWND;
use windows::Win32::UI::Shell::{
    NIF_INFO, NIIF_WARNING, NIM_MODIFY, NOTIFYICONDATAW, Shell_NotifyIconW,
};

/// tray-icon не отдаёт `uID` своей иконки; он берётся из счётчика, начинающегося с 1.
/// Перебираем первые несколько значений и запоминаем подошедшее.
const MAX_ID: u32 = 8;

thread_local! {
    static ICON_ID: Cell<Option<u32>> = const { Cell::new(None) };
}

fn copy_wide(dst: &mut [u16], s: &str) {
    let mut n = 0;
    for (d, c) in dst.iter_mut().zip(s.encode_utf16()) {
        *d = c;
        n += 1;
    }
    // Последний элемент — завершающий ноль.
    let end = n.min(dst.len() - 1);
    dst[end] = 0;
}

pub fn show(hwnd: HWND, title: &str, text: &str) {
    let mut nid = NOTIFYICONDATAW {
        cbSize: size_of::<NOTIFYICONDATAW>() as u32,
        hWnd: hwnd,
        uFlags: NIF_INFO,
        dwInfoFlags: NIIF_WARNING,
        ..Default::default()
    };
    copy_wide(&mut nid.szInfoTitle, title);
    copy_wide(&mut nid.szInfo, text);

    let known = ICON_ID.get();
    let candidates = known
        .into_iter()
        .chain((1..=MAX_ID).filter(|id| Some(*id) != known));
    for id in candidates {
        nid.uID = id;
        // SAFETY: структура заполнена и живёт до конца вызова; hwnd — окно tray-icon этого процесса.
        if unsafe { Shell_NotifyIconW(NIM_MODIFY, &nid) }.as_bool() {
            ICON_ID.set(Some(id));
            return;
        }
    }
    tracing::warn!("уведомление не показано: иконка трея не найдена");
}

#[cfg(test)]
mod tests {
    #[test]
    fn copy_wide_truncates_with_nul() {
        let mut buf = [0xFFFFu16; 4];
        super::copy_wide(&mut buf, "abcdef");
        assert_eq!(buf, [b'a' as u16, b'b' as u16, b'c' as u16, 0]);
        super::copy_wide(&mut buf, "x");
        assert_eq!(&buf[..2], &[b'x' as u16, 0]);
    }
}
