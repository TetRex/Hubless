// SPDX-License-Identifier: GPL-3.0-or-later
//! CLI-подкоманды устройств. Каждая запись сначала печатает свой пакет; `--dry-run` — только печать.

use std::process::ExitCode;

use hidapi::HidApi;

use crate::centurion;
use crate::devices::discovery::{self, Found, TYPE_MOUSE};
use crate::devices::{keyboard, mouse};
use crate::hidpp::features::{battery, brightness, dpi, onboard, report_rate, rgb_effects};
use crate::hidpp::message::hex;
use crate::hidpp::{Error, Write};
use crate::settings::{self, DeviceSettings, MouseSettings};

pub const MOUSE_USAGE: &str =
    "  hubless mouse [status]    состояние мыши: батарея, DPI, частота опроса, lift-off, onboard
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
        Some("raw") => match parse_raw(&args[1..]) {
            Ok(writes) => run_sequence(found, &writes, dry_run),
            Err(e) => {
                eprintln!(
                    "{e}\n  hubless mouse raw <фича>:<функция>[s] <байты…> [, …] [--dry-run]"
                );
                ExitCode::from(2)
            }
        },
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
            Ok((w, save)) => run_write(found, &w, dry_run, save),
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
/// Как сохранить применённую запись в `devices.toml`.
type Save = Box<dyn FnOnce(&mut MouseSettings)>;

