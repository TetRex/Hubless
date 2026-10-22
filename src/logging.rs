// SPDX-License-Identifier: GPL-3.0-or-later
//! Логи в `%LOCALAPPDATA%\Hubless\logs` с ежедневной ротацией.

use tracing_appender::rolling::{RollingFileAppender, Rotation};
use tracing_subscriber::EnvFilter;

use crate::config::{Config, log_dir};

const KEEP_FILES: usize = 7;

/// Переменная окружения переопределяет фильтр из конфига (синтаксис `EnvFilter`).
const ENV_FILTER: &str = "HUBLESS_LOG";

pub fn init(cfg: &Config) {
    let mut directives = cfg.log_level.clone();
    if cfg.hid_trace {
        directives.push_str(",hubless::hidpp=debug");
    }
    let filter = EnvFilter::try_from_env(ENV_FILTER)
        .or_else(|_| EnvFilter::try_new(&directives))
        .unwrap_or_else(|_| EnvFilter::new("info"));

    let appender = RollingFileAppender::builder()
        .rotation(Rotation::DAILY)
        .filename_prefix("hubless")
        .filename_suffix("log")
        .max_log_files(KEEP_FILES)
        .build(log_dir());

    let builder = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_ansi(false)
        .with_target(true);
    match appender {
        Ok(appender) => builder.with_writer(appender).init(),
        // Нет доступа к папке логов — пишем в stderr (виден только при запуске из консоли).
        Err(_) => builder.with_writer(std::io::stderr).init(),
    }

    std::panic::set_hook(Box::new(|info| {
        tracing::error!("panic: {info}");
    }));
}

/// Удалить логи (кнопка в настройках): все файлы папки логов, кроме самого свежего — в него
/// сейчас пишет этот процесс, и удалённый файл до ротации глотал бы новые записи, — его обнулить.
/// Возвращает число удалённых и обнулённых файлов.
pub fn clear() -> Result<usize, String> {
    let dir = log_dir();
    let entries = match std::fs::read_dir(&dir) {
        Ok(e) => e,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        Err(e) => return Err(format!("{}: {e}", dir.display())),
    };
    let mut files: Vec<(std::path::PathBuf, std::time::SystemTime)> = entries
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_ok_and(|t| t.is_file()))
        .map(|e| {
            let modified = e
                .metadata()
                .and_then(|m| m.modified())
                .unwrap_or(std::time::UNIX_EPOCH);
            (e.path(), modified)
        })
        .collect();
    files.sort_by_key(|(_, modified)| *modified);
    let current = files.pop();
    let mut done = 0;
    for (path, _) in files {
        match std::fs::remove_file(&path) {
            Ok(()) => done += 1,
            Err(e) => tracing::warn!("лог {}: {e}", path.display()),
        }
    }
    if let Some((path, _)) = current {
        let truncated = std::fs::OpenOptions::new()
            .write(true)
            .open(&path)
            .and_then(|f| f.set_len(0));
        match truncated {
            Ok(()) => done += 1,
            Err(e) => tracing::warn!("лог {}: {e}", path.display()),
        }
    }
    tracing::info!(files = done, "логи удалены");
    Ok(done)
}
