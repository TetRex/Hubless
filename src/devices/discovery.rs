// SPDX-License-Identifier: GPL-3.0-or-later
//! Поиск устройств HID++ 2.0: проводные (индекс 0xFF) и за ресиверами (слоты 1–6).

use hidapi::HidApi;

use crate::devices::registry::{self, KnownKind};
use crate::hidpp::features::device_name;
use crate::hidpp::message::DEVICE_SELF;
use crate::hidpp::root::{self, FeatureEntry, Protocol};
use crate::hidpp::transport::{self, Transport};
use crate::hidpp::{Device, receiver};

/// Типы устройств из `0x0005`.
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

/// Ресивер, у которого спарено устройство нужного типа, но оно не ответило (спит или выключено).
/// Держим его открытым и ждём уведомления о связи (`0x41`) для этих слотов.
pub struct Sleeping {
    pub transport: Transport,
    pub slots: Vec<u8>,
    /// Имя спящего устройства из регистров ресивера (если ресивер его знает).
    pub name: Option<String>,
}

/// Ресивер по известному PID или строке продукта «… Receiver» (так называются и Unifying,
/// и Bolt, и LIGHTSPEED). LIGHTSPEED-ресиверы на ping по `0xFF` не отвечают вовсе —
/// поэтому сначала эти признаки, а ping — только для остальных.
fn named_receiver(info: &transport::Interface) -> bool {
    registry::lookup(info.pid).is_some_and(|k| k.kind == KnownKind::Receiver)
        || info.product.to_ascii_lowercase().contains("receiver")
}

/// Найти устройства заданного типа (по `0x0005`). Только чтение.
pub fn find(api: &HidApi, want_kind: u8) -> Vec<Found> {
    find_or_sleeping(api, want_kind).0
}

/// Как [`find`], плюс ресивер со спящим устройством нужного типа, если ничего не нашлось.
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
        // Не ресивер по имени: ping по 0xFF — HID++ 2.0 у проводного устройства, 1.0 у ресивера.
        let self_ping = (!named_receiver(&info)).then(|| root::ping(&t, DEVICE_SELF));
        let receiver = matches!(self_ping, None | Some(Ok(Protocol::V1)));
        // Слот, точно ли там устройство нужного типа (по pairing info) и быстрый ли пинг.
        let slots: Vec<(u8, bool, bool)> = if receiver {
            let regs = receiver::read_info(&t);
            let reg = |label: String| {
                regs.iter()
                    .find(|r| r.label == label)
                    .and_then(|r| r.result.as_ref().ok())
            };
            // Только спаренные слоты; тип из pairing info отсекает чужие устройства без пинга.
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
                // Pairing info не читается (Bolt и неизвестные ресиверы): пингуем все слоты быстро.
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

/// Опросить устройство: HID++ 2.0 нужного типа — имя и таблица фич. `quick` — пинг без
/// повтора и с коротким таймаутом (слот, про который неизвестно, спарен ли он).
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
