// SPDX-License-Identifier: GPL-3.0-or-later
//! The Logitech "Centurion" protocol (PRO X 2 LIGHTSPEED headset, adapter PID `0x0AF7`).
//!
//! Reference: HeadsetControl (GPL-3.0), `lib/devices/protocols/logitech_centurion_protocol.hpp`.
//! 64-byte frames on a collection with usage page `0xFFA0`:
//! `[0x51][len = payload + 1][flags][payload...]`.
//! A direct request to the adapter: payload `[feature_index][function | sw_id][params...]`.
//! A request to the headset goes through the adapter's bridge feature `0x0003`:
//! `[bridge][0x10 | sw_id][size_hi][size_lo][0x00][sub_index][function | sw_id][params...]`;
//! the adapter first acknowledges (ACK, sw_id = ours), then sends an event with the reply (sw_id = 0).

use std::time::{Duration, Instant};

use hidapi::{HidApi, HidDevice};

use crate::hidpp::message::hex;

pub const USAGE_PAGE: u16 = 0xFFA0;
const REPORT_ID: u8 = 0x51;
const FRAME_LEN: usize = 64;
const SW_ID: u8 = 0x01;
const TIMEOUT: Duration = Duration::from_secs(1);

pub const ROOT: u16 = 0x0000;
pub const FEATURE_SET: u16 = 0x0001;
pub const BRIDGE: u16 = 0x0003;
pub const BATTERY_SOC: u16 = 0x0104;
pub const AUTO_SLEEP: u16 = 0x0108;
pub const SIDETONE: u16 = 0x0604;

/// Reading and writing the value of `SIDETONE` and `AUTO_SLEEP` (per HeadsetControl).
const FN_GET: u8 = 0x00;
const FN_SET: u8 = 0x10;
/// Sidetone: the microphone ID and the device's scale maximum (HeadsetControl, PRO X 2).
const SIDETONE_MIC: u8 = 0x01;
pub const SIDETONE_MAX: u8 = 100;

const ROOT_GET_FEATURE: u8 = 0x00;
const FS_GET_COUNT: u8 = 0x00;
const FS_GET_FEATURE_ID: u8 = 0x10;
const BRIDGE_STATUS: u8 = 0x00;
const BRIDGE_SEND: u8 = 0x10;
const BATTERY_GET: u8 = 0x00;

/// Feature names (reference: HeadsetControl). For display only.
pub fn feature_name(id: u16) -> Option<&'static str> {
    Some(match id {
        0x0000 => "ROOT",
        0x0001 => "FEATURE_SET",
        0x0003 => "CENTURION_BRIDGE",
        0x0104 => "BATTERY_SOC",
        0x0108 => "AUTO_SLEEP",
        0x020D => "ADVANCED_PARA_EQ",
        0x0601 => "MIC_MUTE_LED",
        0x0604 => "SIDETONE",
        0x060B => "VOICE_PROMPTS",
        0x0621 => "LIGHTING",
        0x0636 => "ONBOARD_EQ",
        _ => return None,
    })
}

#[derive(Debug)]
pub enum Error {
    Hid(hidapi::HidError),
    Timeout,
    /// The adapter replied that the headset isn't connected (it is off).
    Offline,
    /// An error in the HID++ 2.0 format: `[0xFF, feature, function|sw, code]`.
    Rejected(u8),
    FeatureMissing(u16),
    Malformed,
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Hid(e) => write!(f, "hid: {e}"),
            Self::Timeout => f.write_str("no reply"),
            Self::Offline => f.write_str("headset not connected"),
            Self::Rejected(c) => write!(f, "rejected, code {c:#04x}"),
            Self::FeatureMissing(id) => write!(f, "feature {id:#06x} is not supported"),
            Self::Malformed => f.write_str("unexpected reply format"),
        }
    }
}

