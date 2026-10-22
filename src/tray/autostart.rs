// SPDX-License-Identifier: GPL-3.0-or-later
//! Start with Windows through `HKCU\Software\Microsoft\Windows\CurrentVersion\Run`.

use crate::win::{hkcu_delete_value, hkcu_read_string, hkcu_write_string};

const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
const VALUE: &str = "Hubless";

fn command() -> Option<String> {
    let exe = std::env::current_exe().ok()?;
    Some(format!("\"{}\"", exe.display()))
}

/// On only if the entry points to the current exe.
pub fn is_enabled() -> bool {
    match (hkcu_read_string(RUN_KEY, VALUE), command()) {
        (Some(current), Some(ours)) => current.eq_ignore_ascii_case(&ours),
        _ => false,
    }
}

/// The program moved (installation): if start with Windows is on for any path, point it to `exe`.
pub fn point_to(exe: &std::path::Path) {
    if hkcu_read_string(RUN_KEY, VALUE).is_some()
        && let Err(e) = hkcu_write_string(RUN_KEY, VALUE, &format!("\"{}\"", exe.display()))
    {
        tracing::warn!("автозапуск: {e}");
    }
}

/// Turns start with Windows off, whatever path it points to (uninstalling).
pub fn remove() {
    if let Err(e) = hkcu_delete_value(RUN_KEY, VALUE) {
        tracing::warn!("автозапуск: {e}");
    }
}

pub fn set_enabled(enable: bool) -> std::io::Result<()> {
    if enable {
        let cmd =
            command().ok_or_else(|| std::io::Error::other("could not determine the exe path"))?;
        hkcu_write_string(RUN_KEY, VALUE, &cmd)?;
    } else {
        hkcu_delete_value(RUN_KEY, VALUE)?;
    }
    Ok(())
}
