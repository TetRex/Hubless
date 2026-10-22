// SPDX-License-Identifier: GPL-3.0-or-later
//! Known Logitech USB PIDs (VID `0x046D`). Extended from real dumps in `docs/devices/`.
//! The device type is determined by querying (ping/0x0005), a receiver also by its product string,
//! a Centurion headset by the `0xFFA0` collection; the table is for names and hints.

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
    // PIDs per Solaar (not verified on hardware).
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
    // Verified (docs/devices).
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
