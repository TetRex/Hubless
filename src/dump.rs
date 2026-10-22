// SPDX-License-Identifier: GPL-3.0-or-later
//! `hubless --dump`: all Logitech devices and their HID++ features, as text and as JSON.
//! Read-only. Serial numbers and unit IDs are masked.
//!
//! The text is collected in a per-thread buffer ([`out!`]): the CLI prints it and the settings window
//! shows it (the device report for new models).

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use hidapi::HidApi;
use serde_json::{Value, json};

use crate::centurion;
use crate::devices::registry::{self, KnownKind};
use crate::hidpp::features::{self, device_info, device_name};
use crate::hidpp::message::{DEVICE_SELF, hex};
use crate::hidpp::root::{self, FeatureEntry, Protocol};
use crate::hidpp::transport::{self, Interface, Transport};
use crate::hidpp::{Device, Error, receiver};

const MASK: &str = "XXXXXXXX";
pub const DEFAULT_FILE: &str = "hubless-dump.json";

thread_local! {
    static TEXT: RefCell<String> = const { RefCell::new(String::new()) };
}

/// A line for the text part of the report (like `println!`).
macro_rules! out {
    ($($arg:tt)*) => {
        TEXT.with_borrow_mut(|t| {
            use std::fmt::Write as _;
            let _ = writeln!(t, $($arg)*);
        })
    };
}

/// The device report: text for a human, and JSON.
pub struct Dump {
    pub text: String,
    pub json: Value,
}

/// Queries all Logitech devices (read-only).
pub fn collect() -> Result<Dump, String> {
    let api = HidApi::new().map_err(|e| format!("hidapi: {e}"))?;
    TEXT.with_borrow_mut(String::clear);
    let interfaces = transport::enumerate(&api);
    if interfaces.is_empty() {
        out!("No Logitech devices found.");
    }
    let mut json_ifaces = Vec::new();
    for info in interfaces {
        json_ifaces.push(dump_interface(&api, info));
    }
    let json = json!({
        "tool": format!("hubless {}", env!("CARGO_PKG_VERSION")),
        "generated_utc": crate::timeutil::iso_utc(),
        "interfaces": json_ifaces,
    });
    Ok(Dump {
        text: TEXT.with_borrow_mut(std::mem::take),
        json,
    })
}

pub fn run(out: Option<PathBuf>) -> ExitCode {
    let dump = match collect() {
        Ok(d) => d,
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::FAILURE;
        }
    };
    print!("{}", dump.text);
    let path = out.unwrap_or_else(|| PathBuf::from(DEFAULT_FILE));
    match write_json(&path, &dump.json) {
        Ok(()) => {
            println!("\nJSON: {}", path.display());
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("\ncould not write {}: {e}", path.display());
            ExitCode::FAILURE
        }
    }
}

fn write_json(path: &Path, doc: &Value) -> std::io::Result<()> {
    let mut text = serde_json::to_string_pretty(doc)?;
    text.push('\n');
    std::fs::write(path, text)
}