fn plan_write(found: &Found, cmd: &str, args: &[String]) -> Result<(Write, Save), String> {
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
            let lod_saved = mouse::lift_off_from_device(lod);
            let save: Save = if cmd == "dpi" {
                Box::new(move |m| {
                    m.dpi = Some(x);
                    m.dpi_y = Some(y);
                })
            } else {
                Box::new(move |m| m.lift_off = lod_saved)
            };
            Ok((dpi::set_parameters(x, y, lod), save))
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
            Ok((
                report_rate::set(index),
                Box::new(move |m| m.rate_hz = Some(hz)),
            ))
        }
        _ => {
            let mode = arg(0)
                .and_then(onboard::Mode::parse)
                .ok_or("укажите onboard или host")?;
            let saved = match mode {
                onboard::Mode::Host => settings::Mode::Host,
                _ => settings::Mode::Onboard,
            };
            Ok((
                onboard::set_mode(mode),
                Box::new(move |m| m.mode = Some(saved)),
            ))
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

fn run_write(found: &Found, w: &Write, dry_run: bool, save: Save) -> ExitCode {
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
            if mouse::verify(&dev, w) == Some(false) {
                eprintln!(
                    "  устройство приняло запись, но значение не изменилось — в настройки не сохраняю"
                );
                return ExitCode::FAILURE;
            }
            let settings = save_settings(|s| save(&mut s.mouse));
            if settings.mouse.mode != Some(settings::Mode::Host) {
                println!(
                    "  режим onboard: при переподключении мышь вернётся к своему профилю; \
                     чтобы Hubless применял настройки сам — hubless mouse mode host"
                );
            }
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

fn link_text(found: &Found) -> String {
    if found.via_receiver() {
        format!("ресивер {:04X}, слот {}", found.pid, found.index)
    } else {
        format!("кабель, PID {:04X}", found.pid)
    }
}

fn battery_text(b: &battery::Battery) -> String {
    let state = match b.charging {
        battery::ChargeState::Discharging => String::new(),
        c if c.is_charging() => ", заряжается".into(),
        c => format!(", {c:?}"),
    };
    format!("{}%{state}", b.percent)
}

fn print_status(found: &Found) {
    let dev = found.device();
    let s = mouse::status(&dev);
    println!("{} ({})", found.name, link_text(found));
    println!("  батарея:     {}", show(&s.battery, battery_text));
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

pub const KEYBOARD_USAGE: &str =
    "  hubless keyboard [status]                    батарея, яркость, подсветка, Game Mode
  hubless keyboard brightness <N> [--dry-run]  яркость подсветки
  hubless keyboard lighting static <RRGGBB> | off | release [--dry-run]
                                               эффект подсветки (Hubless забирает управление) / вернуть клавиатуре
  hubless keyboard lighting breathe <RRGGBB> [--period МС] [--intensity 0–100]
  hubless keyboard lighting cycle [--period МС] [--intensity 0–100]
  hubless keyboard lighting wave [--direction НАПР] [--period МС] [--intensity 0–100]
  hubless keyboard lighting ripple <RRGGBB> [--period 2–200]
                                               встроенные эффекты; период 1000–20000 мс (по умолчанию 5000),
                                               направления: right, left, up, down, center-out, center-in, in, out, cycle";

pub fn keyboard(args: &[String]) -> ExitCode {
    let dry_run = args.iter().any(|a| a == "--dry-run");
    let args: Vec<String> = args.iter().filter(|a| *a != "--dry-run").cloned().collect();
    let api = match HidApi::new() {
        Ok(api) => api,
        Err(e) => {
            eprintln!("hidapi: {e}");
            return ExitCode::FAILURE;
        }
    };
    let keyboards = discovery::find(&api, discovery::TYPE_KEYBOARD);
    let Some(found) = keyboards.first() else {
        eprintln!("Клавиатура не найдена (выключена или спит?).");
        return ExitCode::FAILURE;
    };
    match args.first().map(String::as_str) {
        None | Some("status") => {
            print_keyboard_status(found);
            ExitCode::SUCCESS
        }
        Some("lighting") => match plan_lighting(found, &args[1..]) {
            Ok((writes, lighting)) => {
                let code = run_sequence(found, &writes, dry_run);
                if !dry_run && code == ExitCode::SUCCESS {
                    save_settings(|s| s.keyboard.lighting = lighting);
                    if lighting.is_some() && !crate::hidden_window::is_running() {
                        println!(
                            "  Hubless в трее не запущен: гасить подсветку при простое некому — \
                             запустите hubless или верните управление: hubless keyboard lighting release"
                        );
                    }
                }
                code
            }
            Err(e) => {
                eprintln!("{e}");
                ExitCode::from(2)
            }
        },
        Some("brightness") => {
            let dev = found.device();
            let info = match brightness::info(&dev) {
                Ok(i) => i,
                Err(e) => {
                    eprintln!("яркость: {e}");
                    return ExitCode::FAILURE;
                }
            };
            let Some(value) = args.get(1).and_then(|s| s.parse::<u16>().ok()) else {
                eprintln!("укажите яркость числом от {} до {}", info.min, info.max);
                return ExitCode::from(2);
            };
            if !(info.min..=info.max).contains(&value) {
                eprintln!("яркость {value} вне диапазона {}–{}", info.min, info.max);
                return ExitCode::from(2);
            }
            run_keyboard_write(found, &brightness::set(value), dry_run)
        }
        Some(other) => {
            eprintln!("Неизвестная команда: keyboard {other}\n\n{KEYBOARD_USAGE}");
            ExitCode::from(2)
        }
    }
}

fn print_keyboard_status(found: &Found) {
    let dev = found.device();
    let s = keyboard::status(&dev);
    println!("{} ({})", found.name, link_text(found));
    println!("  батарея:     {}", show(&s.battery, battery_text));
    println!(
        "  яркость:     {}",
        show(&s.brightness, |v| match &s.brightness_info {
            Ok(i) => format!("{v} (диапазон {}–{}, ступеней {})", i.min, i.max, i.steps),
            Err(_) => v.to_string(),
        })
    );
    if let Some(on) = &s.illumination {
        println!(
            "  подсветка:   {}",
            show(on, |on| if *on { "вкл" } else { "выкл" }.into())
        );
    }
    match &s.rgb {
        Ok(info) => {
            println!(
                "  зоны подсветки: {} (caps {:#06x})",
                info.zones.len(),
                info.caps
            );
            for z in &info.zones {
                let effects: Vec<String> = z
                    .effects
                    .iter()
                    .map(|e| {
                        rgb_effects::effect_name(e.id)
                            .map_or(format!("{:#04x}", e.id), str::to_owned)
                    })
                    .collect();
                println!(
                    "    зона {} ({}): {}",
                    z.index,
                    rgb_effects::location_name(z.location),
                    effects.join(", ")
                );
            }
        }
        Err(e) => println!("  зоны подсветки: ошибка: {e}"),
    }
    if let Ok(raw) = rgb_effects::sw_control(&dev) {
        println!(
            "  управление:  {} (сырые байты {})",
            if raw.get(1).copied().unwrap_or(0) == 0 {
                "у клавиатуры"
            } else {
                "у приложения"
            },
            hex(&raw)
        );
    }
    if let Ok((idle, sleep)) = rgb_effects::timers(&dev) {
        println!("  таймеры:     простой {idle} с, сон {sleep} с");
    }
    println!("  per-key:     {}", yes(s.has_per_key));
    println!(
        "  опрос:       {}",
        show(&s.rate_ms, |ms| format!("{ms} мс"))
    );
    println!(
        "  режим:       {}",
        show(&s.onboard_mode, |m| format!("{m:?}"))
    );
    println!(
        "  Game Mode:   {}",
        if s.has_disable_controls {
            "0x4523 есть, протокол не описан — нужен захват трафика G Hub"
        } else {
            "нет 0x4523"
        }
    );
}

/// Изменить `devices.toml` и попросить запущенный Hubless применить настройки заново.
fn save_settings(update: impl FnOnce(&mut DeviceSettings)) -> DeviceSettings {
    let settings = match DeviceSettings::update(update) {
        Ok(s) => {
            println!("  сохранено в {}", DeviceSettings::path().display());
            s
        }
        Err(e) => {
            eprintln!("  не удалось сохранить настройки: {e}");
            DeviceSettings::load()
        }
    };
    if crate::hidden_window::notify_settings_changed() {
        println!("  запущенный Hubless применит настройки");
    }
    settings
}

/// Запись в клавиатуру: печать пакета, отправка, проверка чтением, сохранение в настройки.
fn run_keyboard_write(found: &Found, w: &Write, dry_run: bool) -> ExitCode {
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
            let applied = match w.feature {
                brightness::ID => brightness::current(&dev)
                    .ok()
                    .map(|v| v.to_be_bytes().as_slice() == w.params.as_slice()),
                _ => None,
            };
            if applied == Some(false) {
                eprintln!("  устройство приняло запись, но значение не изменилось");
                return ExitCode::FAILURE;
            }
            if w.feature == brightness::ID {
                let value = u16::from_be_bytes([w.params[0], w.params[1]]);
                save_settings(|s| s.keyboard.brightness = Some(value));
            }
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("  ошибка: {e}");
            ExitCode::FAILURE
        }
    }
}

/// Последовательность записей для подсветки (как в Solaar: режим host + перехват управления).
/// Возвращает записи и новое значение настройки (`None` — подсветкой управляет клавиатура).
fn plan_lighting(
    found: &Found,
    args: &[String],
) -> Result<(Vec<Write>, Option<settings::Lighting>), String> {
    let dev = found.device();
    let info = rgb_effects::info(&dev).map_err(|e| format!("подсветка: {e}"))?;
    let zone = info.zones.first().ok_or("у клавиатуры нет зон подсветки")?;
    let color = |name: &str| {
        args.get(1)
            .and_then(|s| rgb_effects::parse_rgb(s))
            .ok_or(format!(
                "укажите цвет RRGGBB, например: hubless keyboard lighting {name} ff8000"
            ))
    };
    let opts = EffectOptions::parse(&args[1..])?;
    let lighting = match args.first().map(String::as_str) {
        Some("static") => settings::Lighting::Static {
            color: color("static")?,
        },
        Some("breathe") => settings::Lighting::Breathe {
            color: color("breathe")?,
            period_ms: opts.period_ms()?,
            intensity: opts.intensity,
        },
        Some("cycle") => settings::Lighting::Cycle {
            period_ms: opts.period_ms()?,
            intensity: opts.intensity,
        },
        Some("wave") => settings::Lighting::Wave {
            period_ms: opts.period_ms()?,
            direction: opts.direction,
            intensity: opts.intensity,
        },
        Some("ripple") => settings::Lighting::Ripple {
            color: color("ripple")?,
            period: opts.ripple_period()?,
        },
        Some("off") => settings::Lighting::Off,
        Some("release") => {
            let writes = vec![
                rgb_effects::release(),
                onboard::set_mode(onboard::Mode::Onboard),
            ];
            return Ok((writes, None));
        }
        _ => {
            return Err(
                "укажите: static <RRGGBB> | breathe <RRGGBB> | cycle | wave | ripple <RRGGBB> | off | release"
                    .into(),
            );
        }
    };
    let effect =
        rgb_effects::effect_for(zone, lighting).ok_or("зона не поддерживает этот эффект")?;
    let writes = vec![
        onboard::set_mode(onboard::Mode::Host),
        rgb_effects::claim(),
        effect,
    ];
    Ok((writes, Some(lighting)))
}

/// Необязательные параметры встроенных эффектов: `--period`, `--intensity`, `--direction`.
struct EffectOptions {
    period: Option<u16>,
    intensity: u8,
    direction: rgb_effects::Direction,
}

impl EffectOptions {
    fn parse(args: &[String]) -> Result<Self, String> {
        let mut o = Self {
            period: None,
            intensity: 100,
            direction: rgb_effects::Direction::Right,
        };
        let mut it = args.iter();
        while let Some(a) = it.next() {
            if !a.starts_with("--") {
                continue;
            }
            let v = it.next().ok_or(format!("{a}: нет значения"))?;
            match a.as_str() {
                "--period" => {
                    o.period = Some(v.parse().map_err(|_| format!("--period: не число: {v}"))?);
                }
                "--intensity" => {
                    o.intensity = v
                        .parse()
                        .ok()
                        .filter(|i| *i <= 100)
                        .ok_or(format!("--intensity: нужно число 0–100: {v}"))?;
                }
                "--direction" => {
                    o.direction = rgb_effects::Direction::parse(v)
                        .ok_or(format!("--direction: неизвестное направление: {v}"))?;
                }
                _ => return Err(format!("неизвестный параметр: {a}")),
            }
        }
        Ok(o)
    }

    fn period_ms(&self) -> Result<u16, String> {
        let p = self.period.unwrap_or(5000);
        rgb_effects::PERIOD_MS
            .contains(&p)
            .then_some(p)
            .ok_or(format!("--period: нужно {:?} мс", rgb_effects::PERIOD_MS))
    }

    fn ripple_period(&self) -> Result<u16, String> {
        let p = self.period.unwrap_or(rgb_effects::RIPPLE_PERIOD_DEFAULT);
        rgb_effects::RIPPLE_PERIOD
            .contains(&p)
            .then_some(p)
            .ok_or(format!("--period: нужно {:?}", rgb_effects::RIPPLE_PERIOD))
    }
}

/// Напечатать все пакеты последовательности, затем (без --dry-run) отправить по порядку.
fn run_sequence(found: &Found, writes: &[Write], dry_run: bool) -> ExitCode {
    let dev = found.device();
    println!("{}:", found.name);
    for (i, w) in writes.iter().enumerate() {
        match dev.preview(w) {
            Ok(r) => println!(
                "  {}. {}\n     пакет: {}",
                i + 1,
                w.what,
                packet_text(r.as_bytes())
            ),
            Err(e) => {
                eprintln!("  {}. {}: {e}", i + 1, w.what);
                return ExitCode::FAILURE;
            }
        }
    }
    if dry_run {
        if writes
            .iter()
            .any(|w| w.feature == rgb_effects::ID && w.params == rgb_effects::claim().params)
        {
            print_idle_packets(&dev);
        }
        println!("  --dry-run: не отправлено");
        return ExitCode::SUCCESS;
    }
    for (i, w) in writes.iter().enumerate() {
        match dev.apply(w) {
            Ok(r) => println!("  {}. ответ: {}", i + 1, hex(r.as_bytes())),
            Err(e) => {
                eprintln!("  {}. ошибка: {e} — остальные записи не отправлены", i + 1);
                return ExitCode::FAILURE;
            }
        }
    }
    ExitCode::SUCCESS
}

/// Пакеты, которые Hubless в трее отправляет, пока держит управление подсветкой.
fn print_idle_packets(dev: &crate::hidpp::Device) {
    println!("  Пока подсветкой управляет Hubless, при простое и активности он отправляет:");
    let steps = [
        ("клавиатура сообщила о простое", rgb_effects::idle_mode()),
        (
            "через «сон − простой» секунд после простоя",
            rgb_effects::power_off(),
        ),
        (
            "клавиатура сообщила об активности (после сна)",
            rgb_effects::power_on(),
        ),
        ("затем снова", rgb_effects::claim()),
    ];
    for (when, w) in steps {
        if let Ok(r) = dev.preview(&w) {
            println!(
                "   - {when}: {}
       пакет: {}",
                w.what,
                packet_text(r.as_bytes())
            );
        }
    }
    println!("   - после пробуждения повторяется эффект зоны (пакет 3 выше)");
}

pub const HEADSET_USAGE: &str =
    "  hubless headset [status]                     батарея, sidetone, автоотключение
  hubless headset sidetone <0–100> [--dry-run] громкость своего голоса в наушниках
  hubless headset auto-off <0–255> [--dry-run] автоотключение через N минут, 0 — никогда";

pub fn headset(args: &[String]) -> ExitCode {
    let dry_run = args.iter().any(|a| a == "--dry-run");
    let args: Vec<String> = args.iter().filter(|a| *a != "--dry-run").cloned().collect();
    let api = match HidApi::new() {
        Ok(api) => api,
        Err(e) => {
            eprintln!("hidapi: {e}");
            return ExitCode::FAILURE;
        }
    };
    let Some(mut a) = centurion::Adapter::find(&api) else {
        eprintln!("Адаптер гарнитуры не найден.");
        return ExitCode::FAILURE;
    };
    match a.headset_connected() {
        Ok(true) => {}
        Ok(false) => {
            eprintln!("Гарнитура не подключена к адаптеру (выключена?).");
            return ExitCode::FAILURE;
        }
        Err(e) => {
            eprintln!("состояние моста: {e}");
            return ExitCode::FAILURE;
        }
    }
    let value = |name: &str, max: u8| -> Result<u8, ExitCode> {
        args.get(1)
            .and_then(|s| s.parse::<u8>().ok())
            .filter(|v| *v <= max)
            .ok_or_else(|| {
                eprintln!("{name}: укажите число 0–{max}");
                ExitCode::from(2)
            })
    };
    match args.first().map(String::as_str) {
        None | Some("status") => {
            print_headset_status(&mut a);
            ExitCode::SUCCESS
        }
        Some("sidetone") => {
            let level = match value("sidetone", centurion::SIDETONE_MAX) {
                Ok(v) => v,
                Err(c) => return c,
            };
            let state = match a.sidetone() {
                Ok(s) => s,
                Err(e) => {
                    eprintln!("sidetone: {e}");
                    return ExitCode::FAILURE;
                }
            };
            run_headset_write(&mut a, &centurion::set_sidetone(&state, level), dry_run)
        }
        Some("auto-off") => match value("auto-off", u8::MAX) {
            Ok(m) => run_headset_write(&mut a, &centurion::set_auto_sleep(m), dry_run),
            Err(c) => c,
        },
        Some(other) => {
            eprintln!("Неизвестная команда: headset {other}\n\n{HEADSET_USAGE}");
            ExitCode::from(2)
        }
    }
}

fn print_headset_status(a: &mut centurion::Adapter) {
    println!("PRO X 2 LIGHTSPEED (адаптер Centurion)");
    match a.battery() {
        Ok(b) => println!(
            "  батарея:     {}%{} (сырые байты {})",
            b.percent,
            if matches!(b.charging_state, 1 | 2) {
                ", заряжается"
            } else {
                ""
            },
            hex(&b.raw)
        ),
        Err(e) => println!("  батарея:     ошибка: {e}"),
    }
    match a.sidetone_level() {
        Ok(v) => println!("  sidetone:    {v} (0–{})", centurion::SIDETONE_MAX),
        Err(e) => println!("  sidetone:    ошибка: {e}"),
    }
    match a.auto_sleep_minutes() {
        Ok(0) => println!("  автоотключение: никогда"),
        Ok(m) => println!("  автоотключение: через {m} мин"),
        Err(e) => println!("  автоотключение: ошибка: {e}"),
    }
}

/// Запись в гарнитуру: печать кадра, отправка, ответ.
fn run_headset_write(a: &mut centurion::Adapter, w: &centurion::Write, dry_run: bool) -> ExitCode {
    let frame = match a.preview(w) {
        Ok(f) => f,
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::FAILURE;
        }
    };
    println!("PRO X 2 LIGHTSPEED: {}", w.what);
    println!("  кадр: {}", headset_frame_text(&frame));
    if dry_run {
        println!("  --dry-run: не отправлено");
        return ExitCode::SUCCESS;
    }
    match a.apply(w) {
        Ok(r) => {
            println!("  ответ: {}", hex(&r));
            // Проверка чтением: значение — последний байт параметров записи.
            let expected = w.params.last().copied().unwrap_or(0);
            let actual = match w.feature {
                centurion::SIDETONE => a.sidetone_level(),
                centurion::AUTO_SLEEP => a.auto_sleep_minutes(),
                _ => return ExitCode::SUCCESS,
            };
            match actual {
                Ok(v) if v == expected => {
                    println!("  проверено чтением: {v}");
                    ExitCode::SUCCESS
                }
                Ok(v) => {
                    eprintln!("  устройство приняло запись, но читается {v} вместо {expected}");
                    ExitCode::FAILURE
                }
                Err(e) => {
                    eprintln!("  проверка чтением не удалась: {e}");
                    ExitCode::FAILURE
                }
            }
        }
        Err(e) => {
            eprintln!("  ошибка: {e}");
            ExitCode::FAILURE
        }
    }
}

