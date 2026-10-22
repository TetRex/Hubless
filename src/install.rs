// SPDX-License-Identifier: GPL-3.0-or-later
//! Per-user installation without an installer.
//!
//! The downloaded exe copies itself to `%LOCALAPPDATA%\Programs\Hubless`, adds a Start menu shortcut
//! and an entry in Settings → Apps, and starts the installed copy, so the downloaded file can be
//! deleted. Running a newer download replaces the installed copy (the running one is asked to quit
//! first, so the keyboard gets its lighting back). Everything is per user (HKCU), no admin rights.
//! Builds inside a Cargo `target` directory and `HUBLESS_PORTABLE=1` run in place.

use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};
use std::time::{Duration, Instant};

use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx, IPersistFile,
};
use windows::Win32::UI::Shell::{IShellLinkW, ShellLink};
use windows::Win32::UI::WindowsAndMessaging::{MB_ICONINFORMATION, MB_OK, MessageBoxW};
use windows::core::{HSTRING, Interface, w};

use crate::hidden_window;
use crate::i18n::tr;
use crate::tray::autostart;
use crate::win::{hkcu_delete_tree, hkcu_write_dword, hkcu_write_string};

const UNINSTALL_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Uninstall\Hubless";
const EXE_NAME: &str = "hubless.exe";
const HOMEPAGE: &str = "https://github.com/TetRex/Hubless";
/// How long to wait for a running Hubless to exit before replacing or removing it.
const QUIT_TIMEOUT: Duration = Duration::from_secs(10);
/// How long to retry copying while the old exe is still locked by its exiting process.
const COPY_TIMEOUT: Duration = Duration::from_secs(6);
const POLL: Duration = Duration::from_millis(100);

/// `%LOCALAPPDATA%\Programs\Hubless`: where Hubless lives once installed.
pub fn install_dir() -> PathBuf {
    std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join("Programs")
        .join("Hubless")
}

fn installed_exe() -> PathBuf {
    install_dir().join(EXE_NAME)
}

fn shortcut_path() -> Option<PathBuf> {
    let appdata = std::env::var_os("APPDATA")?;
    Some(PathBuf::from(appdata).join(r"Microsoft\Windows\Start Menu\Programs\Hubless.lnk"))
}

fn same_path(a: &Path, b: &Path) -> bool {
    a.to_string_lossy()
        .eq_ignore_ascii_case(&b.to_string_lossy())
}

/// A build inside a Cargo `target` directory (Cargo keeps `.rustc_info.json` at its root), or
/// `HUBLESS_PORTABLE` is set: run in place, don't install.
fn runs_in_place(exe: &Path) -> bool {
    std::env::var_os("HUBLESS_PORTABLE").is_some()
        || exe
            .ancestors()
            .skip(1)
            .take(3)
            .any(|dir| dir.join(".rustc_info.json").exists())
}

/// Started from a downloaded file: install this exe (replacing the installed copy) and start the
/// installed copy. `Some` means this process is done; `None` means keep running here (this is the
/// installed copy or a dev build, or the installation failed).
pub fn from_download() -> Option<ExitCode> {
    let exe = std::env::current_exe().ok()?;
    let target = installed_exe();
    if same_path(&exe, &target) || runs_in_place(&exe) {
        return None;
    }
    tracing::info!(from = %exe.display(), to = %target.display(), "установка");
    if let Err(e) = install(&exe, &target) {
        tracing::error!("установка: {e}; работаю из скачанного файла");
        return None;
    }
    match Command::new(&target).arg("--installed").spawn() {
        Ok(_) => Some(ExitCode::SUCCESS),
        Err(e) => {
            tracing::error!("запуск установленной копии: {e}");
            None
        }
    }
}

fn install(exe: &Path, target: &Path) -> Result<(), String> {
    // The same file is already installed: nothing to copy (the installed copy then just opens
    // the panel of the running instance, or starts).
    let unchanged = std::fs::read(target)
        .ok()
        .is_some_and(|old| std::fs::read(exe).is_ok_and(|new| new == old));
    if !unchanged {
        stop_running()?;
        std::fs::create_dir_all(install_dir())
            .map_err(|e| format!("{}: {e}", install_dir().display()))?;
        copy_with_retry(exe, target)?;
    }
    if let Err(e) = create_shortcut(target) {
        tracing::warn!("ярлык в меню «Пуск»: {e}");
    }
    if let Err(e) = register(target) {
        tracing::warn!("запись в «Приложениях»: {e}");
    }
    autostart::point_to(target);
    Ok(())
}

/// Asks a running Hubless to quit and waits until it is gone.
fn stop_running() -> Result<(), String> {
    if !hidden_window::request_quit() {
        return Ok(());
    }
    let deadline = Instant::now() + QUIT_TIMEOUT;
    while hidden_window::is_running() {
        if Instant::now() > deadline {
            return Err("the running Hubless didn't quit".into());
        }
        std::thread::sleep(POLL);
    }
    Ok(())
}

