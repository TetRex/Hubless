// SPDX-License-Identifier: GPL-3.0-or-later
//! The shared device state model: the data source for the panel and the tray.

use std::collections::BTreeMap;

use crate::config::DEFAULT_LOW_BATTERY;
use crate::i18n::tr;
use crate::settings::{EqPreset, PanelLayout};

/// The order of the variants is the default card order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum DeviceKind {
    Mouse,
    Keyboard,
    Headset,
}

impl DeviceKind {
    /// The device key in `devices.toml` (panel layout).
    pub fn key(self) -> &'static str {
        match self {
            Self::Mouse => "mouse",
            Self::Keyboard => "keyboard",
            Self::Headset => "headset",
        }
    }

    /// The type name: the card's name if the device didn't report its own.
    pub fn title(self) -> &'static str {
        match self {
            Self::Mouse => tr("Mouse"),
            Self::Keyboard => tr("Keyboard"),
            Self::Headset => tr("Headset"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Link {
    Wireless,
    Usb,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BatteryState {
    pub percent: u8,
    /// Charging (or on external power).
    pub charging: bool,
    /// Charging and fully charged.
    pub full: bool,
}

impl BatteryState {
    /// The charge is below `threshold`, %, and the device isn't charging.
    pub fn is_low(self, threshold: u8) -> bool {
        self.percent < threshold && !self.charging
    }
}

#[derive(Debug, Clone)]
pub struct DeviceState {
    pub kind: DeviceKind,
    pub name: String,
    pub connected: bool,
    pub link: Option<Link>,
    pub battery: Option<BatteryState>,
    /// Current setting values (for the panel's controls).
    pub controls: crate::devices::manager::Controls,
    /// The last write error: a short message in the card.
    pub error: Option<String>,
}

impl DeviceState {
    fn new(kind: DeviceKind) -> Self {
        Self {
            kind,
            name: String::new(),
            connected: false,
            link: None,
            battery: None,
            controls: Default::default(),
            error: None,
        }
    }

    /// The name for the panel and the tray: from the device, otherwise the type name.
    pub fn display_name(&self) -> &str {
        if self.name.is_empty() {
            self.kind.title()
        } else {
            &self.name
        }
    }

    /// "94% ⚡" or "—".
    pub fn battery_text(&self) -> Option<String> {
        self.battery
            .map(|b| format!("{}%{}", b.percent, if b.charging { " ⚡" } else { "" }))
    }
}

#[derive(Debug, Clone)]
pub struct AppState {
    /// Found devices (one per type), in type order. A card exists while the
    /// device is connected or paired with a connected receiver/adapter.
    pub devices: Vec<DeviceState>,
    /// Display names of the profiles and the index of the active one.
    pub profiles: Vec<String>,
    pub profile: usize,
    /// Order and hidden cards of the panel (from `devices.toml`).
    pub layout: PanelLayout,
    /// Your own keyboard palette colours (from `devices.toml`).
    pub colors: Vec<[u8; 3]>,
    /// Your own headset equalizer presets (from `devices.toml`).
    pub eq_presets: Vec<EqPreset>,
    /// Low battery threshold, % (from `config.toml`).
    pub low_battery: u8,
    /// Colours of single keys in the active profile: key name → colour (from `devices.toml`).
    pub key_colors: BTreeMap<String, [u8; 3]>,
    /// The keyboard layout in the key colour editor (from `devices.toml`).
    pub key_layout: crate::keyboard_layout::Layout,
}

impl Default for AppState {
    fn default() -> Self {
        Self {
            devices: Vec::new(),
            profiles: Vec::new(),
            profile: 0,
            layout: PanelLayout::default(),
            colors: Vec::new(),
            eq_presets: Vec::new(),
            low_battery: DEFAULT_LOW_BATTERY,
            key_colors: BTreeMap::new(),
            key_layout: crate::keyboard_layout::Layout::default(),
        }
    }
}

impl AppState {
    /// The device's card; if there is none, add it (in its place by type).
    pub fn device_entry(&mut self, kind: DeviceKind) -> &mut DeviceState {
        let at = match self.devices.binary_search_by_key(&kind, |d| d.kind) {
            Ok(i) => i,
            Err(i) => {
                self.devices.insert(i, DeviceState::new(kind));
                i
            }
        };
        &mut self.devices[at]
    }

    /// The device is gone (neither it nor its receiver or adapter is there): remove the card.
    pub fn remove_device(&mut self, kind: DeviceKind) {
        self.devices.retain(|d| d.kind != kind);
    }

    /// All devices in the order set in the settings.
    pub fn ordered(&self) -> Vec<&DeviceState> {
        let mut list: Vec<&DeviceState> = self.devices.iter().collect();
        // The sort is stable: devices not listed in the order keep the normal order.
        list.sort_by_key(|d| self.layout.rank(d.kind.key()));
        list
    }

    /// The new order (keys of all devices) if the device at `from` is moved to
    /// `to`; `None` if an index is out of range or the place is the same.
    pub fn moved(&self, from: usize, to: usize) -> Option<Vec<String>> {
        let mut keys: Vec<String> = self
            .ordered()
            .iter()
            .map(|d| d.kind.key().to_owned())
            .collect();
        if from >= keys.len() || to >= keys.len() || from == to {
            return None;
        }
        let key = keys.remove(from);
        keys.insert(to, key);
        // Devices that aren't present now keep their place in the list, at the end.
        for k in &self.layout.order {
            if !keys.contains(k) {
                keys.push(k.clone());
            }
        }
        Some(keys)
    }

    /// Panel cards: in order, without hidden ones.
    pub fn panel_devices(&self) -> Vec<&DeviceState> {
        let mut list = self.ordered();
        list.retain(|d| !self.layout.is_hidden(d.kind.key()));
        list
    }

    /// Lines for the tray tooltip: one per connected device, in panel order.
    pub fn tray_lines(&self) -> Vec<String> {
        self.ordered()
            .into_iter()
            .filter(|d| d.connected)
            .map(|d| match d.battery_text() {
                Some(b) => format!("{}: {b}", d.display_name()),
                None => d.display_name().to_owned(),
            })
            .collect()
    }

    /// The connected keyboard with colours of single keys: its keys in the editor layout and the colour of
    /// keys without their own colour (`None`: the effect isn't static, so key colours aren't visible).
    pub fn keyboard_keys(
        &self,
    ) -> Option<(Vec<&'static crate::keyboard_layout::Key>, Option<[u8; 3]>)> {
        self.devices.iter().find_map(|d| match &d.controls {
            crate::devices::manager::Controls::Keyboard {
                key_zones,
                lighting,
                ..
            } if d.connected && !key_zones.is_empty() => Some((
                crate::keyboard_layout::keys_for(self.key_layout, key_zones),
                crate::devices::keyboard::key_base(*lighting),
            )),
            _ => None,
        })
    }

    /// The curve of the connected headset's built-in equalizer, dB per band.
    pub fn headset_eq(&self) -> Option<[i8; crate::headset_eq::BANDS]> {
        self.devices.iter().find_map(|d| match &d.controls {
            crate::devices::manager::Controls::Headset { eq: Some(eq), .. } if d.connected => {
                let gains: Vec<i8> = eq.iter().map(|&(_, g)| g).collect();
                gains.try_into().ok()
            }
            _ => None,
        })
    }

    /// Connected devices with a charge below the threshold.
    pub fn low_battery_devices(&self) -> impl Iterator<Item = &DeviceState> {
        self.devices
            .iter()
            .filter(|d| d.connected && d.battery.is_some_and(|b| b.is_low(self.low_battery)))
    }

    pub fn any_low_battery(&self) -> bool {
        self.low_battery_devices().next().is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// All three devices, as the author has them.
    fn three() -> AppState {
        let mut s = AppState::default();
        for kind in [DeviceKind::Headset, DeviceKind::Mouse, DeviceKind::Keyboard] {
            s.device_entry(kind);
        }
        s
    }

    #[test]
    fn devices_appear_and_disappear() {
        let mut s = AppState::default();
        assert!(s.panel_devices().is_empty(), "до поиска карточек нет");
        s.device_entry(DeviceKind::Headset).name = "A50".into();
        s.device_entry(DeviceKind::Mouse);
        let kinds: Vec<_> = s.devices.iter().map(|d| d.kind).collect();
        assert_eq!(
            kinds,
            [DeviceKind::Mouse, DeviceKind::Headset],
            "порядок по типу"
        );
        assert_eq!(
            s.devices[0].display_name(),
            "Mouse",
            "без имени — название типа"
        );
        s.device_entry(DeviceKind::Headset);
        assert_eq!(s.devices.len(), 2, "повторно не добавляется");
        s.remove_device(DeviceKind::Mouse);
        assert_eq!(s.devices.len(), 1);
        assert_eq!(s.devices[0].display_name(), "A50");
    }

    #[test]
    fn low_battery_ignores_charging_and_disconnected() {
        let mut s = AppState::default();
        let m = s.device_entry(DeviceKind::Mouse);
        m.name = "PRO X 2".into();
        m.connected = true;
        m.battery = Some(BatteryState {
            percent: 10,
            charging: true,
            full: false,
        });
        assert!(!s.any_low_battery());
        s.device_entry(DeviceKind::Mouse).battery = Some(BatteryState {
            percent: 10,
            charging: false,
            full: false,
        });
        assert!(s.any_low_battery());
        s.low_battery = 5;
        assert!(!s.any_low_battery(), "порог из настроек");
        s.low_battery = DEFAULT_LOW_BATTERY;
        assert_eq!(s.tray_lines(), vec!["PRO X 2: 10%".to_owned()]);
        s.device_entry(DeviceKind::Mouse).connected = false;
        assert!(!s.any_low_battery());
        assert!(s.tray_lines().is_empty());
    }

    #[test]
    fn layout_orders_and_hides() {
        let mut s = three();
        let kinds = |list: Vec<&DeviceState>| list.iter().map(|d| d.kind).collect::<Vec<_>>();
        assert_eq!(
            kinds(s.panel_devices()),
            [DeviceKind::Mouse, DeviceKind::Keyboard, DeviceKind::Headset]
        );
        s.layout.order = vec!["headset".into()];
        s.layout.set_hidden("mouse", true);
        assert_eq!(
            kinds(s.ordered()),
            [DeviceKind::Headset, DeviceKind::Mouse, DeviceKind::Keyboard]
        );
        assert_eq!(
            kinds(s.panel_devices()),
            [DeviceKind::Headset, DeviceKind::Keyboard]
        );
        assert_eq!(s.moved(0, 3), None);
        assert_eq!(s.moved(1, 1), None);
        // headset, mouse, keyboard → mouse, keyboard, headset
        assert_eq!(
            s.moved(0, 2),
            Some(vec!["mouse".into(), "keyboard".into(), "headset".into()])
        );
        assert_eq!(
            s.moved(2, 0),
            Some(vec!["keyboard".into(), "headset".into(), "mouse".into()])
        );
        // A missing device doesn't lose its place in the saved order.
        s.layout.order = vec!["headset".into(), "mouse".into(), "keyboard".into()];
        s.remove_device(DeviceKind::Keyboard);
        assert_eq!(
            s.moved(1, 0),
            Some(vec!["mouse".into(), "headset".into(), "keyboard".into()])
        );
    }
}
