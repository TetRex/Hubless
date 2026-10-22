// SPDX-License-Identifier: GPL-3.0-or-later
//! Small Win32 helpers: strings, registry.

use std::ffi::OsStr;
use std::os::windows::ffi::OsStrExt;

use windows::Win32::Foundation::{ERROR_FILE_NOT_FOUND, WIN32_ERROR};
use windows::Win32::System::Registry::{
    HKEY, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, REG_SZ, RRF_RT_REG_DWORD, RRF_RT_REG_SZ,
    RegDeleteKeyValueW, RegGetValueW, RegSetKeyValueW,
};
use windows::core::PCWSTR;

/// UTF-16 with a terminating zero.
pub fn wide(s: impl AsRef<OsStr>) -> Vec<u16> {
    s.as_ref().encode_wide().chain(Some(0)).collect()
}

fn check(err: WIN32_ERROR) -> windows::core::Result<()> {
    err.ok()
}

pub fn hkcu_read_dword(subkey: &str, value: &str) -> Option<u32> {
    let (subkey, value) = (wide(subkey), wide(value));
    let mut data = 0u32;
    let mut size = size_of::<u32>() as u32;
    // SAFETY: the buffer `data` of `size` bytes lives until the end of the call.
    let err = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            PCWSTR(subkey.as_ptr()),
            PCWSTR(value.as_ptr()),
            RRF_RT_REG_DWORD,
            None,
            Some((&raw mut data).cast()),
            Some(&mut size),
        )
    };
    check(err).ok().map(|()| data)
}

pub fn hkcu_read_string(subkey: &str, value: &str) -> Option<String> {
    read_string(HKEY_CURRENT_USER, subkey, value)
}

pub fn hklm_read_string(subkey: &str, value: &str) -> Option<String> {
    read_string(HKEY_LOCAL_MACHINE, subkey, value)
}

fn read_string(root: HKEY, subkey: &str, value: &str) -> Option<String> {
    let (subkey, value) = (wide(subkey), wide(value));
    let mut size = 0u32;
    // SAFETY: the first call only learns the size, the second writes into a buffer of that size.
    unsafe {
        check(RegGetValueW(
            root,
            PCWSTR(subkey.as_ptr()),
            PCWSTR(value.as_ptr()),
            RRF_RT_REG_SZ,
            None,
            None,
            Some(&mut size),
        ))
        .ok()?;
        let mut buf = vec![0u16; (size as usize).div_ceil(2)];
        check(RegGetValueW(
            root,
            PCWSTR(subkey.as_ptr()),
            PCWSTR(value.as_ptr()),
            RRF_RT_REG_SZ,
            None,
            Some(buf.as_mut_ptr().cast()),
            Some(&mut size),
        ))
        .ok()?;
        let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
        Some(String::from_utf16_lossy(&buf[..len]))
    }
}

pub fn hkcu_write_string(subkey: &str, value: &str, data: &str) -> windows::core::Result<()> {
    let (subkey, value, data) = (wide(subkey), wide(value), wide(data));
    // SAFETY: all buffers live until the end of the call; the size in bytes includes the terminating zero.
    check(unsafe {
        RegSetKeyValueW(
            HKEY_CURRENT_USER,
            PCWSTR(subkey.as_ptr()),
            PCWSTR(value.as_ptr()),
            REG_SZ.0,
            Some(data.as_ptr().cast()),
            (data.len() * 2) as u32,
        )
    })
}

/// Deleting a value that doesn't exist counts as success.
pub fn hkcu_delete_value(subkey: &str, value: &str) -> windows::core::Result<()> {
    let (subkey, value) = (wide(subkey), wide(value));
    // SAFETY: the strings live until the end of the call.
    let err = unsafe {
        RegDeleteKeyValueW(
            HKEY_CURRENT_USER,
            PCWSTR(subkey.as_ptr()),
            PCWSTR(value.as_ptr()),
        )
    };
    if err == ERROR_FILE_NOT_FOUND {
        return Ok(());
    }
    check(err)
}
