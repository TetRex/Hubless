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
    "  hubless mouse [status]    mouse state: battery, DPI, polling rate, lift-off, onboard
  hubless mouse backup      full onboard memory backup to %APPDATA%\\Hubless\\backups
  hubless mouse dpi <N> [<Y>] [--dry-run]          DPI (and a separate Y, if supported)
  hubless mouse lod <low|medium|high> [--dry-run]  lift-off distance
  hubless mouse rate <Hz> [--dry-run]              polling rate
  hubless mouse mode <onboard|host> [--dry-run]    settings mode";

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
        eprintln!("Mouse not found (off or asleep?).");
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
                    "{e}\n  hubless mouse raw <feature>:<function>[s] <bytes…> [, …] [--dry-run]"
                );
                ExitCode::from(2)
            }
        },
        Some("backup") => match mouse::backup(found) {
            Ok(path) => {
                println!("Backup: {}", path.display());
                println!("Sector map: {}", path.with_extension("json").display());
                ExitCode::SUCCESS
            }
            Err(e) => {
                eprintln!("Backup failed: {e}");
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
            eprintln!("Unknown command: mouse {other}\n\n{MOUSE_USAGE}");
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
            let current = dpi::parameters(&dev).map_err(|e| format!("reading DPI: {e}"))?;
            let caps = dpi::capabilities(&dev).map_err(|e| format!("DPI capabilities: {e}"))?;
            let (x, y, lod) = if cmd == "dpi" {
                let parse = |s: Option<&str>| s.and_then(|s| s.parse::<u16>().ok());
                let x = parse(arg(0)).ok_or("give DPI as a number, e.g. hubless mouse dpi 1600")?;
                let y = match parse(arg(1)) {
                    Some(_) if !caps.separate_y => {
                        return Err("the mouse has no separate Y DPI".into());
                    }
                    Some(y) => y,
                    None => x,
                };
                let supported =
                    dpi::supported(&dev, false).map_err(|e| format!("DPI list: {e}"))?;
                for v in [x, y] {
                    if !supported.contains(&v) {
                        return Err(format!(
                            "DPI {v} is not supported: {}",
                            mouse::describe_dpi_list(&supported)
                        ));
                    }
                }
                (x, y, current.lift_off)
            } else {
                if !caps.lift_off {
                    return Err("the mouse has no lift-off setting".into());
                }
                let lod = arg(0)
                    .and_then(dpi::LiftOff::parse)
                    .ok_or("give low, medium or high")?;
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
                .ok_or("give the rate in Hz, e.g. 1000")?;
            let index = report_rate::index_of_hz(hz).ok_or(format!("unknown rate {hz} Hz"))?;
            let supported = report_rate::supported(&dev).map_err(|e| format!("rate list: {e}"))?;
            if !supported.contains(&index) {
                let list: Vec<String> = supported
                    .iter()
                    .filter_map(|&i| report_rate::hz(i))
                    .map(|h| h.to_string())
                    .collect();
                return Err(format!(
                    "{hz} Hz is not available on this connection (available: {} Hz)",
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
                .ok_or("give onboard or host")?;
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
    println!("  packet: {}", packet_text(packet.as_bytes()));
    if dry_run {
        println!("  --dry-run: not sent");
        return ExitCode::SUCCESS;
    }
    match dev.apply(w) {
        Ok(r) => {
            println!("  reply: {}", hex(r.as_bytes()));
            if mouse::verify(&dev, w) == Some(false) {
                eprintln!(
                    "  the device accepted the write but the value did not change; not saving it"
                );
                return ExitCode::FAILURE;
            }
            let settings = save_settings(|s| save(&mut s.mouse));
            if settings.mouse.mode != Some(settings::Mode::Host) {
                println!(
                    "  onboard mode: on reconnect the mouse returns to its own profile; \
                     to have Hubless apply settings itself, run: hubless mouse mode host"
                );
            }
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("  error: {e}");
            ExitCode::FAILURE
        }
    }
}

fn show<T>(r: &Result<T, Error>, f: impl FnOnce(&T) -> String) -> String {
    match r {
        Ok(v) => f(v),
        Err(e) => format!("error: {e}"),
    }
}

fn link_text(found: &Found) -> String {
    if found.via_receiver() {
        format!("receiver {:04X}, slot {}", found.pid, found.index)
    } else {
        format!("cable, PID {:04X}", found.pid)
    }
}

fn battery_text(b: &battery::Battery) -> String {
    let state = match b.charging {
        battery::ChargeState::Discharging => String::new(),
        c if c.is_charging() => ", charging".into(),
        c => format!(", {c:?}"),
    };
    format!("{}%{state}", b.percent)
}

fn print_status(found: &Found) {
    let dev = found.device();
    let s = mouse::status(&dev);
    println!("{} ({})", found.name, link_text(found));
    println!("  battery:     {}", show(&s.battery, battery_text));
    println!(
        "  DPI:         {}",
        show(&s.dpi, |p| {
            let y = if p.y != p.x && p.y != 0 {
                format!(" / Y {}", p.y)
            } else {
                String::new()
            };
            format!("{}{y} (default {})", p.x, p.x_default)
        })
    );
    println!(
        "  features:    {}",
        show(&s.dpi_caps, |c| format!(
            "separate Y: {}, lift-off: {}",
            yes(c.separate_y),
            yes(c.lift_off)
        ))
    );
    println!(
        "  lift-off:    {}",
        show(&s.dpi, |p| format!("{:?}", p.lift_off))
    );
    println!(
        "  DPI available: {}",
        show(&s.dpi_supported, |l| mouse::describe_dpi_list(l))
    );
    println!(
        "  polling:     {}",
        show(&s.rate, |i| report_rate::hz(*i)
            .map_or(format!("index {i}"), |hz| format!("{hz} Hz")))
    );
    println!(
        "  polling available: {}",
        show(&s.rate_supported, |l| {
            l.iter()
                .filter_map(|&i| report_rate::hz(i))
                .map(|hz| hz.to_string())
                .collect::<Vec<_>>()
                .join(", ")
                + " Hz"
        })
    );
    println!(
        "  mode:        {}",
        show(&s.onboard_mode, |m| format!("{m:?}"))
    );
    println!(
        "  profile:     {}",
        show(&s.onboard_profile, |p| format!("{p:#06x}"))
    );
    println!(
        "  DPI slot:    {}",
        show(&s.onboard_dpi_index, |i| i.to_string())
    );
}

fn yes(b: bool) -> &'static str {
    if b { "yes" } else { "no" }
}

pub const KEYBOARD_USAGE: &str =
    "  hubless keyboard [status]                    battery, brightness, lighting, Game Mode
  hubless keyboard brightness <N> [--dry-run]  backlight brightness
  hubless keyboard lighting static <RRGGBB> | off | release [--dry-run]
                                               lighting effect (Hubless takes control) / give it back to the keyboard
  hubless keyboard lighting breathe <RRGGBB> [--period MS] [--intensity 0–100]
  hubless keyboard lighting cycle [--period MS] [--intensity 0–100]
  hubless keyboard lighting wave [--direction DIR] [--period MS] [--intensity 0–100]
  hubless keyboard lighting ripple <RRGGBB> [--period 2–200]
                                               built-in effects; period 1000–20000 ms (default 5000),
                                               directions: right, left, up, down, center-out, center-in, in, out, cycle";

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
        eprintln!("Keyboard not found (off or asleep?).");
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
                            "  Hubless is not running in the tray, so nothing will dim the lighting when idle; \
                             start hubless or hand control back: hubless keyboard lighting release"
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
                    eprintln!("brightness: {e}");
                    return ExitCode::FAILURE;
                }
            };
            let Some(value) = args.get(1).and_then(|s| s.parse::<u16>().ok()) else {
                eprintln!(
                    "give brightness as a number from {} to {}",
                    info.min, info.max
                );
                return ExitCode::from(2);
            };
            if !(info.min..=info.max).contains(&value) {
                eprintln!("brightness {value} is outside {}–{}", info.min, info.max);
                return ExitCode::from(2);
            }
            run_keyboard_write(found, &brightness::set(value), dry_run)
        }
        Some(other) => {
            eprintln!("Unknown command: keyboard {other}\n\n{KEYBOARD_USAGE}");
            ExitCode::from(2)
        }
    }
}