impl From<hidapi::HidError> for Error {
    fn from(e: hidapi::HidError) -> Self {
        Self::Hid(e)
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Feature {
    pub index: u8,
    pub id: u16,
    pub version: u8,
    pub flags: u8,
}

#[derive(Debug, Clone)]
pub struct Battery {
    pub percent: u8,
    /// 1 or 2 means charging (per HeadsetControl), anything else is on battery.
    pub charging_state: u8,
    pub raw: Vec<u8>,
}

/// A frame: `[0x51][len][flags=0][payload...]`, padded with zeros to 64 bytes.
pub fn build_frame(payload: &[u8]) -> [u8; FRAME_LEN] {
    assert!(payload.len() <= FRAME_LEN - 3, "payload too long");
    let mut frame = [0u8; FRAME_LEN];
    frame[0] = REPORT_ID;
    frame[1] = payload.len() as u8 + 1;
    frame[3..3 + payload.len()].copy_from_slice(payload);
    frame
}

/// How many bytes of a message fit in a frame (as in G Hub: length `0x3d` = flags + 60).
const CHUNK: usize = 60;

/// A message into frames: a long one is split into 60-byte chunks with flags `(number << 1) | more_follow`
/// (G Hub equalizer capture 2026-10-20: `01 03 05 07 08`). A short one is a single frame with flags 0.
pub fn build_frames(payload: &[u8]) -> Vec<[u8; FRAME_LEN]> {
    let chunks: Vec<&[u8]> = payload.chunks(CHUNK).collect();
    let last = chunks.len().saturating_sub(1);
    chunks
        .iter()
        .enumerate()
        .map(|(i, chunk)| {
            let mut frame = build_frame(chunk);
            frame[2] = ((i as u8) << 1) | u8::from(i < last);
            frame
        })
        .collect()
}

/// The frame payload (after the length and flags bytes).
pub fn parse_frame(frame: &[u8]) -> Option<&[u8]> {
    if frame.len() < 3 || frame[0] != REPORT_ID {
        return None;
    }
    let len = frame[1] as usize;
    if len <= 1 || len + 2 > frame.len() {
        return None;
    }
    Some(&frame[3..len + 2])
}

pub struct Adapter {
    dev: HidDevice,
    bridge: Option<u8>,
    /// Indices of the headset's features behind the bridge (cached for the session).
    headset_index: std::collections::HashMap<u16, u8>,
    /// All frames received during requests (for the dump).
    pub log: Vec<String>,
}

/// A write to the headset through the bridge (the counterpart of `hidpp::Write`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Write {
    pub feature: u16,
    pub function: u8,
    pub params: Vec<u8>,
    pub what: String,
}

/// Sidetone (how loud you hear your own voice in the headphones), 0–100.
pub fn set_sidetone(state: &[u8], level: u8) -> Write {
    let level = level.min(SIDETONE_MAX);
    // HeadsetControl: if the read reply is ≥ 4 bytes, it is the v2 format with a 0xFF byte.
    let params = if state.len() >= 4 {
        vec![SIDETONE_MIC, 0xFF, level]
    } else {
        vec![SIDETONE_MIC, level]
    };
    Write {
        feature: SIDETONE,
        function: FN_SET,
        params,
        what: format!("sidetone {level}"),
    }
}

/// Auto power-off after `minutes` minutes without sound (0 = never, per HeadsetControl).
pub fn set_auto_sleep(minutes: u8) -> Write {
    Write {
        feature: AUTO_SLEEP,
        function: FN_SET,
        params: vec![minutes],
        what: if minutes == 0 {
            "auto-off: never".into()
        } else {
            format!("auto-off after {minutes} min")
        },
    }
}

impl Adapter {
    pub fn open(api: &HidApi, path: &std::ffi::CStr) -> Result<Self, Error> {
        Ok(Self {
            dev: api.open_path(path)?,
            bridge: None,
            headset_index: std::collections::HashMap::new(),
            log: Vec::new(),
        })
    }

    /// The first Centurion adapter (a `0xFFA0` collection on a Logitech device) with a bridge found,
    /// and its name (the USB product string).
    pub fn find(api: &HidApi) -> Option<(Self, String)> {
        api.device_list()
            .filter(|d| {
                d.vendor_id() == crate::hidpp::transport::LOGITECH_VID
                    && d.usage_page() == USAGE_PAGE
            })
            .find_map(|info| {
                let mut a = Self::open(api, info.path())
                    .inspect_err(|e| tracing::warn!("гарнитура: не открыть адаптер: {e}"))
                    .ok()?;
                a.find_bridge()
                    .inspect_err(|e| tracing::warn!("гарнитура: нет моста: {e}"))
                    .ok()?;
                let name = info
                    .product_string()
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(str::to_owned)
                    .or_else(|| {
                        crate::devices::registry::lookup(info.product_id())
                            .map(|k| k.name.to_owned())
                    })
                    .unwrap_or_default();
                Some((a, name))
            })
    }

    /// The next asynchronous frame (payload), or `None` if nothing came within `timeout`.
    pub fn next_event(&mut self, timeout: Duration) -> Result<Option<Vec<u8>>, Error> {
        match self.recv(Instant::now() + timeout) {
            Ok(p) => Ok(Some(p)),
            Err(Error::Timeout) => Ok(None),
            Err(e) => Err(e),
        }
    }

