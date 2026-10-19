// SPDX-License-Identifier: GPL-3.0-or-later
//! `0x0005` DeviceName & Type.

use crate::hidpp::{Device, Error};

pub const ID: u16 = 0x0005;
const GET_NAME_LEN: u8 = 0;
const GET_NAME: u8 = 1;
const GET_TYPE: u8 = 2;

pub fn name(dev: &Device) -> Result<String, Error> {
    let len = dev.call(ID, GET_NAME_LEN, &[])?.params()[0] as usize;
    let mut bytes = Vec::with_capacity(len);
    while bytes.len() < len {
        let r = dev.call(ID, GET_NAME, &[bytes.len() as u8])?;
        let chunk = r.params();
        let take = chunk.len().min(len - bytes.len());
        if take == 0 {
            break;
        }
        bytes.extend_from_slice(&chunk[..take]);
    }
    let end = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
    Ok(String::from_utf8_lossy(&bytes[..end]).into_owned())
}

pub fn kind(dev: &Device) -> Result<(u8, &'static str), Error> {
    let t = dev.call(ID, GET_TYPE, &[])?.params()[0];
    let name = match t {
        0 => "keyboard",
        1 => "remote_control",
        2 => "numpad",
        3 => "mouse",
        4 => "touchpad",
        5 => "trackball",
        6 => "presenter",
        7 => "receiver",
        8 => "headset",
        9 => "webcam",
        10 => "steering_wheel",
        11 => "joystick",
        12 => "gamepad",
        13 => "dock",
        14 => "speaker",
        15 => "microphone",
        _ => "other",
    };
    Ok((t, name))
}
