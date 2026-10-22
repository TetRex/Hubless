// SPDX-License-Identifier: GPL-3.0-or-later
// Hubless: a lightweight G Hub replacement for Windows.
// Copyright (C) 2026 TetRex

#![windows_subsystem = "windows"]

mod app;
mod centurion;
mod cli;
mod config;
mod devices;
mod dump;
mod eq_presets;
mod foreground;
mod headset_eq;
mod hidden_window;
mod hidpp;
mod i18n;
mod install;
mod keyboard_layout;
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
  hubless --settings [devices|profiles|eq|keys|advanced]
                            (eq, keys — devices with the headset equalizer or the key colours open)
                            open the settings window of the running Hubless
  hubless --quit            quit the running Hubless (like Exit in the panel)
  hubless --uninstall       remove Hubless from this PC (the settings are kept)
  hubless --help            this help
  hubless --version         version

Mouse:";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        None => run_tray(false),
        // Started by the installer from the downloaded file: the installed copy.
        Some("--installed") => run_tray(true),
        Some("--uninstall") => {
            attach_console();
            let (cfg, _) = config::Config::load();
            logging::init(&cfg);
            i18n::set(i18n::Choice::parse(&cfg.language).resolve());
            install::uninstall()
        }
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
                Some("keys") => 5,
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

/// The GUI subsystem doesn't get a console; for the CLI we attach to the parent's console.
fn attach_console() {
    // SAFETY: no side effects if the parent has no console.
    let _ = unsafe { AttachConsole(ATTACH_PARENT_PROCESS) };
}

/// The tray app. `installed`: started by the installer, so say that the downloaded file can go.
fn run_tray(installed: bool) -> ExitCode {
    let (cfg, cfg_err) = config::Config::load();
    logging::init(&cfg);
    if let Some(e) = cfg_err {
        tracing::warn!("конфиг: {e}");
    }
    tracing::info!(version = env!("CARGO_PKG_VERSION"), "старт");
    // The language of Rust strings right away; Slint translations are selected after the first window is created.
    i18n::set(i18n::Choice::parse(&cfg.language).resolve());

    // Started from a downloaded file: install it and hand over to the installed copy.
    if !installed && let Some(code) = install::from_download() {
        return code;
    }

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
    if installed {
        app::post(app::AppEvent::Installed);
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
