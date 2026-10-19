// SPDX-License-Identifier: GPL-3.0-or-later
//! Протокол HID++ (1.0 — ресиверы, 2.0 — устройства).

pub mod features;
pub mod message;
pub mod receiver;
pub mod root;
pub mod transport;

use std::collections::HashMap;
use std::fmt;

use message::Report;
use root::FeatureEntry;
use transport::Transport;

#[derive(Debug)]
pub enum Error {
    Hid(hidapi::HidError),
    /// Нет ответа (устройство за ресивером может спать — это состояние «offline», не сбой).
    Timeout,
    Disconnected,
    NoHidppCollection,
    Hidpp20(u8),
    Hidpp10(u8),
    /// Фича отсутствует в таблице устройства.
    FeatureMissing(u16),
    /// Фича из чёрного списка (DFU/обновление прошивки) — никогда не вызывается.
    Forbidden(u16),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Hid(e) => write!(f, "hid: {e}"),
            Self::Timeout => f.write_str("нет ответа"),
            Self::Disconnected => f.write_str("устройство отключено"),
            Self::NoHidppCollection => f.write_str("нет подходящей HID++ коллекции"),
            Self::Hidpp20(c) => write!(
                f,
                "HID++ 2.0 ошибка {c:#04x} ({})",
                message::error_20_name(*c)
            ),
            Self::Hidpp10(c) => write!(
                f,
                "HID++ 1.0 ошибка {c:#04x} ({})",
                message::error_10_name(*c)
            ),
            Self::FeatureMissing(id) => write!(f, "фича {id:#06x} не поддерживается"),
            Self::Forbidden(id) => write!(f, "фича {id:#06x} в чёрном списке"),
        }
    }
}

impl std::error::Error for Error {}

impl From<hidapi::HidError> for Error {
    fn from(e: hidapi::HidError) -> Self {
        Self::Hid(e)
    }
}

/// Устройство HID++ 2.0 на транспорте: индекс и кеш таблицы фич на время сессии.
pub struct Device<'t> {
    pub transport: &'t Transport,
    pub index: u8,
    features: HashMap<u16, FeatureEntry>,
}

impl<'t> Device<'t> {
    pub fn new(transport: &'t Transport, index: u8, table: &[FeatureEntry]) -> Self {
        Self {
            transport,
            index,
            features: table.iter().map(|f| (f.id, *f)).collect(),
        }
    }

    pub fn feature(&self, id: u16) -> Option<&FeatureEntry> {
        self.features.get(&id)
    }

    /// Вызов функции фичи по её ID (индекс берётся из кеша).
    pub fn call(&self, feature_id: u16, function: u8, params: &[u8]) -> Result<Report, Error> {
        if features::is_forbidden(feature_id) {
            return Err(Error::Forbidden(feature_id));
        }
        let f = self
            .feature(feature_id)
            .ok_or(Error::FeatureMissing(feature_id))?;
        self.transport.call(self.index, f.index, function, params)
    }
}
