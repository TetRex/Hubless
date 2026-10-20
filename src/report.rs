// SPDX-License-Identifier: GPL-3.0-or-later
//! Отчёт об устройствах для поддержки новых моделей (раздел окна настроек).
//!
//! Тот же опрос, что `hubless --dump` (только чтение, серийные номера маскируются), плюс шапка:
//! версии Hubless и Windows, запущен ли G Hub. Файл сохраняется в `%LOCALAPPDATA%\Hubless\reports`;
//! отправляет его сам пользователь — через форму issue на GitHub. Hubless в сеть не ходит.

use std::path::PathBuf;

use windows::Win32::Foundation::{CloseHandle, HANDLE};
use windows::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, OpenClipboard, SetClipboardData,
};
use windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS,
};
use windows::Win32::System::Memory::{GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalUnlock};

use crate::config::APP_DIR;

/// Форма нового issue с шаблоном отчёта об устройстве.
pub const ISSUE_URL: &str =
    "https://github.com/TetRex/Hubless/issues/new?template=device-report.yml";

/// Процессы G Hub: пока он запущен, устройства могут отвечать ему, а не нам.
const GHUB_PROCESSES: &[&str] = &["lghub.exe", "lghub_agent.exe", "lghub_system_tray.exe"];

/// Готовый отчёт: где сохранён и полный текст (для предпросмотра и буфера обмена).
#[derive(Debug, Clone)]
pub struct Report {
    pub path: PathBuf,
    pub text: String,
}

pub fn reports_dir() -> PathBuf {
    std::env::var_os("LOCALAPPDATA")
        .map_or_else(std::env::temp_dir, PathBuf::from)
        .join(APP_DIR)
        .join("reports")
}

/// Опросить устройства и сохранить отчёт (долго: до нескольких секунд — вызывать не на главном потоке).
pub fn create() -> Result<Report, String> {
    let dump = crate::dump::collect()?;
    let json = serde_json::to_string_pretty(&dump.json).map_err(|e| e.to_string())?;
    let text = format!(
        "Hubless {version} device report\n\
         Windows: {windows}\n\
         G Hub running: {ghub}\n\
         Generated (UTC): {time}\n\
         {dump}\n\
         ----- JSON -----\n\
         {json}\n",
        version = env!("CARGO_PKG_VERSION"),
        windows = windows_version(),
        ghub = if ghub_running() { "yes" } else { "no" },
        time = crate::timeutil::iso_utc(),
        dump = dump.text,
    );
    let dir = reports_dir();
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let stamp = crate::timeutil::iso_utc()
        .replace([':', '-'], "")
        .replace('T', "-")
        .trim_end_matches('Z')
        .to_owned();
    let path = dir.join(format!("hubless-report-{stamp}.txt"));
    std::fs::write(&path, &text).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(Report { path, text })
}

/// «Windows 11 Pro 24H2 (build 26100)» из реестра.
fn windows_version() -> String {
    const KEY: &str = r"SOFTWARE\Microsoft\Windows NT\CurrentVersion";
    let read = |v: &str| crate::win::hklm_read_string(KEY, v).unwrap_or_default();
    let build = read("CurrentBuild");
    let mut product = read("ProductName");
    // ProductName у Windows 11 до сих пор «Windows 10 …» — правим по номеру сборки.
    if build.parse::<u32>().is_ok_and(|b| b >= 22000) {
        product = product.replacen("Windows 10", "Windows 11", 1);
    }
    format!("{product} {} (build {build})", read("DisplayVersion"))
}

/// Запущен ли G Hub.
pub fn ghub_running() -> bool {
    // SAFETY: снимок процессов закрывается ниже; структура записи — локальная, с размером.
    unsafe {
        let Ok(snapshot) = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) else {
            return false;
        };
        let mut entry = PROCESSENTRY32W {
            dwSize: size_of::<PROCESSENTRY32W>() as u32,
            ..Default::default()
        };
        let mut found = false;
        let mut ok = Process32FirstW(snapshot, &mut entry).is_ok();
        while ok && !found {
            let len = entry
                .szExeFile
                .iter()
                .position(|&c| c == 0)
                .unwrap_or(entry.szExeFile.len());
            let name = String::from_utf16_lossy(&entry.szExeFile[..len]).to_lowercase();
            found = GHUB_PROCESSES.contains(&name.as_str());
            ok = Process32NextW(snapshot, &mut entry).is_ok();
        }
        let _ = CloseHandle(snapshot);
        found
    }
}

/// Текст в буфер обмена (CF_UNICODETEXT).
pub fn copy_to_clipboard(text: &str) -> bool {
    const CF_UNICODETEXT: u32 = 13;
    let wide: Vec<u16> = text.encode_utf16().chain(Some(0)).collect();
    // SAFETY: память выделяется под всю строку с нулём и после SetClipboardData принадлежит
    // системе; буфер обмена закрывается в любом случае.
    unsafe {
        if OpenClipboard(None).is_err() {
            return false;
        }
        let ok = (|| {
            EmptyClipboard().ok()?;
            let mem = GlobalAlloc(GMEM_MOVEABLE, wide.len() * 2).ok()?;
            let dst = GlobalLock(mem).cast::<u16>();
            if dst.is_null() {
                return None;
            }
            std::ptr::copy_nonoverlapping(wide.as_ptr(), dst, wide.len());
            let _ = GlobalUnlock(mem);
            SetClipboardData(CF_UNICODETEXT, Some(HANDLE(mem.0))).ok()
        })()
        .is_some();
        let _ = CloseClipboard();
        ok
    }
}
