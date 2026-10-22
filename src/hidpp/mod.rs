// SPDX-License-Identifier: GPL-3.0-or-later
//! The HID++ protocol (1.0 for receivers, 2.0 for devices).

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
    /// No reply (a device behind a receiver may be asleep: that is the "offline" state, not a failure).
    Timeout,
    Disconnected,
    NoHidppCollection,
    Hidpp20(u8),
    Hidpp10(u8),
    /// The feature is missing from the device's table.
    FeatureMissing(u16),
    /// A blacklisted feature (DFU/firmware update): never called.
    Forbidden(u16),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Hid(e) => write!(f, "hid: {e}"),
            Self::Timeout => f.write_str("no reply"),
            Self::Disconnected => f.write_str("device disconnected"),
            Self::NoHidppCollection => f.write_str("no suitable HID++ collection"),
            Self::Hidpp20(c) => write!(
                f,
                "HID++ 2.0 error {c:#04x} ({})",
                message::error_20_name(*c)
            ),
            Self::Hidpp10(c) => write!(
                f,
                "HID++ 1.0 error {c:#04x} ({})",
                message::error_10_name(*c)
            ),
            Self::FeatureMissing(id) => write!(f, "feature {id:#06x} is not supported"),
            Self::Forbidden(id) => write!(f, "feature {id:#06x} is blacklisted"),
        }
    }
}

impl std::error::Error for Error {}

impl From<hidapi::HidError> for Error {
    fn from(e: hidapi::HidError) -> Self {
        Self::Hid(e)
    }
}

/// An HID++ 2.0 device on a transport: its index and the feature table cached for the session.
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

    fn index_of(&self, feature_id: u16) -> Result<u8, Error> {
        if features::is_forbidden(feature_id) {
            return Err(Error::Forbidden(feature_id));
        }
        self.feature(feature_id)
            .map(|f| f.index)
            .ok_or(Error::FeatureMissing(feature_id))
    }

    /// Calls a feature function by the feature's ID (the index comes from the cache).
    pub fn call(&self, feature_id: u16, function: u8, params: &[u8]) -> Result<Report, Error> {
        let index = self.index_of(feature_id)?;
        self.transport.call(self.index, index, function, params)
    }

    /// Performs a write to the device.
    pub fn apply(&self, w: &Write) -> Result<Report, Error> {
        let index = self.index_of(w.feature)?;
        self.transport
            .call_as(w.short, self.index, index, w.function, &w.params)
    }

    /// The packet that [`Device::apply`] will send (sw_id shown as 0).
    pub fn preview(&self, w: &Write) -> Result<Report, Error> {
        let index = self.index_of(w.feature)?;
        self.transport
            .preview(w.short, self.index, index, w.function, &w.params)
    }
}

/// A write to a device: built in advance so that it can be shown before it is sent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Write {
    pub feature: u16,
    pub function: u8,
    pub params: Vec<u8>,
    pub what: String,
    /// Send as a short report (like G Hub) if the parameters fit.
    pub short: bool,
}
