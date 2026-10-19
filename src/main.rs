// SPDX-License-Identifier: GPL-3.0-or-later
// Hubless — лёгкая замена G Hub для Windows.
// Copyright (C) 2026 TetRex

#![windows_subsystem = "windows"]

mod app;
mod centurion;
mod cli;
mod config;
mod devices;
mod dump;
mod hidden_window;
mod hidpp;
mod logging;
mod panel;
mod settings;
mod single_instance;
mod state;
mod theme;
mod timeutil;
mod tray;
mod win;

mod ui {
    slint::include_modules!();
}

use std::process::ExitCode;

use windows::Win32::System::Console::{ATTACH_PARENT_PROCESS, AttachConsole};

const USAGE: &str = "\
Hubless — лёгкая замена G Hub

Использование:
  hubless                   запустить в трее
  hubless --dump [файл]     устройства и таблицы HID++ фич (по умолчанию hubless-dump.json)
  hubless --test-toast      показать тестовое уведомление (Hubless должен быть запущен)
  hubless --help            эта справка
  hubless --version         версия

Мышь:";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        None => run_tray(),
        Some("--dump") => {
            attach_console();
            let (cfg, _) = config::Config::load();
            logging::init(&cfg);
            dump::run(args.get(1).map(Into::into))
        }
        Some("mouse") => {
            attach_console();
            let (cfg, _) = config::Config::load();
            logging::init(&cfg);
            cli::mouse(&args[1..])
        }
        Some("--test-toast") => {
            attach_console();
            if hidden_window::request_test_notification() {
                println!("Запрос отправлен запущенному Hubless.");
                ExitCode::SUCCESS
            } else {
                eprintln!("Hubless не запущен: уведомления показывает приложение в трее.");
                ExitCode::FAILURE
            }
        }
        Some("--help" | "-h") => {
            attach_console();
            println!("{USAGE}\n{}", cli::MOUSE_USAGE);
            ExitCode::SUCCESS
        }
        Some("--version" | "-V") => {
            attach_console();
            println!("hubless {}", env!("CARGO_PKG_VERSION"));
            ExitCode::SUCCESS
        }
        Some(other) => {
            attach_console();
            eprintln!(
                "Неизвестный аргумент: {other}\n\n{USAGE}\n{}",
                cli::MOUSE_USAGE
            );
            ExitCode::from(2)
        }
    }
}

/// GUI-подсистема не получает консоль; для CLI подключаемся к консоли родителя.
fn attach_console() {
    // SAFETY: без побочных эффектов, если консоли у родителя нет.
    let _ = unsafe { AttachConsole(ATTACH_PARENT_PROCESS) };
}

fn run_tray() -> ExitCode {
    let (cfg, cfg_err) = config::Config::load();
    logging::init(&cfg);
    if let Some(e) = cfg_err {
        tracing::warn!("конфиг: {e}");
    }
    tracing::info!(version = env!("CARGO_PKG_VERSION"), "старт");

    let Some(_instance) = single_instance::acquire() else {
        tracing::info!("уже запущен — открываю панель существующего экземпляра");
        hidden_window::notify_running_instance();
        return ExitCode::SUCCESS;
    };

    if let Err(e) = slint::BackendSelector::new()
        .backend_name("winit".into())
        .renderer_name("software".into())
        .with_winit_window_attributes_hook(panel::window_attributes)
        .select()
    {
        tracing::error!("Slint backend: {e}");
        return ExitCode::FAILURE;
    }

    if let Err(e) = app::init() {
        tracing::error!("инициализация: {e}");
        return ExitCode::FAILURE;
    }
    let result = slint::run_event_loop_until_quit();
    app::shutdown();

    match result {
        Ok(()) => {
            tracing::info!("завершено");
            ExitCode::SUCCESS
        }
        Err(e) => {
            tracing::error!("event loop: {e}");
            ExitCode::FAILURE
        }
    }
}
