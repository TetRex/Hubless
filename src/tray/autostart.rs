// SPDX-License-Identifier: GPL-3.0-or-later
//! Автозапуск через `HKCU\Software\Microsoft\Windows\CurrentVersion\Run`.

use crate::win::{hkcu_delete_value, hkcu_read_string, hkcu_write_string};

const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
const VALUE: &str = "ProTray";

fn command() -> Option<String> {
    let exe = std::env::current_exe().ok()?;
    Some(format!("\"{}\"", exe.display()))
}

/// Включён, только если запись указывает на текущий exe.
pub fn is_enabled() -> bool {
    match (hkcu_read_string(RUN_KEY, VALUE), command()) {
        (Some(current), Some(ours)) => current.eq_ignore_ascii_case(&ours),
        _ => false,
    }
}

pub fn set_enabled(enable: bool) -> std::io::Result<()> {
    if enable {
        let cmd =
            command().ok_or_else(|| std::io::Error::other("не удалось определить путь exe"))?;
        hkcu_write_string(RUN_KEY, VALUE, &cmd)?;
    } else {
        hkcu_delete_value(RUN_KEY, VALUE)?;
    }
    Ok(())
}