fn print_keyboard_status(found: &Found) {
    let dev = found.device();
    let s = keyboard::status(&dev);
    println!("{} ({})", found.name, link_text(found));
    println!("  battery:     {}", show(&s.battery, battery_text));
    println!(
        "  brightness:  {}",
        show(&s.brightness, |v| match &s.brightness_info {
            Ok(i) => format!("{v} (range {}–{}, {} steps)", i.min, i.max, i.steps),
            Err(_) => v.to_string(),
        })
    );
    if let Some(on) = &s.illumination {
        println!(
            "  lighting:    {}",
            show(on, |on| if *on { "on" } else { "off" }.into())
        );
    }
    match &s.rgb {
        Ok(info) => {
            println!(
                "  lighting zones: {} (caps {:#06x})",
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
                    "    zone {} ({}): {}",
                    z.index,
                    rgb_effects::location_name(z.location),
                    effects.join(", ")
                );
            }
        }
        Err(e) => println!("  lighting zones: error: {e}"),
    }
    if let Ok(raw) = rgb_effects::sw_control(&dev) {
        println!(
            "  control:     {} (raw bytes {})",
            if raw.get(1).copied().unwrap_or(0) == 0 {
                "keyboard"
            } else {
                "application"
            },
            hex(&raw)
        );
    }
    if let Ok((idle, sleep)) = rgb_effects::timers(&dev) {
        println!("  timers:      idle {idle} s, sleep {sleep} s");
    }
    println!("  per-key:     {}", yes(s.has_per_key));
    println!(
        "  polling:     {}",
        show(&s.rate_ms, |ms| format!("{ms} ms"))
    );
    println!(
        "  mode:        {}",
        show(&s.onboard_mode, |m| format!("{m:?}"))
    );
    println!(
        "  Game Mode:   {}",
        if s.has_disable_controls {
            "0x4523 present, protocol unknown (needs a G Hub capture)"
        } else {
            "no 0x4523"
        }
    );
}

