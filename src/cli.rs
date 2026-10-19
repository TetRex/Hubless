// SPDX-License-Identifier: GPL-3.0-or-later
//! CLI-подкоманды устройств. Каждая запись сначала печатает свой пакет; `--dry-run` — только печать.

use std::process::ExitCode;

use hidapi::HidApi;

use crate::devices::discovery::{self, Found, TYPE_MOUSE};
use crate::devices::mouse;
use crate::hidpp::features::{battery, dpi, onboard, report_rate};
use crate::hidpp::message::hex;
use crate::hidpp::{Error, Write};

pub const MOUSE_USAGE: &str = "\
  hubless mouse [status]    состояние мыши: батарея, DPI, частота опроса, lift-off, onboard
  hubless mouse backup      полный бэкап onboard-памяти в %APPDATA%\\Hubless\\backups
  hubless mouse dpi <N> [<Y>] [--dry-run]          DPI (и отдельно Y, если поддерживается)
  hubless mouse lod <low|medium|high> [--dry-run]  lift-off distance
  hubless mouse rate <Гц> [--dry-run]              частота опроса
  hubless mouse mode <onboard|host> [--dry-run]    режим настроек";

pub fn mouse(args: &[String]) -> ExitCode {
    let dry_run = args.iter().any(|a| a == "--dry-run");
    let args: Vec<String> = args.iter().filter(|a| *a != "--dry-run").cloned().collect();
    let api = match HidApi::new() {
        Ok(api) => api,
        Err(e) => {
            eprintln!("hidapi: {e}");
            return ExitCode::FAILURE;
        }
    };
    let mice = discovery::find(&api, TYPE_MOUSE);
    let Some(found) = mice.first() else {
        eprintln!("Мышь не найдена (выключена или спит?).");
        return ExitCode::FAILURE;
    };
    match args.first().map(String::as_str) {
        None | Some("status") => {
            print_status(found);
            ExitCode::SUCCESS
        }
        Some("backup") => match mouse::backup(found) {
            Ok(path) => {
                println!("Бэкап: {}", path.display());
                println!(
                    "Описание секторов: {}",
                    path.with_extension("json").display()
                );
                ExitCode::SUCCESS
            }
            Err(e) => {
                eprintln!("Бэкап не удался: {e}");
                ExitCode::FAILURE
            }
        },
        Some(cmd @ ("dpi" | "lod" | "rate" | "mode")) => match plan_write(found, cmd, &args[1..]) {
            Ok(w) => run_write(found, &w, dry_run),
            Err(e) => {
                eprintln!("{e}");
                ExitCode::from(2)
            }
        },
        Some(other) => {
            eprintln!("Неизвестная команда: mouse {other}\n\n{MOUSE_USAGE}");
            ExitCode::from(2)
        }
    }
}

/// Построить запись по аргументам с проверкой против того, что поддерживает мышь.
fn plan_write(found: &Found, cmd: &str, args: &[String]) -> Result<Write, String> {
    let dev = found.device();
    let arg = |i: usize| args.get(i).map(String::as_str);
    match cmd {
        "dpi" | "lod" => {
            let current = dpi::parameters(&dev).map_err(|e| format!("чтение DPI: {e}"))?;
            let caps = dpi::capabilities(&dev).map_err(|e| format!("возможности DPI: {e}"))?;
            let (x, y, lod) = if cmd == "dpi" {
                let parse = |s: Option<&str>| s.and_then(|s| s.parse::<u16>().ok());
                let x =
                    parse(arg(0)).ok_or("укажите DPI числом, например: hubless mouse dpi 1600")?;
                let y = match parse(arg(1)) {
                    Some(_) if !caps.separate_y => {
                        return Err("мышь не поддерживает отдельный DPI по Y".into());
                    }
                    Some(y) => y,
                    None => x,
                };
                let supported =
                    dpi::supported(&dev, false).map_err(|e| format!("список DPI: {e}"))?;
                for v in [x, y] {
                    if !supported.contains(&v) {
                        return Err(format!(
                            "DPI {v} не поддерживается: {}",
                            mouse::describe_dpi_list(&supported)
                        ));
                    }
                }
                (x, y, current.lift_off)
            } else {
                if !caps.lift_off {
                    return Err("мышь не поддерживает настройку lift-off".into());
                }
                let lod = arg(0)
                    .and_then(dpi::LiftOff::parse)
                    .ok_or("укажите low, medium или high")?;
                (current.x, current.y, lod)
            };
            Ok(dpi::set_parameters(x, y, lod))
        }
        "rate" => {
            let hz: u32 = arg(0)
                .and_then(|s| s.parse().ok())
                .ok_or("укажите частоту в Гц, например 1000")?;
            let index =
                report_rate::index_of_hz(hz).ok_or(format!("неизвестная частота {hz} Гц"))?;
            let supported =
                report_rate::supported(&dev).map_err(|e| format!("список частот: {e}"))?;
            if !supported.contains(&index) {
                let list: Vec<String> = supported
                    .iter()
                    .filter_map(|&i| report_rate::hz(i))
                    .map(|h| h.to_string())
                    .collect();
                return Err(format!(
                    "{hz} Гц недоступна в этом подключении (доступно: {} Гц)",
                    list.join(", ")
                ));
            }
            Ok(report_rate::set(index))
        }
        _ => {
            let mode = arg(0)
                .and_then(onboard::Mode::parse)
                .ok_or("укажите onboard или host")?;
            Ok(onboard::set_mode(mode))
        }
    }
}

