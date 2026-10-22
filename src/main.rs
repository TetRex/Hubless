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
mod foreground;
mod headset_eq;
mod hidden_window;
mod hidpp;
mod i18n;
mod logging;
mod panel;
mod report;
mod settings;
mod settings_window;
mod single_instance;
mod state;
mod theme;
mod timeutil;
mod tray;
mod update;
mod win;

mod ui {
    slint::include_modules!();
}

use std::process::ExitCode;

use windows::Win32::System::Console::{ATTACH_PARENT_PROCESS, AttachConsole};

const USAGE: &str = "\
Hubless — a lightweight G Hub replacement

Usage:
  hubless                   run in the tray
  hubless --dump [file]     devices and HID++ feature tables (default hubless-dump.json)
  hubless --report          save a device report to send as a GitHub issue
  hubless --test-toast      show a test notification (Hubless must be running)
  hubless --settings [devices|profiles|eq|advanced]   (eq — devices with the headset equalizer open)
                            open the settings window of the running Hubless
  hubless --quit            quit the running Hubless (like Exit in the menu)
  hubless --help            this help
  hubless --version         version

Mouse:";

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
        Some("keyboard") => {
            attach_console();
            let (cfg, _) = config::Config::load();
            logging::init(&cfg);
            cli::keyboard(&args[1..])
        }
        Some("profile") => {
            attach_console();
            cli::profile(&args[1..])
        }
        Some("headset") => {
            attach_console();
            let (cfg, _) = config::Config::load();
            logging::init(&cfg);
            cli::headset(&args[1..])
        }
        Some("--test-toast") => {
            attach_console();
            if hidden_window::request_test_notification() {
                println!("Sent to the running Hubless.");
                ExitCode::SUCCESS
            } else {
                eprintln!("Hubless is not running: notifications are shown by the tray app.");
                ExitCode::FAILURE
            }
        }
        Some("--report") => {
            attach_console();
            let (cfg, _) = config::Config::load();
            logging::init(&cfg);
            match report::create() {
                Ok(r) => {
                    println!("Device report saved: {}", r.path.display());
                    println!("Send it as a GitHub issue: {}", report::ISSUE_URL);
                    ExitCode::SUCCESS
                }
                Err(e) => {
                    eprintln!("{e}");
                    ExitCode::FAILURE
                }
            }
        }
        Some("--settings") => {
            attach_console();
            let page = match args.get(1).map(String::as_str) {
                Some("devices") => 1,
                Some("profiles") => 2,
                Some("report" | "advanced") => 3,
                Some("eq") => 4,
                _ => 0,
            };
            if hidden_window::request_settings(page) {
                ExitCode::SUCCESS
            } else {
                eprintln!("Hubless is not running.");
                ExitCode::FAILURE
            }
        }
        Some("--quit") => {
            attach_console();
            if hidden_window::request_quit() {
                println!("The running Hubless is quitting.");
                ExitCode::SUCCESS
            } else {
                eprintln!("Hubless is not running.");
                ExitCode::FAILURE
            }
        }
        Some("--help" | "-h") => {
            attach_console();
            println!(
                "{USAGE}\n{}\n\nKeyboard:\n{}\n\nHeadset:\n{}\n\nProfiles:\n{}",
                cli::MOUSE_USAGE,
                cli::KEYBOARD_USAGE,
                cli::HEADSET_USAGE,
                cli::PROFILE_USAGE
            );
            ExitCode::SUCCESS
        }
        Some("--version" | "-V") => {
            attach_console();
            println!("hubless {}", env!("CARGO_PKG_VERSION"));
            ExitCode::SUCCESS
        }
        Some(other) => {
            attach_console();
            eprintln!("Unknown argument: {other}\n\n{USAGE}\n{}", cli::MOUSE_USAGE);
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
    // Язык строк из Rust — сразу; переводы Slint выбираются после создания первого окна.
    i18n::set(i18n::Choice::parse(&cfg.language).resolve());

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