fn dump_interface(api: &HidApi, info: Interface) -> Value {
    let known = registry::lookup(info.pid);
    let title = known.map_or(info.product.as_str(), |k| k.name);
    out!(
        "\n== {:04X} {} (interface {}, \"{}\") ==",
        info.pid,
        title,
        info.interface,
        info.product
    );
    let collections: Vec<String> = info
        .collections
        .iter()
        .map(|c| format!("{:04X}:{:04X}", c.usage_page, c.usage))
        .collect();
    out!("  collections: {}", collections.join(" "));

    let mut j = json!({
        "pid": format!("{:04X}", info.pid),
        "product": info.product,
        "known_as": known.map(|k| k.name),
        "interface": info.interface,
        "release": format!("{:04X}", info.release),
        "usb_serial": info.serial.as_ref().map(|_| MASK),
        "collections": collections,
    });
    // Centurion (headsets) is found by the 0xFFA0 collection, not by PID: that way unknown models get in too.
    if let Some(c) = info
        .collections
        .iter()
        .find(|c| c.usage_page == centurion::USAGE_PAGE)
    {
        j["centurion"] = dump_centurion(api, &c.path);
        return j;
    }
    if !info.is_hidpp() {
        out!("  no HID++ collections (page FF00/FF43), skipped");
        return j;
    }

    let transport = match Transport::open(api, &info) {
        Ok(t) => t,
        Err(e) => {
            out!("  could not open: {e}");
            j["error"] = json!(e.to_string());
            return j;
        }
    };

    let mut devices = Vec::new();
    let mut is_receiver = known.is_some_and(|k| k.kind == KnownKind::Receiver)
        || info.product.to_ascii_lowercase().contains("receiver");
    // LIGHTSPEED receivers don't answer a ping at 0xFF at all, so they get the quick mode.
    let ping_self = || root::ping(&transport, DEVICE_SELF);
    let self_protocol = if is_receiver {
        transport.quick(ping_self)
    } else {
        ping_self()
    };
    match &self_protocol {
        Ok(Protocol::V1) => {
            out!("  0xFF: HID++ 1.0 receiver");
            is_receiver = true;
        }
        Ok(Protocol::V2(major, minor)) => {
            out!("  0xFF: HID++ {major}.{minor}");
            let d = dump_device(&transport, DEVICE_SELF, (*major, *minor));
            is_receiver |= d["type"] == "receiver";
            devices.push(d);
        }
        Err(e) => out!("  0xFF: {e}"),
    }
    j["self_protocol"] = protocol_json(&self_protocol);

    if is_receiver {
        j["role"] = json!("receiver");
        let registers = dump_registers(&transport);
        for slot in 1..=6u8 {
            // A slot without pairing info is most likely empty, so it is queried quickly.
            let paired = registers.iter().any(|r| {
                r["label"] == format!("pairing_info_slot{slot}") && r.get("raw").is_some()
            });
            let ping = || root::ping(&transport, slot);
            let result = if paired {
                ping()
            } else {
                transport.quick(ping)
            };
            match result {
                Ok(Protocol::V2(major, minor)) => {
                    out!("  slot {slot}: HID++ {major}.{minor}");
                    devices.push(dump_device(&transport, slot, (major, minor)));
                }
                Ok(Protocol::V1) => out!("  slot {slot}: HID++ 1.0 (not supported)"),
                Err(Error::Timeout) if !paired => out!("  slot {slot}: not paired"),
                Err(Error::Timeout) => out!("  slot {slot}: no reply (asleep or off)"),
                Err(e) => out!("  slot {slot}: {e}"),
            }
        }
        j["registers"] = Value::Array(registers);
    }
    j["devices"] = Value::Array(devices);

    let notes = transport.take_notifications();
    if !notes.is_empty() {
        j["notifications"] = json!(notes.iter().map(|r| hex(r.as_bytes())).collect::<Vec<_>>());
    }
    j
}

fn protocol_json(p: &Result<Protocol, Error>) -> Value {
    match p {
        Ok(Protocol::V2(major, minor)) => json!(format!("{major}.{minor}")),
        Ok(Protocol::V1) => json!("1.0"),
        Err(e) => json!({ "error": e.to_string() }),
    }
}

fn dump_registers(t: &Transport) -> Vec<Value> {
    receiver::read_info(t)
        .into_iter()
        .map(|reg| {
            let mut v = json!({
                "label": reg.label,
                "register": format!("{:02X}", reg.register),
                "params": hex(&reg.params),
            });
            match &reg.result {
                Ok(raw) => {
                    out!("  reg  {:<20} {}", reg.label, hex(raw));
                    v["raw"] = json!(hex(raw));
                    if reg.label.starts_with("pairing_info")
                        && let Some(wpid) = receiver::pairing_wpid(raw)
                    {
                        v["wpid"] = json!(format!("{wpid:04X}"));
                    }
                }
                Err(e) => {
                    out!("  reg  {:<20} {e}", reg.label);
                    v["error"] = json!(e.to_string());
                }
            }
            v
        })
        .collect()
}

fn dump_device(t: &Transport, index: u8, protocol: (u8, u8)) -> Value {
    let mut errors = Vec::new();
    let table = match root::enumerate(t, index) {
        Ok(table) => table,
        Err(e) => {
            out!("    feature table: {e}");
            errors.push(format!("feature table: {e}"));
            Vec::new()
        }
    };
    let dev = Device::new(t, index, &table);

    let name = device_name::name(&dev)
        .inspect_err(|e| errors.push(format!("name: {e}")))
        .ok();
    let kind = device_name::kind(&dev)
        .inspect_err(|e| errors.push(format!("type: {e}")))
        .ok();
    out!(
        "    {} ({})",
        name.as_deref().unwrap_or("?"),
        kind.map_or("?", |k| k.1)
    );

    let mut j = json!({
        "index": format!("{index:02X}"),
        "protocol": format!("{}.{}", protocol.0, protocol.1),
        "name": name,
        "type": kind.map(|k| k.1),
        "type_id": kind.map(|k| k.0),
    });

    match device_info::device_info(&dev) {
        Ok(info) => {
            let mut raw_hex: Vec<String> = info.raw.iter().map(|b| format!("{b:02x}")).collect();
            for i in device_info::UNIT_ID_BYTES {
                raw_hex[i] = "XX".into();
            }
            let models: Vec<String> = info.model_ids.iter().map(|m| format!("{m:04X}")).collect();
            out!(
                "    model {} transport {:04X}, firmware entities: {}",
                models.join("/"),
                info.transport,
                info.entity_count
            );
            j["device_info"] = json!({
                "entity_count": info.entity_count,
                "unit_id": MASK,
                "transport": format!("{:04X}", info.transport),
                "model_ids": models,
                "extended_model_id": format!("{:02X}", info.extended_model_id),
                "capabilities": format!("{:02X}", info.capabilities),
                "raw": raw_hex.join(" "),
            });
            let mut fw = Vec::new();
            for entity in 0..info.entity_count {
                match device_info::firmware(&dev, entity) {
                    Ok(f) => {
                        out!("    {:<12} {}", f.kind, f.version);
                        fw.push(json!({
                            "entity": f.entity,
                            "kind": f.kind,
                            "version": f.version,
                            "raw": hex(&f.raw),
                        }));
                    }
                    Err(e) => errors.push(format!("firmware {entity}: {e}")),
                }
            }
            j["firmware"] = Value::Array(fw);
        }
        Err(e) => errors.push(format!("device info: {e}")),
    }

    out!("    features ({}):", table.len());
    out!("      idx  id    ver  flags             name");
    j["features"] = Value::Array(table.iter().map(feature_json).collect());
    if !errors.is_empty() {
        for e in &errors {
            out!("    ! {e}");
        }
        j["errors"] = json!(errors);
    }
    j
}

