// SPDX-License-Identifier: GPL-3.0-or-later
//! HID++ reports: short `0x10` (7 bytes), long `0x11` (20), very long `0x12` (64).
//!
//! Format: `[report_id][device_index][feature_index | sub_id][function<<4 | sw_id | address][params...]`.

use std::fmt;

pub const SHORT: u8 = 0x10;
pub const LONG: u8 = 0x11;
pub const VERY_LONG: u8 = 0x12;

pub const SHORT_LEN: usize = 7;
pub const LONG_LEN: usize = 20;
pub const VERY_LONG_LEN: usize = 64;

/// The `device_index` of a wired device or of the receiver itself.
pub const DEVICE_SELF: u8 = 0xFF;

/// HID++ 2.0 error: `feature_index = 0xFF`.
pub const ERROR_20: u8 = 0xFF;
/// HID++ 1.0 error: `sub_id = 0x8F`.
pub const ERROR_10: u8 = 0x8F;

pub const fn report_len(report_id: u8) -> Option<usize> {
    match report_id {
        SHORT => Some(SHORT_LEN),
        LONG => Some(LONG_LEN),
        VERY_LONG => Some(VERY_LONG_LEN),
        _ => None,
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct Report {
    buf: [u8; VERY_LONG_LEN],
    len: usize,
}

impl Report {
    /// A report from raw bytes (with the report ID). An unknown ID or a short buffer gives `None`.
    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        let len = report_len(*bytes.first()?)?;
        if bytes.len() < len {
            return None;
        }
        let mut buf = [0; VERY_LONG_LEN];
        buf[..len].copy_from_slice(&bytes[..len]);
        Some(Self { buf, len })
    }

    /// A new report; the parameters are padded with zeros to the report size.
    pub fn new(report_id: u8, device: u8, sub_id: u8, address: u8, params: &[u8]) -> Self {
        let len = report_len(report_id).expect("valid HID++ report id");
        assert!(
            params.len() <= len - 4,
            "too many params for report {report_id:#04x}"
        );
        let mut buf = [0; VERY_LONG_LEN];
        buf[..4].copy_from_slice(&[report_id, device, sub_id, address]);
        buf[4..4 + params.len()].copy_from_slice(params);
        Self { buf, len }
    }

    pub fn as_bytes(&self) -> &[u8] {
        &self.buf[..self.len]
    }

    pub fn report_id(&self) -> u8 {
        self.buf[0]
    }

    pub fn device(&self) -> u8 {
        self.buf[1]
    }

    /// HID++ 2.0: the feature index; HID++ 1.0: the sub ID.
    pub fn sub_id(&self) -> u8 {
        self.buf[2]
    }

    /// HID++ 2.0: `function << 4 | sw_id`; HID++ 1.0: the register address.
    pub fn address(&self) -> u8 {
        self.buf[3]
    }

    pub fn params(&self) -> &[u8] {
        &self.buf[4..self.len]
    }
}

impl fmt::Debug for Report {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&hex(self.as_bytes()))
    }
}

pub fn hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 3);
    for (i, b) in bytes.iter().enumerate() {
        if i > 0 {
            s.push(' ');
        }
        s.push_str(&format!("{b:02x}"));
    }
    s
}

/// HID++ 2.0 error codes.
pub fn error_20_name(code: u8) -> &'static str {
    match code {
        0x00 => "no error",
        0x01 => "unknown",
        0x02 => "invalid argument",
        0x03 => "out of range",
        0x04 => "hardware error",
        0x05 => "logitech internal",
        0x06 => "invalid feature index",
        0x07 => "invalid function id",
        0x08 => "busy",
        0x09 => "unsupported",
        _ => "?",
    }
}

/// HID++ 1.0 error codes.
pub fn error_10_name(code: u8) -> &'static str {
    match code {
        0x01 => "invalid sub id",
        0x02 => "invalid address",
        0x03 => "invalid value",
        0x04 => "connection fail",
        0x05 => "too many devices",
        0x06 => "already exists",
        0x07 => "busy",
        0x08 => "unknown device",
        0x09 => "resource error",
        0x0A => "request unavailable",
        0x0B => "invalid param value",
        0x0C => "wrong pin code",
        _ => "?",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn long_report_layout() {
        let r = Report::new(LONG, 0x01, 0x00, 0x1A, &[0x00, 0x05]);
        assert_eq!(r.as_bytes().len(), LONG_LEN);
        assert_eq!(&r.as_bytes()[..6], &[0x11, 0x01, 0x00, 0x1A, 0x00, 0x05]);
        assert_eq!(r.address() >> 4, 1);
        assert_eq!(r.params().len(), 16);
    }

    #[test]
    fn parse_rejects_unknown_and_truncated() {
        assert!(Report::from_bytes(&[0x20, 0, 0, 0, 0, 0, 0]).is_none());
        assert!(Report::from_bytes(&[0x11, 0xFF, 0, 0]).is_none());
        let r = Report::from_bytes(&[0x10, 0xFF, 0x8F, 0x00, 0x1A, 0x01, 0x00, 0xEE]).unwrap();
        assert_eq!(r.as_bytes().len(), SHORT_LEN);
        assert_eq!(r.sub_id(), ERROR_10);
    }
}