    /// The bridge event about the headset connecting: `[bridge, 0x00, 0x00, connected]`.
    pub fn connection_event(&self, p: &[u8]) -> Option<bool> {
        let bridge = self.bridge?;
        (p.len() >= 4 && p[0] == bridge && p[1] == 0x00 && p[2] == 0x00).then(|| p[3] != 0)
    }

    /// Whether the headset is connected to the adapter (by the bridge state).
    pub fn headset_connected(&mut self) -> Result<bool, Error> {
        let s = self.bridge_status()?;
        Ok(s.len() >= 4 && s[3] != 0)
    }

    /// The index of a headset feature (cached).
    fn index_of(&mut self, id: u16) -> Result<u8, Error> {
        if let Some(&i) = self.headset_index.get(&id) {
            return Ok(i);
        }
        let i = self.headset_feature(id)?.index;
        self.headset_index.insert(id, i);
        Ok(i)
    }

    /// Sidetone level 0–100: the last byte of the fn0 reply (`01 01 32` → 50, verified 2026-10-19).
    pub fn sidetone_level(&mut self) -> Result<u8, Error> {
        self.sidetone()?.last().copied().ok_or(Error::Malformed)
    }

    /// Auto power-off in minutes, 0 = never: the first byte of the fn0 reply (verified 2026-10-19).
    pub fn auto_sleep_minutes(&mut self) -> Result<u8, Error> {
        self.auto_sleep()?.first().copied().ok_or(Error::Malformed)
    }

    /// The raw `SIDETONE` fn0 reply.
    pub fn sidetone(&mut self) -> Result<Vec<u8>, Error> {
        let i = self.index_of(SIDETONE)?;
        self.bridged(i, FN_GET, &[])
    }

    /// The raw `AUTO_SLEEP` fn0 reply.
    pub fn auto_sleep(&mut self) -> Result<Vec<u8>, Error> {
        let i = self.index_of(AUTO_SLEEP)?;
        self.bridged(i, FN_GET, &[])
    }

    /// The frames that will go to the adapter for a write (without sending them).
    pub fn preview(&mut self, w: &Write) -> Result<Vec<[u8; FRAME_LEN]>, Error> {
        let i = self.index_of(w.feature)?;
        let payload = self.bridge_payload(i, w.function, &w.params)?;
        Ok(build_frames(&payload))
    }

    /// The current curve of the headset's built-in equalizer.
    pub fn eq(&mut self) -> Result<crate::headset_eq::Eq, Error> {
        let i = self.index_of(crate::headset_eq::FEATURE)?;
        let (function, params) = crate::headset_eq::read_request();
        let r = self.bridged(i, function, &params)?;
        crate::headset_eq::parse(&r).ok_or(Error::Malformed)
    }

    /// Sends a write; returns the headset's reply (the data after the index and function).
    pub fn apply(&mut self, w: &Write) -> Result<Vec<u8>, Error> {
        let i = self.index_of(w.feature)?;
        self.bridged(i, w.function, &w.params)
    }

    fn send(&mut self, payload: &[u8]) -> Result<(), Error> {
        for frame in build_frames(payload) {
            let len = frame[1] as usize + 2;
            tracing::debug!(target: "hubless::centurion", "→ {}", hex(&frame[..len]));
            self.dev.write(&frame)?;
        }
        Ok(())
    }

