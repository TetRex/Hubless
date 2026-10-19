// SPDX-License-Identifier: GPL-3.0-or-later
//! Регистры ресивера (HID++ 1.0) — только чтение, для диагностики.
//! Номера регистров — по Solaar; расшифровка минимальная, сырые байты сохраняются в дампе.

use super::Error;
use super::message::DEVICE_SELF;
use super::transport::Transport;

const CONNECTION_STATE: u8 = 0x02;
const RECEIVER_INFO: u8 = 0xB5;
const FIRMWARE: u8 = 0xF1;
/// Подрегистр RECEIVER_INFO с информацией о паре в слоте `n` (1..6): `0x20 + n - 1`.
const PAIRING_INFO: u8 = 0x20;

pub struct RegisterValue {
    pub label: String,
    pub register: u8,
    pub params: [u8; 3],
    pub result: Result<Vec<u8>, Error>,
}

fn read(
    t: &Transport,
    label: impl Into<String>,
    register: u8,
    long: bool,
    params: [u8; 3],
) -> RegisterValue {
    RegisterValue {
        label: label.into(),
        register,
        params,
        result: t
            .read_register(DEVICE_SELF, register, long, params)
            .map(|r| r.params().to_vec()),
    }
}

pub fn read_info(t: &Transport) -> Vec<RegisterValue> {
    let mut regs = vec![
        read(t, "connection_state", CONNECTION_STATE, false, [0; 3]),
        read(t, "firmware_version", FIRMWARE, false, [0x01, 0, 0]),
        read(t, "firmware_build", FIRMWARE, false, [0x02, 0, 0]),
        read(t, "bootloader", FIRMWARE, false, [0x04, 0, 0]),
    ];
    // У LIGHTSPEED-ресиверов обычно один слот: пустые не отвечают вовсе, поэтому быстрый режим.
    for slot in 1..=6u8 {
        regs.push(t.quick(|| {
            read(
                t,
                format!("pairing_info_slot{slot}"),
                RECEIVER_INFO,
                true,
                [PAIRING_INFO + slot - 1, 0, 0],
            )
        }));
    }
    regs
}

/// Wireless PID устройства из ответа pairing info (байты 3..5 после адреса регистра).
pub fn pairing_wpid(raw: &[u8]) -> Option<u16> {
    let wpid = u16::from_be_bytes([*raw.get(3)?, *raw.get(4)?]);
    (wpid != 0).then_some(wpid)
}