/// Кадр Centurion в hex со свёрнутым хвостом нулей.
fn headset_frame_text(frame: &[u8]) -> String {
    let used = frame.iter().rposition(|&b| b != 0).map_or(1, |i| i + 1);
    let mut s = hex(&frame[..used]);
    if used < frame.len() {
        s.push_str(&format!(" … ({} × 00)", frame.len() - used));
    }
    s
}

pub const PROFILE_USAGE: &str =
    "  hubless profile [list]                       профили настроек мыши и клавиатуры
  hubless profile add <имя>                    новый профиль — копия действующего
  hubless profile use <имя>                    выбрать профиль
  hubless profile bind <имя> <exe>             включать профиль, когда активно окно exe (например cs2.exe)
  hubless profile remove <имя>                 удалить профиль";

pub fn profile(args: &[String]) -> ExitCode {
    let mut file = settings::SettingsFile::load();
    let arg = |i: usize| args.get(i).map(String::as_str);
    let result = match (arg(0), arg(1), arg(2)) {
        (None | Some("list"), _, _) => {
            let current = file.current_name();
            for name in file.names() {
                let exe = file
                    .profiles
                    .iter()
                    .find(|p| p.name == name)
                    .map(|p| p.exe.join(", "))
                    .unwrap_or_default();
                let mark = if name == current { "*" } else { " " };
                if exe.is_empty() {
                    println!("{mark} {name}");
                } else {
                    println!("{mark} {name}  ({exe})");
                }
            }
            return ExitCode::SUCCESS;
        }
        (Some("add"), Some(name), _) => file.add(name),
        (Some("use"), Some(name), _) => file.select(name),
        (Some("remove"), Some(name), _) => file.remove(name),
        (Some("bind"), Some(name), Some(exe)) => file.bind(name, exe),
        _ => {
            eprintln!("{PROFILE_USAGE}");
            return ExitCode::from(2);
        }
    };
    match result.and_then(|()| file.save()) {
        Ok(()) => {
            println!("Действующий профиль: {}", file.current_name());
            if crate::hidden_window::notify_settings_changed() {
                println!("  запущенный Hubless применит настройки");
            }
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("{e}");
            ExitCode::FAILURE
        }
    }
}

