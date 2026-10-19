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
