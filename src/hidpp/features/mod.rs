// SPDX-License-Identifier: GPL-3.0-or-later
//! Фичи HID++ 2.0: по файлу на фичу, плюс справочник имён.

pub mod battery;
pub mod brightness;
pub mod device_info;
pub mod device_name;
pub mod dpi;
pub mod onboard;
pub mod report_rate;
pub mod rgb_effects;

/// DFU и обновление прошивки — никогда не вызывать.
pub fn is_forbidden(id: u16) -> bool {
    matches!(id, 0x00C0..=0x00C3 | 0x00D0)
}

/// Имена известных фич (справочник — Solaar). Только для отображения в дампе.
pub fn name(id: u16) -> Option<&'static str> {
    Some(match id {
        0x0000 => "ROOT",
        0x0001 => "FEATURE_SET",
        0x0002 => "FEATURE_INFO",
        0x0003 => "DEVICE_FW_VERSION",
        0x0004 => "DEVICE_UNIT_ID",
        0x0005 => "DEVICE_NAME",
        0x0006 => "DEVICE_GROUPS",
        0x0007 => "DEVICE_FRIENDLY_NAME",
        0x0008 => "KEEP_ALIVE",
        0x0020 => "CONFIG_CHANGE",
        0x0021 => "CRYPTO_ID",
        0x0080 => "TARGET_SOFTWARE",
        0x0081 => "WIRELESS_SIGNAL_STRENGTH",
        0x00C0 => "DFUCONTROL_LEGACY",
        0x00C1 => "DFUCONTROL_UNSIGNED",
        0x00C2 => "DFUCONTROL_SIGNED",
        0x00C3 => "DFUCONTROL",
        0x00D0 => "DFU",
        0x1000 => "BATTERY_STATUS",
        0x1001 => "BATTERY_VOLTAGE",
        0x1004 => "UNIFIED_BATTERY",
        0x1010 => "CHARGING_CONTROL",
        0x1300 => "LED_CONTROL",
        0x1802 => "DEVICE_RESET",
        0x1805 => "OOBSTATE",
        0x1806 => "CONFIG_DEVICE_PROPS",
        0x1814 => "CHANGE_HOST",
        0x1815 => "HOSTS_INFO",
        0x1981 => "BACKLIGHT",
        0x1982 => "BACKLIGHT2",
        0x1983 => "BACKLIGHT3",
        0x1990 => "ILLUMINATION",
        0x1B00 => "REPROG_CONTROLS",
        0x1B04 => "REPROG_CONTROLS_V4",
        0x1BC0 => "REPORT_HID_USAGE",
        0x1D4B => "WIRELESS_DEVICE_STATUS",
        0x1DF0 => "REMAINING_PAIRING",
        0x1E00 => "ENABLE_HIDDEN_FEATURES",
        0x2100 => "VERTICAL_SCROLLING",
        0x2110 => "SMART_SHIFT",
        0x2111 => "SMART_SHIFT_ENHANCED",
        0x2120 => "HI_RES_SCROLLING",
        0x2121 => "HIRES_WHEEL",
        0x2130 => "LOWRES_WHEEL",
        0x2150 => "THUMB_WHEEL",
        0x2200 => "MOUSE_POINTER",
        0x2201 => "ADJUSTABLE_DPI",
        0x2202 => "EXTENDED_ADJUSTABLE_DPI",
        0x2205 => "POINTER_SPEED",
        0x2230 => "ANGLE_SNAPPING",
        0x2240 => "SURFACE_TUNING",
        0x2400 => "HYBRID_TRACKING",
        0x40A0 => "FN_INVERSION",
        0x40A2 => "NEW_FN_INVERSION",
        0x4100 => "ENCRYPTION",
        0x4220 => "LOCK_KEY_STATE",
        0x4520 => "KEYBOARD_LAYOUT",
        0x4521 => "KEYBOARD_DISABLE_KEYS",
        0x4522 => "KEYBOARD_DISABLE_BY_USAGE",
        0x4530 => "DUALPLATFORM",
        0x4531 => "MULTIPLATFORM",
        0x4540 => "KEYBOARD_LAYOUT_2",
        0x8010 => "GKEY",
        0x8020 => "MKEYS",
        0x8030 => "MR",
        0x8040 => "BRIGHTNESS_CONTROL",
        0x8060 => "REPORT_RATE",
        0x8061 => "EXTENDED_ADJUSTABLE_REPORT_RATE",
        0x8070 => "COLOR_LED_EFFECTS",
        0x8071 => "RGB_EFFECTS",
        0x8080 => "PER_KEY_LIGHTING",
        0x8081 => "PER_KEY_LIGHTING_V2",
        0x8090 => "MODE_STATUS",
        0x8100 => "ONBOARD_PROFILES",
        0x8110 => "MOUSE_BUTTON_SPY",
        0x8111 => "LATENCY_MONITORING",
        0x8120 => "GAMING_ATTACHMENTS",
        0x8123 => "FORCE_FEEDBACK",
        0x8300 => "SIDETONE",
        0x8310 => "EQUALIZER",
        0x8320 => "HEADSET_OUT",
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    #[test]
    fn dfu_is_forbidden() {
        for id in [0x00C0, 0x00C1, 0x00C2, 0x00C3, 0x00D0] {
            assert!(super::is_forbidden(id), "{id:#06x}");
        }
        for id in [0x0000, 0x0001, 0x0003, 0x0005, 0x1004, 0x8100] {
            assert!(!super::is_forbidden(id), "{id:#06x}");
        }
    }
}