    /// The next payload before the deadline.
    fn recv(&mut self, deadline: Instant) -> Result<Vec<u8>, Error> {
        let mut buf = [0u8; FRAME_LEN];
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Err(Error::Timeout);
            }
            let n = self
                .dev
                .read_timeout(&mut buf, left.as_millis().max(1) as i32)?;
            if n == 0 {
                continue;
            }
            if let Some(p) = parse_frame(&buf[..n]) {
                tracing::debug!(target: "hubless::centurion", "← {}", hex(p));
                self.log.push(hex(p));
                return Ok(p.to_vec());
            }
        }
    }

    /// A direct request to an adapter feature; the reply is `[feature_index][function|sw][data...]`.
    pub fn direct(
        &mut self,
        feature_index: u8,
        function: u8,
        params: &[u8],
    ) -> Result<Vec<u8>, Error> {
        let func_sw = (function & 0xF0) | SW_ID;
        let mut payload = vec![feature_index, func_sw];
        payload.extend_from_slice(params);
        self.send(&payload)?;
        let deadline = Instant::now() + TIMEOUT;
        loop {
            let r = self.recv(deadline)?;
            if r.len() >= 4 && r[0] == 0xFF && r[1] == feature_index && r[2] == func_sw {
                return Err(Error::Rejected(r[3]));
            }
            if r.len() >= 2 && r[0] == feature_index && r[1] == func_sw {
                return Ok(r[2..].to_vec());
            }
        }
    }

    /// The adapter feature index by ID (Root.GetFeature: `[index, type, version]`).
    pub fn get_feature(&mut self, id: u16) -> Result<Feature, Error> {
        let r = self.direct(0x00, ROOT_GET_FEATURE, &id.to_be_bytes())?;
        match r.as_slice() {
            [0, ..] if id != ROOT => Err(Error::FeatureMissing(id)),
            [index, flags, version, ..] => Ok(Feature {
                index: *index,
                id,
                version: *version,
                flags: *flags,
            }),
            _ => Err(Error::Malformed),
        }
    }

    /// Finds the bridge and remembers its index.
    pub fn find_bridge(&mut self) -> Result<u8, Error> {
        let f = self.get_feature(BRIDGE)?;
        self.bridge = Some(f.index);
        Ok(f.index)
    }

    /// The bridge state (a direct request): per HeadsetControl, `03 e8 00 01 01` means the headset
    /// is connected, `03 e8 00 00 01` that it isn't.
    pub fn bridge_status(&mut self) -> Result<Vec<u8>, Error> {
        let bridge = self.bridge.ok_or(Error::FeatureMissing(BRIDGE))?;
        self.direct(bridge, BRIDGE_STATUS, &[])
    }

    /// A request to the headset through the bridge.
    pub fn bridged(
        &mut self,
        sub_index: u8,
        function: u8,
        params: &[u8],
    ) -> Result<Vec<u8>, Error> {
        let bridge = self.bridge.ok_or(Error::FeatureMissing(BRIDGE))?;
        let send_fn = BRIDGE_SEND | SW_ID;
        let payload = self.bridge_payload(sub_index, function, params)?;
        self.send(&payload)?;

        let deadline = Instant::now() + TIMEOUT;
        loop {
            let r = match self.recv(deadline) {
                Err(Error::Timeout) => {
                    // An ACK without a reply, or silence: ask the bridge whether the headset is there.
                    return match self.bridge_status() {
                        Ok(s) if s.len() >= 4 && s[2] == 0 && s[3] == 0 => Err(Error::Offline),
                        _ => Err(Error::Timeout),
                    };
                }
                other => other?,
            };
            if r.len() >= 4 && r[0] == 0xFF && r[1] == bridge && r[2] == send_fn {
                return match self.bridge_status() {
                    Ok(s) if s.len() >= 4 && s[2] == 0 && s[3] == 0 => Err(Error::Offline),
                    _ => Err(Error::Rejected(r[3])),
                };
            }
            if r.len() < 2 || r[0] != bridge {
                continue;
            }
            // The headset connecting/disconnecting event: [bridge, 0x00, 0x00, connected].
            if r[1] == 0x00 {
                if r.len() >= 4 && r[3] == 0 {
                    return Err(Error::Offline);
                }
                continue;
            }
            if r[1] >> 4 != BRIDGE_SEND >> 4 || r[1] & 0x0F != 0 {
                continue; // ACK (sw_id = ours) or someone else's
            }
            // [bridge, 0x10, len_hi, len_lo, 0x00, sub_index | 0xFF, ...]
            if r.len() < 7 || r[4] != 0x00 {
                continue;
            }
            if r[5] == 0xFF && r[6] == sub_index {
                return Err(Error::Rejected(r.get(8).copied().unwrap_or(0)));
            }
            if r[5] == sub_index {
                return Ok(r[7..].to_vec());
            }
        }
    }

    /// The payload of a request to the headset through the bridge:
    /// `[bridge][0x10|sw][size_hi][size_lo][0x00][sub_index][function|sw][params...]`.
    fn bridge_payload(&self, sub_index: u8, function: u8, params: &[u8]) -> Result<Vec<u8>, Error> {
        let bridge = self.bridge.ok_or(Error::FeatureMissing(BRIDGE))?;
        let mut sub = vec![0x00, sub_index, (function & 0xF0) | SW_ID];
        sub.extend_from_slice(params);
        let mut payload = vec![
            bridge,
            BRIDGE_SEND | SW_ID,
            (sub.len() >> 8) as u8 & 0x0F,
            sub.len() as u8,
        ];
        payload.extend_from_slice(&sub);
        Ok(payload)
    }

    /// The headset feature index by ID through the bridge.
    pub fn headset_feature(&mut self, id: u16) -> Result<Feature, Error> {
        let r = self.bridged(0x00, ROOT_GET_FEATURE, &id.to_be_bytes())?;
        match r.as_slice() {
            [0, ..] if id != ROOT => Err(Error::FeatureMissing(id)),
            [index, flags, version, ..] => Ok(Feature {
                index: *index,
                id,
                version: *version,
                flags: *flags,
            }),
            [index, ..] => Ok(Feature {
                index: *index,
                id,
                version: 0,
                flags: 0,
            }),
            _ => Err(Error::Malformed),
        }
    }

    /// The adapter's feature table (FeatureSet directly).
    pub fn adapter_features(&mut self) -> Result<Vec<Feature>, Error> {
        let fs = self.get_feature(FEATURE_SET)?;
        let count = *self
            .direct(fs.index, FS_GET_COUNT, &[])?
            .first()
            .ok_or(Error::Malformed)?;
        let mut list = Vec::new();
        for index in 0..count {
            let r = self.direct(fs.index, FS_GET_FEATURE_ID, &[index])?;
            list.push(parse_feature_entry(index, &r)?);
        }
        Ok(list)
    }

    /// The headset's feature table (FeatureSet through the bridge).
    pub fn headset_features(&mut self) -> Result<Vec<Feature>, Error> {
        let fs = self.headset_feature(FEATURE_SET)?;
        let count = *self
            .bridged(fs.index, FS_GET_COUNT, &[])?
            .first()
            .ok_or(Error::Malformed)?;
        let mut list = Vec::new();
        for index in 0..count {
            let r = self.bridged(fs.index, FS_GET_FEATURE_ID, &[index])?;
            list.push(parse_feature_entry(index, &r)?);
        }
        Ok(list)
    }

    pub fn battery(&mut self) -> Result<Battery, Error> {
        let f = self.headset_feature(BATTERY_SOC)?;
        let r = self.bridged(f.index, BATTERY_GET, &[])?;
        let percent = *r.first().ok_or(Error::Malformed)?;
        if percent > 100 {
            return Err(Error::Malformed);
        }
        Ok(Battery {
            percent,
            charging_state: r.get(2).copied().unwrap_or(0),
            raw: r,
        })
    }
}