/// Отладочная последовательность записей (для повторения захватов G Hub):
/// `2202:6 00 03 20 03 20 02 , 2202:7 00 01 , 8061:3s 02` — фича (hex), функция, `s` — short-отчёт,
/// затем байты параметров. Записи уходят подряд за один запуск; DFU-фичи отвергает `Device`.
fn parse_raw(args: &[String]) -> Result<Vec<Write>, String> {
    let mut writes = Vec::new();
    for group in args.split(|a| a == ",") {
        let (head, bytes) = group.split_first().ok_or("пустая запись")?;
        let (feature, function) = head
            .split_once(':')
            .ok_or_else(|| format!("ожидалось <фича>:<функция>, а не {head}"))?;
        let (function, short) = match function.strip_suffix('s') {
            Some(f) => (f, true),
            None => (function, false),
        };
        let feature =
            u16::from_str_radix(feature, 16).map_err(|_| format!("фича не hex: {feature}"))?;
        let function: u8 = function
            .parse()
            .ok()
            .filter(|f| *f < 16)
            .ok_or_else(|| format!("функция 0–15: {function}"))?;
        let params = bytes
            .iter()
            .map(|b| u8::from_str_radix(b, 16).map_err(|_| format!("байт не hex: {b}")))
            .collect::<Result<Vec<u8>, String>>()?;
        writes.push(Write {
            feature,
            function,
            what: format!("{feature:#06x} функция {function}"),
            params,
            short,
        });
    }
    Ok(writes)
}
