// SPDX-License-Identifier: GPL-3.0-or-later
//! Известные USB PID Logitech (VID `0x046D`). Пополняется по реальным дампам из `docs/devices/`.
//! Тип устройства определяется опросом (ping/0x0005), ресивер — ещё и по строке продукта,
//! гарнитура Centurion — по коллекции `0xFFA0`; таблица нужна для имён и подсказок.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KnownKind {
    Receiver,
    Headset,
}

#[derive(Debug, Clone, Copy)]
pub struct Known {
    pub pid: u16,
    pub name: &'static str,
    pub kind: KnownKind,
}

const KNOWN: &[Known] = &[
    // PID по Solaar (не проверены на железе).
    Known {
        pid: 0xC52B,
        name: "Unifying Receiver",
        kind: KnownKind::Receiver,
    },
    Known {
        pid: 0xC548,
        name: "Bolt Receiver",
        kind: KnownKind::Receiver,
    },
    // Проверены (docs/devices).
    Known {
        pid: 0xC547,
        name: "LIGHTSPEED Receiver",
        kind: KnownKind::Receiver,
    },
    Known {
        pid: 0xC54D,
        name: "LIGHTSPEED Receiver",
        kind: KnownKind::Receiver,
    },
    Known {
        pid: 0x0AF7,
        name: "PRO X 2 LIGHTSPEED",
        kind: KnownKind::Headset,
    },
];

pub fn lookup(pid: u16) -> Option<&'static Known> {
    KNOWN.iter().find(|k| k.pid == pid)
}
