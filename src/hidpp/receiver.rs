// SPDX-License-Identifier: GPL-3.0-or-later
//! Receiver registers (HID++ 1.0): read-only, for diagnostics.
//! Register numbers per Solaar; minimal decoding, the raw bytes are kept in the dump.

use super::Error;
use super::message::DEVICE_SELF;
use super::transport::Transport;

const CONNECTION_STATE: u8 = 0x02;
const RECEIVER_INFO: u8 = 0xB5;
const FIRMWARE: u8 = 0xF1;
/// The RECEIVER_INFO subregister with the pairing info of slot `n` (1..6): `0x20 + n - 1`.
const PAIRING_INFO: u8 = 0x20;
/// The RECEIVER_INFO subregister with the device name of slot `n`: `0x40 + n - 1` (per Solaar;
/// Bolt uses other subregisters, not read).
const DEVICE_NAME: u8 = 0x40;

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
    // LIGHTSPEED receivers have 1–2 slots; an empty one answers at once with error 0x09, so stop after
    // two empty slots in a row. The normal timeout with a retry, not the quick mode: right after a
    // device wakes up the receiver may answer late, and a missed reply made the paired mouse look
    // absent (2026-10-23). The quick mode dates from before error replies were parsed (2026-10-19),
    // when every error looked like a timeout.
    let mut empty_in_row = 0;
    for slot in 1..=6u8 {
        let reg = read(
            t,
            format!("pairing_info_slot{slot}"),
            RECEIVER_INFO,
            true,
            [PAIRING_INFO + slot - 1, 0, 0],
        );
        let paired = reg.result.is_ok();
        empty_in_row = if paired { 0 } else { empty_in_row + 1 };
        regs.push(reg);
        if paired {
            // The receiver itself knows the name, so a sleeping device has it too.
            regs.push(read(
                t,
                format!("device_name_slot{slot}"),
                RECEIVER_INFO,
                true,
                [DEVICE_NAME + slot - 1, 0, 0],
            ));
        }
        if empty_in_row == 2 {
            break;
        }
    }
    regs
}

/// The device's wireless PID from the pairing info reply (bytes 3..5 after the register address).
pub fn pairing_wpid(raw: &[u8]) -> Option<u16> {
    let wpid = u16::from_be_bytes([*raw.get(3)?, *raw.get(4)?]);
    (wpid != 0).then_some(wpid)
}

/// The device name from the DEVICE_NAME reply: `[subregister, length, name…]` (per Solaar).
pub fn pairing_name(raw: &[u8]) -> Option<String> {
    let len = usize::from(*raw.get(1)?);
    let bytes = raw.get(2..2 + len)?;
    let name = String::from_utf8_lossy(bytes)
        .trim_end_matches('\0')
        .trim()
        .to_owned();
    (!name.is_empty()).then_some(name)
}

/// The device type from pairing info (byte 7, low nibble; per Solaar): 1 is a keyboard, 2 a mouse.
/// Returns the type in `0x0005` terms (0 keyboard, 3 mouse), if it is known.
pub fn pairing_device_type(raw: &[u8]) -> Option<u8> {
    match raw.get(7)? & 0x0F {
        1 => Some(0),
        2 => Some(3),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn name_from_register() {
        let mut raw = vec![0x40, 7];
        raw.extend_from_slice(b"PRO X 2");
        raw.resize(16, 0);
        assert_eq!(pairing_name(&raw).as_deref(), Some("PRO X 2"));
        assert_eq!(pairing_name(&[0x40, 0]), None);
        assert_eq!(pairing_name(&[0x40, 20, b'A']), None, "длина больше ответа");
    }
}