/// Copies the exe; the old file may stay locked for a moment after its process has exited.
fn copy_with_retry(from: &Path, to: &Path) -> Result<(), String> {
    let deadline = Instant::now() + COPY_TIMEOUT;
    loop {
        match std::fs::copy(from, to) {
            Ok(_) => return Ok(()),
            Err(e) if Instant::now() > deadline => {
                return Err(format!("{}: {e}", to.display()));
            }
            Err(_) => std::thread::sleep(POLL),
        }
    }
}

/// `Hubless.lnk` in the user's Start menu.
fn create_shortcut(exe: &Path) -> windows::core::Result<()> {
    let Some(path) = shortcut_path() else {
        return Ok(());
    };
    let dir = exe.parent().unwrap_or(exe);
    // SAFETY: COM is initialized on this thread before use (S_FALSE if it already was); all
    // strings are HSTRINGs that live until the end of the calls.
    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        let link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER)?;
        link.SetPath(&HSTRING::from(exe.as_os_str()))?;
        link.SetWorkingDirectory(&HSTRING::from(dir.as_os_str()))?;
        link.SetIconLocation(&HSTRING::from(exe.as_os_str()), 0)?;
        link.SetDescription(w!("Hubless: a lightweight G Hub replacement"))?;
        let file: IPersistFile = link.cast()?;
        file.Save(&HSTRING::from(path.as_os_str()), true)?;
    }
    Ok(())
}

/// The entry in Settings → Apps (and "Programs and Features"), with "Uninstall" running
/// `hubless.exe --uninstall`.
fn register(exe: &Path) -> windows::core::Result<()> {
    let exe_text = exe.display().to_string();
    let uninstall = format!("\"{exe_text}\" --uninstall");
    let strings = [
        ("DisplayName", "Hubless".to_owned()),
        ("DisplayVersion", env!("CARGO_PKG_VERSION").to_owned()),
        ("Publisher", "TetRex".to_owned()),
        ("DisplayIcon", format!("{exe_text},0")),
        ("InstallLocation", install_dir().display().to_string()),
        ("UninstallString", uninstall.clone()),
        ("QuietUninstallString", uninstall),
        ("URLInfoAbout", HOMEPAGE.to_owned()),
    ];
    for (name, value) in strings {
        hkcu_write_string(UNINSTALL_KEY, name, &value)?;
    }
    hkcu_write_dword(UNINSTALL_KEY, "NoModify", 1)?;
    hkcu_write_dword(UNINSTALL_KEY, "NoRepair", 1)?;
    let size_kb = std::fs::metadata(exe).map_or(0, |m| m.len() / 1024);
    hkcu_write_dword(UNINSTALL_KEY, "EstimatedSize", size_kb as u32)?;
    Ok(())
}

/// `hubless --uninstall` (Settings → Apps → Uninstall): quit the running Hubless, remove start
/// with Windows, the Start menu shortcut, the Apps entry and the program folder. The settings in
/// `%APPDATA%\Hubless` and the logs are kept.
pub fn uninstall() -> ExitCode {
    tracing::info!("удаление");
    if let Err(e) = stop_running() {
        tracing::warn!("удаление: {e}");
    }
    autostart::remove();
    if let Some(path) = shortcut_path() {
        let _ = std::fs::remove_file(path);
    }
    if let Err(e) = hkcu_delete_tree(UNINSTALL_KEY) {
        tracing::warn!("запись в «Приложениях»: {e}");
    }
    let text = tr("Hubless has been removed. Your settings are kept in %APPDATA%\\Hubless.");
    // SAFETY: the strings live until the end of the call; no owner window.
    unsafe {
        MessageBoxW(
            None,
            &HSTRING::from(text),
            w!("Hubless"),
            MB_OK | MB_ICONINFORMATION,
        );
    }
    remove_program_folder();
    ExitCode::SUCCESS
}

/// The program folder can't be deleted while this exe runs from it: a detached `cmd` deletes it a
/// couple of seconds after this process exits.
fn remove_program_folder() {
    let dir = install_dir();
    let inside = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|p| same_path(p, &dir)))
        .unwrap_or(false);
    if !inside {
        let _ = std::fs::remove_dir_all(&dir);
        return;
    }
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let spawned = Command::new("cmd.exe")
        .raw_arg(format!(
            "/C ping 127.0.0.1 -n 3 >nul & rmdir /S /Q \"{}\"",
            dir.display()
        ))
        .creation_flags(CREATE_NO_WINDOW)
        .spawn();
    if let Err(e) = spawned {
        tracing::warn!("удаление папки программы: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dev_builds_run_in_place() {
        let dir = std::env::temp_dir().join(format!("hubless-install-test-{}", std::process::id()));
        let release = dir.join("target").join("release");
        std::fs::create_dir_all(&release).unwrap();
        let exe = release.join(EXE_NAME);
        assert!(!runs_in_place(&exe) || std::env::var_os("HUBLESS_PORTABLE").is_some());
        std::fs::write(dir.join("target").join(".rustc_info.json"), "{}").unwrap();
        assert!(runs_in_place(&exe), "a Cargo target directory");
        std::fs::remove_dir_all(&dir).unwrap();
        assert!(same_path(
            Path::new(r"C:\Users\A\AppData\Local\Programs\Hubless\hubless.exe"),
            Path::new(r"c:\users\a\appdata\local\programs\hubless\HUBLESS.EXE")
        ));
    }
}
