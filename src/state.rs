// SPDX-License-Identifier: GPL-3.0-or-later
//! Общая модель состояния устройств — источник данных для панели и трея.

/// Порог низкого заряда (индикатор на иконке, уведомление).
pub const LOW_BATTERY: u8 = 15;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceKind {
    Mouse,
    Keyboard,
    Headset,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Link {
    Wireless,
    Usb,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BatteryState {
    pub percent: u8,
    pub charging: bool,
}

impl BatteryState {
    pub fn is_low(self) -> bool {
        self.percent < LOW_BATTERY && !self.charging
    }
}

#[derive(Debug, Clone)]
pub struct DeviceState {
    pub kind: DeviceKind,
    pub name: String,
    pub connected: bool,
    pub link: Option<Link>,
    pub battery: Option<BatteryState>,
    /// Текущие значения настроек (для контролов панели).
    pub controls: crate::devices::manager::Controls,
    /// Последняя ошибка записи — короткое сообщение в карточке.
    pub error: Option<String>,
}

impl DeviceState {
    /// «94% ⚡» или «—».
    pub fn battery_text(&self) -> Option<String> {
        self.battery
            .map(|b| format!("{}%{}", b.percent, if b.charging { " ⚡" } else { "" }))
    }
}

#[derive(Debug, Clone)]
pub struct AppState {
    pub devices: Vec<DeviceState>,
    /// Имена профилей настроек и действующий профиль.
    pub profiles: Vec<String>,
    pub profile: String,
}

impl Default for AppState {
    /// Известные устройства, все отключены до первого опроса.
    fn default() -> Self {
        let device = |kind, name: &str| DeviceState {
            kind,
            name: name.into(),
            connected: false,
            link: None,
            battery: None,
            controls: Default::default(),
            error: None,
        };
        Self {
            devices: vec![
                device(DeviceKind::Mouse, "PRO X Superlight 2"),
                device(DeviceKind::Keyboard, "PRO X TKL"),
                device(DeviceKind::Headset, "PRO X 2 LIGHTSPEED"),
            ],
            profiles: Vec::new(),
            profile: String::new(),
        }
    }
}

impl AppState {
    pub fn device_mut(&mut self, kind: DeviceKind) -> Option<&mut DeviceState> {
        self.devices.iter_mut().find(|d| d.kind == kind)
    }

    /// Строки для tooltip трея: по одной на подключённое устройство.
    pub fn tray_lines(&self) -> Vec<String> {
        self.devices
            .iter()
            .filter(|d| d.connected)
            .map(|d| match d.battery_text() {
                Some(b) => format!("{}: {b}", d.name),
                None => d.name.clone(),
            })
            .collect()
    }

    pub fn any_low_battery(&self) -> bool {
        self.devices
            .iter()
            .any(|d| d.connected && d.battery.is_some_and(BatteryState::is_low))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn low_battery_ignores_charging_and_disconnected() {
        let mut s = AppState::default();
        let m = s.device_mut(DeviceKind::Mouse).unwrap();
        m.connected = true;
        m.battery = Some(BatteryState {
            percent: 10,
            charging: true,
        });
        assert!(!s.any_low_battery());
        s.device_mut(DeviceKind::Mouse).unwrap().battery = Some(BatteryState {
            percent: 10,
            charging: false,
        });
        assert!(s.any_low_battery());
        assert_eq!(s.tray_lines(), vec!["PRO X Superlight 2: 10%".to_owned()]);
        s.device_mut(DeviceKind::Mouse).unwrap().connected = false;
        assert!(!s.any_low_battery());
        assert!(s.tray_lines().is_empty());
    }
}
