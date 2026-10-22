// SPDX-License-Identifier: GPL-3.0-or-later
//! `0x0000` Root and `0x0001` FeatureSet.

use super::Error;
use super::transport::Transport;

/// Root always lives at index 0.
const ROOT_INDEX: u8 = 0x00;
const ROOT_GET_FEATURE: u8 = 0;
const ROOT_PING: u8 = 1;

pub const FEATURE_SET: u16 = 0x0001;
const FS_GET_COUNT: u8 = 0;
const FS_GET_FEATURE_ID: u8 = 1;

/// An arbitrary byte the device must return in its reply to a ping.
const PING_DATA: u8 = 0x5A;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FeatureEntry {
    pub index: u8,
    pub id: u16,
    pub flags: u8,
    pub version: u8,
}

/// Feature type flags (the `type` byte in the FeatureSet/GetFeature reply).
pub fn flag_names(flags: u8) -> Vec<&'static str> {
    [
        (0x80, "obsolete"),
        (0x40, "hidden"),
        (0x20, "engineering"),
        (0x10, "manuf_deactivatable"),
        (0x08, "compliance_deactivatable"),
    ]
    .into_iter()
    .filter(|(bit, _)| flags & bit != 0)
    .map(|(_, name)| name)
    .collect()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Protocol {
    /// HID++ 2.0+ (major, minor).
    V2(u8, u8),
    /// Answers as HID++ 1.0 (a receiver or an old device).
    V1,
}

/// Ping through Root: the protocol version. The 1.0 error "invalid sub id" means HID++ 1.0.
pub fn ping(t: &Transport, device: u8) -> Result<Protocol, Error> {
    match t.call_short(device, ROOT_INDEX, ROOT_PING, &[0, 0, PING_DATA]) {
        Ok(r) => {
            let p = r.params();
            Ok(Protocol::V2(p[0], p[1]))
        }
        Err(Error::Hidpp10(0x01)) => Ok(Protocol::V1),
        Err(e) => Err(e),
    }
}

/// Root.GetFeature: the feature index, or `None` if it isn't supported.
pub fn get_feature(t: &Transport, device: u8, id: u16) -> Result<Option<FeatureEntry>, Error> {
    let r = t.call(device, ROOT_INDEX, ROOT_GET_FEATURE, &id.to_be_bytes())?;
    let p = r.params();
    Ok((p[0] != 0).then_some(FeatureEntry {
        index: p[0],
        id,
        flags: p[1],
        version: p[2],
    }))
}

/// The full feature table through FeatureSet (index 0 is Root itself).
pub fn enumerate(t: &Transport, device: u8) -> Result<Vec<FeatureEntry>, Error> {
    let fs = get_feature(t, device, FEATURE_SET)?.ok_or(Error::FeatureMissing(FEATURE_SET))?;
    let count = t.call(device, fs.index, FS_GET_COUNT, &[])?.params()[0];
    let mut table = Vec::with_capacity(count as usize + 1);
    for index in 0..=count {
        let r = t.call(device, fs.index, FS_GET_FEATURE_ID, &[index])?;
        let p = r.params();
        table.push(FeatureEntry {
            index,
            id: u16::from_be_bytes([p[0], p[1]]),
            flags: p[2],
            version: p[3],
        });
    }
    Ok(table)
}
