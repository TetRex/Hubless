// SPDX-License-Identifier: GPL-3.0-or-later
//! Протокол Logitech «Centurion» (гарнитура PRO X 2 LIGHTSPEED, адаптер PID `0x0AF7`).
//!
//! Справочник — HeadsetControl (GPL-3.0), `lib/devices/protocols/logitech_centurion_protocol.hpp`.
//! Кадры по 64 байта на коллекции usage page `0xFFA0`:
//! `[0x51][len = payload + 1][flags][payload...]`.
//! Прямой запрос к адаптеру: payload `[feature_index][function | sw_id][params...]`.
//! Запрос к гарнитуре идёт через фичу-мост `0x0003` адаптера:
//! `[bridge][0x10 | sw_id][size_hi][size_lo][0x00][sub_index][function | sw_id][params...]`;
//! адаптер сначала подтверждает (ACK, sw_id = наш), затем присылает событие с ответом (sw_id = 0).

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

const ROOT_GET_FEATURE: u8 = 0x00;
const FS_GET_COUNT: u8 = 0x00;
const FS_GET_FEATURE_ID: u8 = 0x10;
const BRIDGE_STATUS: u8 = 0x00;
const BRIDGE_SEND: u8 = 0x10;
const BATTERY_GET: u8 = 0x00;

/// Имена фич (справочник — HeadsetControl). Только для отображения.
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
    /// Адаптер ответил, что гарнитура не подключена (выключена).
    Offline,
    /// Ошибка в формате HID++ 2.0: `[0xFF, feature, function|sw, code]`.
    Rejected(u8),
    FeatureMissing(u16),
    Malformed,
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Hid(e) => write!(f, "hid: {e}"),
            Self::Timeout => f.write_str("нет ответа"),
            Self::Offline => f.write_str("гарнитура не подключена"),
            Self::Rejected(c) => write!(f, "отклонено, код {c:#04x}"),
            Self::FeatureMissing(id) => write!(f, "фича {id:#06x} не поддерживается"),
            Self::Malformed => f.write_str("неожиданный формат ответа"),
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
    /// 1 или 2 — заряжается (по HeadsetControl), остальное — от батареи.
    pub charging_state: u8,
    pub raw: Vec<u8>,
}

/// Кадр: `[0x51][len][flags=0][payload...]`, дополненный нулями до 64 байт.
pub fn build_frame(payload: &[u8]) -> [u8; FRAME_LEN] {
    assert!(payload.len() <= FRAME_LEN - 3, "payload too long");
    let mut frame = [0u8; FRAME_LEN];
    frame[0] = REPORT_ID;
    frame[1] = payload.len() as u8 + 1;
    frame[3..3 + payload.len()].copy_from_slice(payload);
    frame
}

/// Payload кадра (после байтов длины и флагов).
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
    /// Все кадры, полученные во время запросов (для дампа).
    pub log: Vec<String>,
}

impl Adapter {
    pub fn open(api: &HidApi, path: &std::ffi::CStr) -> Result<Self, Error> {
        Ok(Self {
            dev: api.open_path(path)?,
            bridge: None,
            log: Vec::new(),
        })
    }

    fn send(&mut self, payload: &[u8]) -> Result<(), Error> {
        let frame = build_frame(payload);
        tracing::debug!(target: "hubless::centurion", "→ {}", hex(&frame[..payload.len() + 3]));
        self.dev.write(&frame)?;
        Ok(())
    }

    /// Следующий payload до дедлайна.
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

    /// Прямой запрос к фиче адаптера; ответ `[feature_index][function|sw][data...]`.
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

    /// Индекс фичи адаптера по ID (Root.GetFeature: `[index, type, version]`).
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

    /// Найти мост и запомнить его индекс.
    pub fn find_bridge(&mut self) -> Result<u8, Error> {
        let f = self.get_feature(BRIDGE)?;
        self.bridge = Some(f.index);
        Ok(f.index)
    }

    /// Состояние моста (прямой запрос): по HeadsetControl `03 e8 00 01 01` — гарнитура
    /// подключена, `03 e8 00 00 01` — нет.
    pub fn bridge_status(&mut self) -> Result<Vec<u8>, Error> {
        let bridge = self.bridge.ok_or(Error::FeatureMissing(BRIDGE))?;
        self.direct(bridge, BRIDGE_STATUS, &[])
    }

    /// Запрос к гарнитуре через мост.
    pub fn bridged(
        &mut self,
        sub_index: u8,
        function: u8,
        params: &[u8],
    ) -> Result<Vec<u8>, Error> {
        let bridge = self.bridge.ok_or(Error::FeatureMissing(BRIDGE))?;
        let mut sub = vec![0x00, sub_index, (function & 0xF0) | SW_ID];
        sub.extend_from_slice(params);
        assert!(
            sub.len() <= 56,
            "fragmented bridge messages are not needed for reads"
        );
        let send_fn = BRIDGE_SEND | SW_ID;
        let mut payload = vec![
            bridge,
            send_fn,
            (sub.len() >> 8) as u8 & 0x0F,
            sub.len() as u8,
        ];
        payload.extend_from_slice(&sub);
        self.send(&payload)?;

        let deadline = Instant::now() + TIMEOUT;
        loop {
            let r = match self.recv(deadline) {
                Err(Error::Timeout) => {
                    // ACK без ответа или тишина: спросим мост, есть ли гарнитура.
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
            // Событие подключения/отключения гарнитуры: [bridge, 0x00, 0x00, connected].
            if r[1] == 0x00 {
                if r.len() >= 4 && r[3] == 0 {
                    return Err(Error::Offline);
                }
                continue;
            }
            if r[1] >> 4 != BRIDGE_SEND >> 4 || r[1] & 0x0F != 0 {
                continue; // ACK (sw_id = наш) или чужое
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

    /// Индекс фичи гарнитуры по ID через мост.
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

    /// Таблица фич адаптера (FeatureSet напрямую).
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

    /// Таблица фич гарнитуры (FeatureSet через мост).
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

/// Ответ FeatureSet.GetFeatureID у Centurion: `[echo index?][id_hi][id_lo][version][flags]`
/// (раскладка по HeadsetControl, который берёт ID из байтов 1..3).
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