/// The FeatureSet.GetFeatureID reply on Centurion: `[echo index?][id_hi][id_lo][version][flags]`
/// (layout per HeadsetControl, which takes the ID from bytes 1..3).
fn parse_feature_entry(index: u8, r: &[u8]) -> Result<Feature, Error> {
    if r.len() < 3 {
        return Err(Error::Malformed);
    }
    Ok(Feature {
        index,
        id: u16::from_be_bytes([r[1], r[2]]),
        version: r.get(3).copied().unwrap_or(0),
        flags: r.get(4).copied().unwrap_or(0),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn long_messages_are_split_like_g_hub() {
        // An equalizer write: 4 bridge bytes + 256 = 260 bytes → frames 60×4 + 20.
        let payload: Vec<u8> = (0..260).map(|i| i as u8).collect();
        let frames = build_frames(&payload);
        let flags: Vec<u8> = frames.iter().map(|f| f[2]).collect();
        assert_eq!(flags, vec![0x01, 0x03, 0x05, 0x07, 0x08]);
        assert_eq!(frames[0][1], 0x3d);
        assert_eq!(frames[4][1], 0x15);
        assert_eq!(frames[1][3], 60);
        assert_eq!(build_frames(&[1, 2, 3]).len(), 1);
        assert_eq!(build_frames(&[1, 2, 3])[0][2], 0);
    }

    #[test]
    fn frame_roundtrip() {
        let f = build_frame(&[0x00, 0x01, 0x00, 0x03]);
        assert_eq!(&f[..7], &[0x51, 0x05, 0x00, 0x00, 0x01, 0x00, 0x03]);
        assert_eq!(f.len(), 64);
        assert_eq!(parse_frame(&f), Some(&[0x00, 0x01, 0x00, 0x03][..]));
    }

    #[test]
    fn parse_rejects_bad_frames() {
        assert_eq!(parse_frame(&[0x50, 0x05, 0, 1, 2, 3, 4]), None);
        assert_eq!(parse_frame(&[0x51, 0x01, 0]), None);
        assert_eq!(parse_frame(&[0x51, 0x40, 0, 1]), None);
    }
}
