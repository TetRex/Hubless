// SPDX-License-Identifier: GPL-3.0-or-later
//! Logs in `%LOCALAPPDATA%\Hubless\logs` with daily rotation.

use tracing_appender::rolling::{RollingFileAppender, Rotation};
use tracing_subscriber::EnvFilter;

use crate::config::{Config, log_dir};

const KEEP_FILES: usize = 7;

/// The environment variable overrides the filter from the config (`EnvFilter` syntax).
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
        // No access to the log folder: write to stderr (visible only when started from a console).
        Err(_) => builder.with_writer(std::io::stderr).init(),
    }

    std::panic::set_hook(Box::new(|info| {
        tracing::error!("panic: {info}");
    }));
}

/// Deletes the logs (a button in the settings): all files in the log folder except the newest one, which
/// this process is writing to right now (a deleted file would swallow new entries until rotation), so it is truncated.
/// Returns the number of deleted and truncated files.
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