/// Изменить `devices.toml` и попросить запущенный Hubless применить настройки заново.
fn save_settings(update: impl FnOnce(&mut DeviceSettings)) -> DeviceSettings {
    let settings = match DeviceSettings::update(update) {
        Ok(s) => {
            println!("  saved to {}", DeviceSettings::path().display());
            s
        }
        Err(e) => {
            eprintln!("  could not save settings: {e}");
            DeviceSettings::load()
        }
    };
    if crate::hidden_window::notify_settings_changed() {
        println!("  the running Hubless will apply the settings");
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
    println!("  packet: {}", packet_text(packet.as_bytes()));
    if dry_run {
        println!("  --dry-run: not sent");
        return ExitCode::SUCCESS;
    }
    match dev.apply(w) {
        Ok(r) => {
            println!("  reply: {}", hex(r.as_bytes()));
            let applied = match w.feature {
                brightness::ID => brightness::current(&dev)
                    .ok()
                    .map(|v| v.to_be_bytes().as_slice() == w.params.as_slice()),
                _ => None,
            };
            if applied == Some(false) {
                eprintln!("  the device accepted the write but the value did not change");
                return ExitCode::FAILURE;
            }
            if w.feature == brightness::ID {
                let value = u16::from_be_bytes([w.params[0], w.params[1]]);
                save_settings(|s| s.keyboard.brightness = Some(value));
            }
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("  error: {e}");
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
    let info = rgb_effects::info(&dev).map_err(|e| format!("lighting: {e}"))?;
    let zone = info
        .zones
        .first()
        .ok_or("the keyboard has no lighting zones")?;
    let color = |name: &str| {
        args.get(1)
            .and_then(|s| rgb_effects::parse_rgb(s))
            .ok_or(format!(
                "give a colour as RRGGBB, e.g. hubless keyboard lighting {name} ff8000"
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
                "give: static <RRGGBB> | breathe <RRGGBB> | cycle | wave | ripple <RRGGBB> | off | release"
                    .into(),
            );
        }
    };
    let effect =
        rgb_effects::effect_for(zone, lighting).ok_or("the zone does not support this effect")?;
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
            let v = it.next().ok_or(format!("{a}: missing value"))?;
            match a.as_str() {
                "--period" => {
                    o.period = Some(
                        v.parse()
                            .map_err(|_| format!("--period: not a number: {v}"))?,
                    );
                }
                "--intensity" => {
                    o.intensity = v
                        .parse()
                        .ok()
                        .filter(|i| *i <= 100)
                        .ok_or(format!("--intensity: needs a number 0–100: {v}"))?;
                }
                "--direction" => {
                    o.direction = rgb_effects::Direction::parse(v)
                        .ok_or(format!("--direction: unknown direction: {v}"))?;
                }
                _ => return Err(format!("unknown option: {a}")),
            }
        }
        Ok(o)
    }

    fn period_ms(&self) -> Result<u16, String> {
        let p = self.period.unwrap_or(5000);
        rgb_effects::PERIOD_MS
            .contains(&p)
            .then_some(p)
            .ok_or(format!("--period: needs {:?} ms", rgb_effects::PERIOD_MS))
    }

    fn ripple_period(&self) -> Result<u16, String> {
        let p = self.period.unwrap_or(rgb_effects::RIPPLE_PERIOD_DEFAULT);
        rgb_effects::RIPPLE_PERIOD
            .contains(&p)
            .then_some(p)
            .ok_or(format!("--period: needs {:?}", rgb_effects::RIPPLE_PERIOD))
    }
}

/// Напечатать все пакеты последовательности, затем (без --dry-run) отправить по порядку.
fn run_sequence(found: &Found, writes: &[Write], dry_run: bool) -> ExitCode {
    let dev = found.device();
    println!("{}:", found.name);
    for (i, w) in writes.iter().enumerate() {
        match dev.preview(w) {
            Ok(r) => println!(
                "  {}. {}\n     packet: {}",
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
        println!("  --dry-run: not sent");
        return ExitCode::SUCCESS;
    }
    for (i, w) in writes.iter().enumerate() {
        match dev.apply(w) {
            Ok(r) => println!("  {}. reply: {}", i + 1, hex(r.as_bytes())),
            Err(e) => {
                eprintln!(
                    "  {}. error: {e}; the remaining writes were not sent",
                    i + 1
                );
                return ExitCode::FAILURE;
            }
        }
    }
    ExitCode::SUCCESS
}

/// Пакеты, которые Hubless в трее отправляет, пока держит управление подсветкой.
fn print_idle_packets(dev: &crate::hidpp::Device) {
    println!("  While Hubless controls the lighting, it sends on idle and activity:");
    let steps = [
        ("the keyboard reported idle", rgb_effects::idle_mode()),
        (
            "(sleep − idle) seconds after idle",
            rgb_effects::power_off(),
        ),
        (
            "the keyboard reported activity (after sleep)",
            rgb_effects::power_on(),
        ),
        ("then again", rgb_effects::claim()),
    ];
    for (when, w) in steps {
        if let Ok(r) = dev.preview(&w) {
            println!(
                "   - {when}: {}
       packet: {}",
                w.what,
                packet_text(r.as_bytes())
            );
        }
    }
    println!("   - after waking, the zone effect is sent again (packet 3 above)");
}

pub const HEADSET_USAGE: &str =
    "  hubless headset [status]                     battery, sidetone, auto-off
  hubless headset sidetone <0–100> [--dry-run] how loud your own voice is in the headset
  hubless headset auto-off <0–255> [--dry-run] turn off after N minutes, 0 means never";

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
        eprintln!("Headset adapter not found.");
        return ExitCode::FAILURE;
    };
    match a.headset_connected() {
        Ok(true) => {}
        Ok(false) => {
            eprintln!("The headset is not connected to the adapter (off?).");
            return ExitCode::FAILURE;
        }
        Err(e) => {
            eprintln!("bridge state: {e}");
            return ExitCode::FAILURE;
        }
    }
    let value = |name: &str, max: u8| -> Result<u8, ExitCode> {
        args.get(1)
            .and_then(|s| s.parse::<u8>().ok())
            .filter(|v| *v <= max)
            .ok_or_else(|| {
                eprintln!("{name}: give a number 0–{max}");
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
            eprintln!("Unknown command: headset {other}\n\n{HEADSET_USAGE}");
            ExitCode::from(2)
        }
    }
}

fn print_headset_status(a: &mut centurion::Adapter) {
    println!("PRO X 2 LIGHTSPEED (Centurion adapter)");
    match a.battery() {
        Ok(b) => println!(
            "  battery:     {}%{} (raw bytes {})",
            b.percent,
            if matches!(b.charging_state, 1 | 2) {
                ", charging"
            } else {
                ""
            },
            hex(&b.raw)
        ),
        Err(e) => println!("  battery:     error: {e}"),
    }
    match a.sidetone_level() {
        Ok(v) => println!("  sidetone:    {v} (0–{})", centurion::SIDETONE_MAX),
        Err(e) => println!("  sidetone:    error: {e}"),
    }
    match a.auto_sleep_minutes() {
        Ok(0) => println!("  auto-off:    never"),
        Ok(m) => println!("  auto-off:    after {m} min"),
        Err(e) => println!("  auto-off:    error: {e}"),
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
    println!("  frame: {}", headset_frame_text(&frame));
    if dry_run {
        println!("  --dry-run: not sent");
        return ExitCode::SUCCESS;
    }
    match a.apply(w) {
        Ok(r) => {
            println!("  reply: {}", hex(&r));
            // Проверка чтением: значение — последний байт параметров записи.
            let expected = w.params.last().copied().unwrap_or(0);
            let actual = match w.feature {
                centurion::SIDETONE => a.sidetone_level(),
                centurion::AUTO_SLEEP => a.auto_sleep_minutes(),
                _ => return ExitCode::SUCCESS,
            };
            match actual {
                Ok(v) if v == expected => {
                    println!("  read back: {v}");
                    ExitCode::SUCCESS
                }
                Ok(v) => {
                    eprintln!(
                        "  the device accepted the write but reads {v} instead of {expected}"
                    );
                    ExitCode::FAILURE
                }
                Err(e) => {
                    eprintln!("  read-back failed: {e}");
                    ExitCode::FAILURE
                }
            }
        }
        Err(e) => {
            eprintln!("  error: {e}");
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
    "  hubless profile [list]                       mouse and keyboard setting profiles
  hubless profile add <name>                   new profile, a copy of the current one
  hubless profile use <name>                   select a profile
  hubless profile bind <name> <exe>            switch to the profile while exe is in the foreground (e.g. cs2.exe)
  hubless profile remove <name>                delete a profile";

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
            println!("Current profile: {}", file.current_name());
            if crate::hidden_window::notify_settings_changed() {
                println!("  the running Hubless will apply the settings");
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
        let (head, bytes) = group.split_first().ok_or("empty write")?;
        let (feature, function) = head
            .split_once(':')
            .ok_or_else(|| format!("expected <feature>:<function>, got {head}"))?;
        let (function, short) = match function.strip_suffix('s') {
            Some(f) => (f, true),
            None => (function, false),
        };
        let feature = u16::from_str_radix(feature, 16)
            .map_err(|_| format!("feature is not hex: {feature}"))?;
        let function: u8 = function
            .parse()
            .ok()
            .filter(|f| *f < 16)
            .ok_or_else(|| format!("function must be 0–15: {function}"))?;
        let params = bytes
            .iter()
            .map(|b| u8::from_str_radix(b, 16).map_err(|_| format!("byte is not hex: {b}")))
            .collect::<Result<Vec<u8>, String>>()?;
        writes.push(Write {
            feature,
            function,
            what: format!("{feature:#06x} function {function}"),
            params,
            short,
        });
    }
    Ok(writes)
}