/// Пакет в hex; младшая тетрада 4-го байта (sw_id) показана как `x`, хвост нулей свёрнут.
fn packet_text(bytes: &[u8]) -> String {
    let mut text: Vec<String> = bytes.iter().map(|b| format!("{b:02x}")).collect();
    if let Some(b) = bytes.get(3) {
        text[3] = format!("{:x}x", b >> 4);
    }
    let used = bytes
        .iter()
        .rposition(|&b| b != 0)
        .map_or(4, |i| (i + 1).max(4));
    let zeros = bytes.len() - used;
    let mut s = text[..used].join(" ");
    if zeros > 0 {
        s.push_str(&format!(" … ({zeros} × 00)"));
    }
    s
}

fn run_write(found: &Found, w: &Write, dry_run: bool) -> ExitCode {
    let dev = found.device();
    let packet = match dev.preview(w) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::FAILURE;
        }
    };
    println!("{}: {}", found.name, w.what);
    println!("  пакет: {}", packet_text(packet.as_bytes()));
    if dry_run {
        println!("  --dry-run: не отправлено");
        return ExitCode::SUCCESS;
    }
    match dev.apply(w) {
        Ok(r) => {
            println!("  ответ: {}", hex(r.as_bytes()));
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("  ошибка: {e}");
            ExitCode::FAILURE
        }
    }
}

fn show<T>(r: &Result<T, Error>, f: impl FnOnce(&T) -> String) -> String {
    match r {
        Ok(v) => f(v),
        Err(e) => format!("ошибка: {e}"),
    }
}

fn print_status(found: &Found) {
    let dev = found.device();
    let s = mouse::status(&dev);
    let link = if found.via_receiver() {
        format!("ресивер {:04X}, слот {}", found.pid, found.index)
    } else {
        format!("кабель, PID {:04X}", found.pid)
    };
    println!("{} ({link})", found.name);
    println!(
        "  батарея:     {}",
        show(&s.battery, |b| {
            let state = match b.charging {
                battery::ChargeState::Discharging => String::new(),
                c if c.is_charging() => ", заряжается".into(),
                c => format!(", {c:?}"),
            };
            format!("{}%{state}", b.percent)
        })
    );
    println!(
        "  DPI:         {}",
        show(&s.dpi, |p| {
            let y = if p.y != p.x && p.y != 0 {
                format!(" / Y {}", p.y)
            } else {
                String::new()
            };
            format!("{}{y} (по умолчанию {})", p.x, p.x_default)
        })
    );
    println!(
        "  возможности: {}",
        show(&s.dpi_caps, |c| format!(
            "раздельный Y: {}, lift-off: {}",
            yes(c.separate_y),
            yes(c.lift_off)
        ))
    );
    println!(
        "  lift-off:    {}",
        show(&s.dpi, |p| format!("{:?}", p.lift_off))
    );
    println!(
        "  DPI доступно: {}",
        show(&s.dpi_supported, |l| mouse::describe_dpi_list(l))
    );
    println!(
        "  опрос:       {}",
        show(&s.rate, |i| report_rate::hz(*i)
            .map_or(format!("индекс {i}"), |hz| format!("{hz} Гц")))
    );
    println!(
        "  опрос доступно: {}",
        show(&s.rate_supported, |l| {
            l.iter()
                .filter_map(|&i| report_rate::hz(i))
                .map(|hz| hz.to_string())
                .collect::<Vec<_>>()
                .join(", ")
                + " Гц"
        })
    );
    println!(
        "  режим:       {}",
        show(&s.onboard_mode, |m| format!("{m:?}"))
    );
    println!(
        "  профиль:     {}",
        show(&s.onboard_profile, |p| format!("{p:#06x}"))
    );
    println!(
        "  слот DPI:    {}",
        show(&s.onboard_dpi_index, |i| i.to_string())
    );
}

fn yes(b: bool) -> &'static str {
    if b { "да" } else { "нет" }
}