fn feature_json(f: &FeatureEntry) -> Value {
    let flags = root::flag_names(f.flags);
    let name = features::name(f.id);
    out!(
        "      {:>3}  {:04X}  {:>3}  {:<16}  {}{}",
        f.index,
        f.id,
        f.version,
        flags.join(","),
        name.unwrap_or("?"),
        if features::is_forbidden(f.id) {
            "  [blacklisted]"
        } else {
            ""
        }
    );
    json!({
        "index": f.index,
        "id": format!("{:04X}", f.id),
        "name": name,
        "version": f.version,
        "flags": flags,
    })
}

/// A headset on the Centurion protocol: read-only (bridge, feature tables, battery).
fn dump_centurion(api: &HidApi, path: &std::ffi::CStr) -> Value {
    let mut a = match centurion::Adapter::open(api, path) {
        Ok(a) => a,
        Err(e) => {
            out!("  Centurion: could not open: {e}");
            return json!({ "error": e.to_string() });
        }
    };
    let mut j = json!({});
    let mut errors = Vec::new();
    let features_json = |list: &[centurion::Feature]| -> Vec<Value> {
        list.iter()
            .map(|f| {
                let name = centurion::feature_name(f.id);
                out!(
                    "      {:>3}  {:04X}  {:>3}  {:02X}     {}",
                    f.index,
                    f.id,
                    f.version,
                    f.flags,
                    name.unwrap_or("?")
                );
                json!({
                    "index": f.index,
                    "id": format!("{:04X}", f.id),
                    "name": name,
                    "version": f.version,
                    "flags": format!("{:02X}", f.flags),
                })
            })
            .collect()
    };

    match a.adapter_features() {
        Ok(list) => {
            out!("  Centurion, adapter features ({}):", list.len());
            out!("      idx  id    ver  flags  name");
            j["adapter_features"] = json!(features_json(&list));
        }
        Err(e) => errors.push(format!("adapter features: {e}")),
    }
    match a.find_bridge() {
        Ok(index) => {
            out!("  bridge (0x0003): index {index}");
            j["bridge_index"] = json!(index);
        }
        Err(e) => {
            out!("  bridge (0x0003): {e}");
            errors.push(format!("bridge: {e}"));
            j["errors"] = json!(errors);
            j["frames"] = json!(a.log);
            return j;
        }
    }
    match a.bridge_status() {
        Ok(s) => {
            out!("  bridge state: {}", hex(&s));
            j["bridge_status"] = json!(hex(&s));
        }
        Err(e) => errors.push(format!("bridge status: {e}")),
    }
    match a.headset_features() {
        Ok(list) => {
            out!("  headset features ({}):", list.len());
            out!("      idx  id    ver  flags  name");
            j["headset_features"] = json!(features_json(&list));
        }
        Err(e) => {
            out!("  headset features: {e}");
            errors.push(format!("headset features: {e}"));
        }
    }
    match a.battery() {
        Ok(b) => {
            let charging = matches!(b.charging_state, 1 | 2);
            out!(
                "  battery: {}%{} (raw {})",
                b.percent,
                if charging { ", charging" } else { "" },
                hex(&b.raw)
            );
            j["battery"] = json!({
                "percent": b.percent,
                "charging_state": b.charging_state,
                "raw": hex(&b.raw),
            });
        }
        Err(e) => {
            out!("  battery: {e}");
            errors.push(format!("battery: {e}"));
        }
    }
    if !errors.is_empty() {
        j["errors"] = json!(errors);
    }
    j["frames"] = json!(a.log);
    j
}
