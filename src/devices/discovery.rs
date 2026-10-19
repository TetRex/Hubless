// SPDX-License-Identifier: GPL-3.0-or-later
//! Поиск устройств HID++ 2.0: проводные (индекс 0xFF) и за ресиверами (слоты 1–6).

use hidapi::HidApi;

use crate::devices::registry::{self, KnownKind};
use crate::hidpp::features::device_name;
use crate::hidpp::message::DEVICE_SELF;
use crate::hidpp::root::{self, FeatureEntry, Protocol};
use crate::hidpp::transport::{self, Transport};
use crate::hidpp::{Device, receiver};

/// Тип устройства «мышь» из `0x0005`.
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

/// Найти устройства заданного типа (по `0x0005`). Только чтение.
pub fn find(api: &HidApi, want_kind: u8) -> Vec<Found> {
    let mut found = Vec::new();
    for info in transport::enumerate(api)
        .into_iter()
        .filter(|i| i.is_hidpp())
    {
        let is_receiver = registry::lookup(info.pid).is_some_and(|k| k.kind == KnownKind::Receiver);
        let Ok(t) = Transport::open(api, &info) else {
            continue;
        };
        let slots: Vec<u8> = if is_receiver {
            // Только спаренные слоты; тип из pairing info отсекает чужие устройства без пинга.
            receiver::read_info(&t)
                .iter()
                .filter_map(|r| {
                    let slot: u8 = r.label.strip_prefix("pairing_info_slot")?.parse().ok()?;
                    let raw = r.result.as_ref().ok()?;
                    match receiver::pairing_device_type(raw) {
                        Some(kind) if kind != want_kind => None,
                        _ => Some(slot),
                    }
                })
                .collect()
        } else {
            vec![DEVICE_SELF]
        };
        for index in slots {
            if let Some((name, table)) = probe(&t, index, want_kind) {
                found.push(Found {
                    transport: t,
                    index,
                    pid: info.pid,
                    name,
                    table,
                });
                break;
            }
        }
    }
    found
}

fn probe(t: &Transport, index: u8, want_kind: u8) -> Option<(String, Vec<FeatureEntry>)> {
    if !matches!(root::ping(t, index), Ok(Protocol::V2(..))) {
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
