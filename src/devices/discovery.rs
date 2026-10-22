// SPDX-License-Identifier: GPL-3.0-or-later
//! Finding HID++ 2.0 devices: wired ones (index 0xFF) and ones behind receivers (slots 1–6).

use hidapi::HidApi;

use crate::devices::registry::{self, KnownKind};
use crate::hidpp::features::device_name;
use crate::hidpp::message::DEVICE_SELF;
use crate::hidpp::root::{self, FeatureEntry, Protocol};
use crate::hidpp::transport::{self, Transport};
use crate::hidpp::{Device, receiver};

/// Device types from `0x0005`.
pub const TYPE_KEYBOARD: u8 = 0;
pub const TYPE_MOUSE: u8 = 3;

pub struct Found {
    pub transport: Transport,
    pub index: u8,
    pub pid: u16,
    pub name: String,
    pub table: Vec<FeatureEntry>,
}

impl Found {
    pub fn device(&self) -> Device<'_> {
        Device::new(&self.transport, self.index, &self.table)
    }

    pub fn via_receiver(&self) -> bool {
        self.index != DEVICE_SELF
    }
}

/// A receiver with a paired device of the right type that didn't answer (asleep or off).
/// We keep it open and wait for the connection notification (`0x41`) for those slots.
pub struct Sleeping {
    pub transport: Transport,
    pub slots: Vec<u8>,
    /// The name of the sleeping device from the receiver's registers (if the receiver knows it).
    pub name: Option<String>,
}

/// A receiver by a known PID or a product string "… Receiver" (that's what Unifying,
/// Bolt and LIGHTSPEED receivers are all called). LIGHTSPEED receivers don't answer a ping at `0xFF`
/// at all, so these signs come first, and the ping is only for the rest.
fn named_receiver(info: &transport::Interface) -> bool {
    registry::lookup(info.pid).is_some_and(|k| k.kind == KnownKind::Receiver)
        || info.product.to_ascii_lowercase().contains("receiver")
}

/// Finds devices of the given type (by `0x0005`). Read-only.
pub fn find(api: &HidApi, want_kind: u8) -> Vec<Found> {
    find_or_sleeping(api, want_kind).0
}

/// Like [`find`], plus a receiver with a sleeping device of the right type if nothing was found.
pub fn find_or_sleeping(api: &HidApi, want_kind: u8) -> (Vec<Found>, Option<Sleeping>) {
    let mut found = Vec::new();
    let mut sleeping = None;
    for info in transport::enumerate(api)
        .into_iter()
        .filter(|i| i.is_hidpp())
    {
        let Ok(t) = Transport::open(api, &info) else {
            continue;
        };
        let mut names = Vec::new();
        // Not a receiver by name: a ping at 0xFF gives HID++ 2.0 for a wired device, 1.0 for a receiver.
        let self_ping = (!named_receiver(&info)).then(|| root::ping(&t, DEVICE_SELF));
        let receiver = matches!(self_ping, None | Some(Ok(Protocol::V1)));
        // A slot, whether it surely holds a device of the right type (by pairing info), and whether the ping is quick.
        let slots: Vec<(u8, bool, bool)> = if receiver {
            let regs = receiver::read_info(&t);
            let reg = |label: String| {
                regs.iter()
                    .find(|r| r.label == label)
                    .and_then(|r| r.result.as_ref().ok())
            };
            // Only paired slots; the type from pairing info filters out other devices without a ping.
            let paired: Vec<(u8, bool, bool)> = (1..=6u8)
                .filter_map(|slot| {
                    let raw = reg(format!("pairing_info_slot{slot}"))?;
                    match receiver::pairing_device_type(raw) {
                        Some(kind) if kind != want_kind => None,
                        kind => Some((slot, kind.is_some(), false)),
                    }
                })
                .collect();
            for &(slot, ..) in &paired {
                if let Some(name) = reg(format!("device_name_slot{slot}"))
                    .and_then(|raw| receiver::pairing_name(raw))
                {
                    names.push((slot, name));
                }
            }
            let any_pairing = (1..=6u8).any(|s| reg(format!("pairing_info_slot{s}")).is_some());
            if any_pairing {
                paired
            } else {
                // Pairing info can't be read (Bolt and unknown receivers): ping all slots quickly.
                (1..=6u8).map(|slot| (slot, false, true)).collect()
            }
        } else if matches!(self_ping, Some(Ok(Protocol::V2(..)))) {
            vec![(DEVICE_SELF, false, false)]
        } else {
            Vec::new()
        };
        let mut silent = Vec::new();
        let mut hit = None;
        for (index, typed, quick) in slots {
            match probe(&t, index, want_kind, quick) {
                Some((name, table)) => {
                    hit = Some((index, name, table));
                    break;
                }
                None if typed => silent.push(index),
                None => {}
            }
        }
        match hit {
            Some((index, name, table)) => found.push(Found {
                transport: t,
                index,
                pid: info.pid,
                name,
                table,
            }),
            None if !silent.is_empty() && sleeping.is_none() => {
                let name = names
                    .into_iter()
                    .find(|(slot, _)| silent.contains(slot))
                    .map(|(_, name)| name);
                sleeping = Some(Sleeping {
                    transport: t,
                    slots: silent,
                    name,
                });
            }
            None => {}
        }
    }
    if !found.is_empty() {
        sleeping = None;
    }
    (found, sleeping)
}

/// Queries a device: an HID++ 2.0 device of the right type gives its name and feature table. `quick` is a
/// ping without a retry and with a short timeout (a slot that may or may not be paired).
fn probe(
    t: &Transport,
    index: u8,
    want_kind: u8,
    quick: bool,
) -> Option<(String, Vec<FeatureEntry>)> {
    let ping = || root::ping(t, index);
    let protocol = if quick { t.quick(ping) } else { ping() };
    if !matches!(protocol, Ok(Protocol::V2(..))) {
        return None;
    }
    let table = root::enumerate(t, index).ok()?;
    let dev = Device::new(t, index, &table);
    let (kind, _) = device_name::kind(&dev).ok()?;
    if kind != want_kind {
        return None;
    }
    let name = device_name::name(&dev).unwrap_or_default();
    Some((name, table))
}
