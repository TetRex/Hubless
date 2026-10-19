// SPDX-License-Identifier: GPL-3.0-or-later
//! Общая модель состояния устройств — источник данных для панели и трея.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceKind {
    Mouse,
    Keyboard,
    Headset,
}

#[derive(Debug, Clone)]
pub struct DeviceState {
    pub kind: DeviceKind,
    pub name: String,
    pub connected: bool,
}

#[derive(Debug, Clone)]
pub struct AppState {
    pub devices: Vec<DeviceState>,
}

impl Default for AppState {
    /// Пока нет HID-ядра (фаза 1): известные устройства, все отключены.
    fn default() -> Self {
        let device = |kind, name: &str| DeviceState {
            kind,
            name: name.into(),
            connected: false,
        };
        Self {
            devices: vec![
                device(DeviceKind::Mouse, "PRO X Superlight 2"),
                device(DeviceKind::Keyboard, "PRO X TKL"),
                device(DeviceKind::Headset, "PRO X Wireless"),
            ],
        }
    }
}
