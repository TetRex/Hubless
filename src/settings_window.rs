// SPDX-License-Identifier: GPL-3.0-or-later
//! The settings window: interface language, start with Windows, battery notifications, order and visibility
//! of devices in the panel, device settings (with equalizer presets), profiles and binding them
//! to programs, updates, logs and the device report.
//!
//! It is created when opened (the gear in the panel) and destroyed when closed, so
//! while there is no window it holds no resources. All actions go to [`App`](crate::app) as
//! [`AppEvent::Settings`]: that is where files are changed and profiles are applied.

use slint::winit_030::WinitWindowAccessor;
use slint::winit_030::winit::window::Theme as WinitTheme;
use slint::{ComponentHandle, Model, ModelRc, SharedString, VecModel};

use crate::app::{self, AppEvent};
use crate::config::{Config, LOW_BATTERY_CHOICES};
use crate::i18n::{self, Choice, Lang};
use crate::settings::SettingsFile;
use crate::state::{AppState, DeviceKind};
use crate::theme::{Fonts, SystemTheme};
use crate::tray::autostart;
use crate::ui;

/// An action in the settings window.
#[derive(Debug)]
pub enum SettingsAction {
    /// An index in the language list: 0 English, 1 Russian.
    Language(i32),
    Autostart(bool),
    /// The low battery threshold: an index in [`LOW_BATTERY_CHOICES`].
    LowBattery(i32),
    NotifyCharged(bool),
    /// Check for updates at startup.
    UpdateOnStart(bool),
    /// Moves a device in the "Devices" section: from a place to a place (indices in the list).
    MoveDevice(i32, i32),
    /// Shows (`true`) or hides a device in the panel.
    ShowDevice(i32, bool),
    /// Device settings from the "Devices" section: mouse host mode, lift-off (1 medium, 2 high),
    /// headset auto power-off in minutes.
    MouseHost(bool),
    LiftOff(i32),
    AutoOff(i32),
    /// Headset equalizer: band and gain, dB.
    EqBand(i32, i32),
    /// An equalizer preset by its index in the list; save the current curve as your own preset;
    /// delete your own preset (index in the list).
    EqPreset(i32),
    SaveEqPreset(String),
    DeleteEqPreset(i32),
    /// Turn the keyboard lighting off after this many idle minutes (`0` = never).
    LightingOff(i32),
    /// The keyboard layout in the colour editor: 0 ANSI, 1 ISO.
    KeyLayout(i32),
    /// The colour of the key under a point (layout units); `None` removes its own colour (eraser).
    PaintKey(f32, f32, Option<[u8; 3]>),
    /// All keys in one colour; `None` removes all their own colours.
    FillKeys(Option<[u8; 3]>),
    Select(i32),
    /// A new profile is a copy of the active one; the name "Profile N" is picked automatically.
    Create,
    Rename(i32, String),
    Delete(i32),
    Use(i32),
    AddProgram(i32, String),
    RemoveProgram(i32, String),
    RefreshRunning,
    CreateReport,
    /// Check for updates, open the new version's page ("General"); logs ("Advanced").
    CheckUpdates,
    OpenUpdate,
    OpenLogs,
    DeleteLogs,
    OpenReports,
    CopyReport,
    OpenIssue,
    Closed,
}

/// The language by its index in the window's list (English, Russian) and back. Until a language is chosen
/// (`auto`), the list shows the one taken from Windows; choosing one fixes the language.
pub fn choice_of(index: i32) -> Choice {
    match index {
        1 => Choice::Fixed(Lang::Ru),
        _ => Choice::Fixed(Lang::En),
    }
}

fn index_of(choice: Choice) -> i32 {
    match choice.resolve() {
        Lang::En => 0,
        Lang::Ru => 1,
    }
}

/// The name of a new profile: "Profile N" with the first free N.
pub fn new_profile_name(existing: &[String]) -> String {
    let base = i18n::tr("Profile {}");
    (2..)
        .map(|n| base.replace("{}", &n.to_string()))
        .find(|name| !existing.contains(name))
        .expect("бесконечный ряд имён")
}

/// The display name of a profile: the first one is "Default" in the interface language.
pub fn display_name(index: usize, name: &str) -> String {
    if index == 0 {
        i18n::tr("Default").to_owned()
    } else {
        name.to_owned()
    }
}

#[derive(Default)]
pub struct SettingsController {
    ui: Option<ui::SettingsWindow>,
    /// Running programs for binding (refreshed when opened and with the button).
    running: Vec<String>,
    /// The last device report (lives while the window is open).
    report: Option<crate::report::Report>,
    /// The new version that was found (the release page for the "Download" button). It lives without the window
    /// too: the check at startup may find it.
    update: Option<crate::update::Release>,
    /// The equalizer sliders are being moved: don't put the curve from the headset into them, otherwise a slider
    /// jumps back to the last written value, and putting a value in triggers a new write.
    eq_editing: bool,
}

impl SettingsController {
    /// Opens (or raises) the window; `page` is the section: 0 "General", 1 "Devices", 2 "Profiles",
    /// 3 "Advanced"; 4 and 5 are "Devices" with the headset (equalizer) or the keyboard
    /// (key colours) expanded.
    pub fn open(&mut self, theme: SystemTheme, state: &AppState, page: Option<usize>) {
        if self.ui.is_none() {
            match ui::SettingsWindow::new() {
                Ok(w) => {
                    wire(&w);
                    self.ui = Some(w);
                }
                Err(e) => {
                    tracing::error!("не удалось создать окно настроек: {e}");
                    return;
                }
            }
            i18n::apply_to_slint();
            self.running = crate::foreground::running_apps();
        }
        let Some(w) = &self.ui else { return };
        if let Some(page) = page {
            w.set_page(if page >= 4 { 1 } else { page as i32 });
        }
        w.set_ghub_running(crate::report::ghub_running());
        self.set_theme(theme);
        self.show_update();
        self.refresh(None);
        self.update_devices(state);
        // After the device list: filling it the first time resets the expanded row.
        let open = match page {
            Some(4) => Some(DeviceKind::Headset),
            Some(5) => Some(DeviceKind::Keyboard),
            _ => None,
        };
        if let (Some(kind), Some(w)) = (open, &self.ui) {
            let row = state
                .ordered()
                .iter()
                .position(|d| d.kind == kind)
                .map_or(-1, |i| i as i32);
            w.set_open_row(row);
        }
        let Some(w) = &self.ui else { return };
        if let Err(e) = w.show() {
            tracing::error!("окно настроек: {e}");
            return;
        }
        w.window().with_winit_window(|win| win.focus_window());
    }

    /// Closes and destroys the window.
    pub fn close(&mut self) {
        if let Some(w) = self.ui.take() {
            let _ = w.hide();
        }
        self.report = None;
    }

    /// The update check has started: the button shows "Checking…".
    pub fn update_started(&self) {
        if let Some(w) = &self.ui {
            w.set_update_busy(true);
            w.set_update_status(SharedString::new());
        }
    }

    pub fn update_finished(&mut self, result: Result<crate::update::Release, String>) {
        let current = env!("CARGO_PKG_VERSION");
        let status = match result {
            Ok(r) if crate::update::is_newer(&r.version, current) => {
                self.update = Some(r);
                None
            }
            Ok(_) => {
                self.update = None;
                Some(i18n::tr("You have the latest version"))
            }
            Err(e) => {
                tracing::warn!("проверка обновлений: {e}");
                Some(i18n::tr("Couldn't check for updates"))
            }
        };
        let Some(w) = &self.ui else { return };
        w.set_update_busy(false);
        match status {
            Some(text) => {
                w.set_update_url(SharedString::new());
                w.set_update_status(text.into());
            }
            None => self.show_update(),
        }
    }

    /// The check at startup found a new version: the "Download" button appears when the window opens.
    pub fn found_update(&mut self, release: crate::update::Release) {
        self.update = Some(release);
        self.show_update();
    }

    /// The new version that was found, in the "General" section (a line and the "Download" button).
    fn show_update(&self) {
        let (Some(w), Some(r)) = (&self.ui, &self.update) else {
            return;
        };
        w.set_update_url(r.url.as_str().into());
        w.set_update_status(
            i18n::tr("Version {} is available")
                .replace("{}", &r.version)
                .into(),
        );
    }

    /// The page of the new version that was found.
    pub fn update_url(&self) -> Option<&str> {
        self.update.as_ref().map(|r| r.url.as_str())
    }

    pub fn set_logs_status(&self, message: &str) {
        if let Some(w) = &self.ui {
            w.set_logs_status(message.into());
        }
    }

    /// Collecting the report has started: the button shows "Collecting…".
    pub fn report_started(&self) {
        if let Some(w) = &self.ui {
            w.set_report_busy(true);
            w.set_report_status(SharedString::new());
            w.set_ghub_running(crate::report::ghub_running());
        }
    }

    pub fn report_finished(&mut self, result: Result<crate::report::Report, String>) {
        let Some(w) = &self.ui else { return };
        w.set_report_busy(false);
        match result {
            Ok(r) => {
                w.set_report_text(r.text.as_str().into());
                w.set_report_path(r.path.display().to_string().into());
                w.set_report_status(
                    i18n::tr("Saved: {}")
                        .replace("{}", &r.path.display().to_string())
                        .into(),
                );
                self.report = Some(r);
            }
            Err(e) => {
                tracing::warn!("отчёт об устройствах: {e}");
                w.set_report_status(i18n::tr("Couldn't create the report").into());
            }
        }
    }

    pub fn report(&self) -> Option<&crate::report::Report> {
        self.report.as_ref()
    }

    pub fn set_report_status(&self, message: &str) {
        if let Some(w) = &self.ui {
            w.set_report_status(message.into());
        }
    }

    pub fn set_theme(&self, theme: SystemTheme) {
        let Some(w) = &self.ui else { return };
        crate::panel::apply_theme_global(&w.global::<ui::Theme>(), &theme, &Fonts::detect());
        w.window().with_winit_window(|win| {
            win.set_theme(Some(if theme.apps_dark {
                WinitTheme::Dark
            } else {
                WinitTheme::Light
            }));
        });
    }

    pub fn refresh_running(&mut self) {
        self.running = crate::foreground::running_apps();
        self.refresh(None);
    }

    /// Rereads the files and updates the window; `select` selects a profile by index.
    pub fn refresh(&self, select: Option<usize>) {
        let Some(w) = &self.ui else { return };
        let file = SettingsFile::load();
        let names = file.names();
        let selected = select
            .unwrap_or(w.get_selected().max(0) as usize)
            .min(names.len() - 1);
        let current = file.current_name();
        let rows: Vec<ui::ProfileRow> = names
            .iter()
            .enumerate()
            .map(|(i, n)| {
                let exe: &[String] = i
                    .checked_sub(1)
                    .and_then(|j| file.profiles.get(j))
                    .map_or(&[], |p| p.exe.as_slice());
                let summary = if i == 0 {
                    i18n::tr("Used when no other profile matches").to_owned()
                } else if exe.is_empty() {
                    i18n::tr("No programs — switch to it by hand").to_owned()
                } else {
                    exe.join(", ")
                };
                let programs: Vec<SharedString> = exe.iter().map(Into::into).collect();
                ui::ProfileRow {
                    name: display_name(i, n).into(),
                    summary: summary.into(),
                    programs: ModelRc::new(VecModel::from(programs)),
                    active: *n == current,
                    base: i == 0,
                }
            })
            .collect();
        // The first list item is a hint; choosing any of the others adds that program.
        let running: Vec<SharedString> =
            std::iter::once(SharedString::from(i18n::tr("Add a running program…")))
                .chain(self.running.iter().map(Into::into))
                .collect();
        w.set_profiles(ModelRc::new(VecModel::from(rows)));
        w.set_selected(selected as i32);
        w.set_running(ModelRc::new(VecModel::from(running)));
        let config = Config::load().0;
        w.set_language(index_of(Choice::parse(&config.language)));
        w.set_autostart(autostart::is_enabled());
        let percent = config.low_battery_percent();
        let choice = LOW_BATTERY_CHOICES
            .iter()
            .position(|&p| p == percent)
            .or_else(|| LOW_BATTERY_CHOICES.iter().position(|&p| p > percent))
            .unwrap_or(LOW_BATTERY_CHOICES.len() - 1);
        w.set_low_battery(choice as i32);
        w.set_notify_charged(config.notify_charged);
        w.set_update_on_start(config.check_updates);
        w.set_error(SharedString::new());
    }

    pub fn set_error(&self, message: &str) {
        if let Some(w) = &self.ui {
            w.set_error(message.into());
        }
    }

    /// The "Devices" section: all devices in the panel's order. Rows are changed in place, so
    /// the model lives and a drag isn't interrupted if a battery update arrives meanwhile.
    pub fn update_devices(&self, state: &AppState) {
        let Some(w) = &self.ui else { return };
        let rows: Vec<ui::DeviceRow> = state
            .ordered()
            .into_iter()
            .map(|d| ui::DeviceRow {
                kind: match d.kind {
                    DeviceKind::Mouse => ui::DeviceKind::Mouse,
                    DeviceKind::Keyboard => ui::DeviceKind::Keyboard,
                    DeviceKind::Headset => ui::DeviceKind::Headset,
                },
                name: d.display_name().into(),
                connected: d.connected,
                shown: !state.layout.is_hidden(d.kind.key()),
                ..device_options(d)
            })
            .collect();
        update_eq(w, state, self.eq_editing);
        update_eq_presets(w, state);
        update_keys(w, state);
        let model = w.get_devices();
        match model.as_any().downcast_ref::<VecModel<ui::DeviceRow>>() {
            Some(current) if current.row_count() == rows.len() => {
                for (i, row) in rows.into_iter().enumerate() {
                    if current.row_data(i).as_ref() != Some(&row) {
                        current.set_row_data(i, row);
                    }
                }
            }
            _ => {
                // The device list changed: the expanded row may now belong to another device.
                w.set_open_row(-1);
                w.set_devices(ModelRc::new(VecModel::from(rows)));
            }
        }
    }

    /// The equalizer sliders are being moved (until the curve is saved in the headset).
    pub fn set_eq_editing(&mut self, editing: bool) {
        self.eq_editing = editing;
    }

    /// Puts the equalizer sliders on the given curve (a preset was chosen).
    pub fn show_eq_gains(&self, gains: &[i8]) {
        let Some(w) = &self.ui else { return };
        let model = w.get_eq_bands();
        for (i, &gain) in gains.iter().enumerate().take(model.row_count()) {
            if let Some(mut band) = model.row_data(i) {
                band.position = -f32::from(gain);
                model.set_row_data(i, band);
            }
        }
    }

    /// The device move has been handled: the list rows settle into place.
    pub fn finish_drop(&self) {
        if let Some(w) = &self.ui {
            w.invoke_finish_drop();
        }
    }
}

/// The "Equalizer" section: the headset's bands. Rows are changed in place, and only those that changed;
/// while the sliders are being moved (`editing`), values from the headset aren't put in, only the headset
/// appearing and disappearing.
fn update_eq(w: &ui::SettingsWindow, state: &AppState, editing: bool) {
    let bands: Vec<ui::EqBand> = state
        .devices
        .iter()
        .find(|d| d.kind == DeviceKind::Headset && d.connected)
        .and_then(|d| match &d.controls {
            crate::devices::manager::Controls::Headset { eq: Some(eq), .. } => Some(eq.clone()),
            _ => None,
        })
        .unwrap_or_default()
        .into_iter()
        .map(|(freq, gain)| ui::EqBand {
            label: band_label(freq).into(),
            position: -f32::from(gain),
        })
        .collect();
    let model = w.get_eq_bands();
    match model.as_any().downcast_ref::<VecModel<ui::EqBand>>() {
        Some(current) if current.row_count() == bands.len() && editing => {}
        Some(current) if current.row_count() == bands.len() => {
            for (i, band) in bands.into_iter().enumerate() {
                if current.row_data(i).as_ref() != Some(&band) {
                    current.set_row_data(i, band);
                }
            }
        }
        _ => w.set_eq_bands(ModelRc::new(VecModel::from(bands))),
    }
}

/// The list of equalizer presets and the selected one, by the headset's curve (in the state it is updated
/// right away on an edit, before the headset replies).
fn update_eq_presets(w: &ui::SettingsWindow, state: &AppState) {
    let Some(gains) = state.headset_eq() else {
        w.set_eq_presets(ModelRc::default());
        return;
    };
    let presets = crate::eq_presets::all(&state.eq_presets);
    let (names, index) = crate::eq_presets::choices(&presets, &gains);
    let names: Vec<SharedString> = names.iter().map(Into::into).collect();
    let current = w.get_eq_presets();
    if current.row_count() != names.len()
        || names
            .iter()
            .enumerate()
            .any(|(i, n)| current.row_data(i).as_ref() != Some(n))
    {
        w.set_eq_presets(ModelRc::new(VecModel::from(names)));
    }
    w.set_eq_preset(index as i32);
    w.set_eq_preset_custom(presets.get(index).is_some_and(|p| p.custom));
}

/// The key colour editor: the layout keys the keyboard has, with colours (their own or the
/// effect's), and your own palette colours. Rows are changed in place, so a brush stroke isn't interrupted.
fn update_keys(w: &ui::SettingsWindow, state: &AppState) {
    let custom: Vec<slint::Color> = state
        .colors
        .iter()
        .map(|&[r, g, b]| slint::Color::from_rgb_u8(r, g, b))
        .collect();
    let current = w.get_custom_colors();
    if current.row_count() != custom.len()
        || custom
            .iter()
            .enumerate()
            .any(|(i, c)| current.row_data(i).as_ref() != Some(c))
    {
        w.set_custom_colors(ModelRc::new(VecModel::from(custom)));
    }
    let (keys, base) = state.keyboard_keys().unwrap_or_default();
    w.set_keys_active(base.is_some());
    w.set_key_layout(i32::from(
        state.key_layout == crate::keyboard_layout::Layout::Iso,
    ));
    w.set_active_profile(
        state
            .profiles
            .get(state.profile)
            .map_or(SharedString::new(), Into::into),
    );
    let caps: Vec<ui::KeyCap> = keys
        .iter()
        .map(|k| {
            let own = state.key_colors.get(k.name).copied();
            let color = own.or(base);
            let [r, g, b] = color.unwrap_or_default();
            // A bright key gets a dark label (brightness per ITU-R BT.601).
            let luma = 299 * u32::from(r) + 587 * u32::from(g) + 114 * u32::from(b);
            ui::KeyCap {
                label: k.label.into(),
                x: k.x,
                y: k.y,
                w: k.w,
                h: k.h,
                color: slint::Color::from_rgb_u8(r, g, b),
                lit: color.is_some(),
                custom: own.is_some(),
                dark_text: luma > 140_000,
            }
        })
        .collect();
    let model = w.get_keys();
    match model.as_any().downcast_ref::<VecModel<ui::KeyCap>>() {
        Some(current) if current.row_count() == caps.len() => {
            for (i, cap) in caps.into_iter().enumerate() {
                if current.row_data(i).as_ref() != Some(&cap) {
                    current.set_row_data(i, cap);
                }
            }
        }
        _ => w.set_keys(ModelRc::new(VecModel::from(caps))),
    }
}

/// The device settings for a "Devices" row (none for a disconnected device).
fn device_options(d: &crate::state::DeviceState) -> ui::DeviceRow {
    use crate::devices::manager::Controls;
    use crate::settings::LiftOff;
    let mut row = ui::DeviceRow {
        lift_off: -1,
        auto_off: -1,
        light_off: -1,
        ..Default::default()
    };
    if !d.connected {
        return row;
    }
    match &d.controls {
        Controls::Mouse {
            host,
            host_mode,
            lift_off,
            ..
        } => {
            row.host = *host;
            row.host_mode = *host_mode;
            row.lift_off = match lift_off {
                Some(LiftOff::Low) => 0,
                Some(LiftOff::Medium) => 1,
                Some(LiftOff::High) => 2,
                None => -1,
            };
        }
        Controls::Headset { auto_off, .. } => {
            row.auto_off = auto_off.map_or(-1, i32::from);
        }
        Controls::Keyboard {
            key_zones,
            lighting_off_min,
            ..
        } => {
            row.per_key = !key_zones.is_empty();
            row.light_off = lighting_off_min.map_or(-1, i32::from);
        }
        _ => {}
    }
    row
}

/// "80 Hz", "2.2 kHz" in the interface language.
fn band_label(freq: u16) -> String {
    if freq >= 1000 {
        let khz = f32::from(freq) / 1000.0;
        let text = if freq.is_multiple_of(1000) {
            format!("{khz:.0}")
        } else {
            format!("{khz:.1}")
        };
        i18n::tr("{} kHz").replace("{}", &text)
    } else {
        i18n::tr("{} Hz").replace("{}", &freq.to_string())
    }
}

fn wire(w: &ui::SettingsWindow) {
    let post = |a: SettingsAction| app::post(AppEvent::Settings(a));
    w.on_set_language(move |i| post(SettingsAction::Language(i)));
    w.on_set_autostart(move |on| post(SettingsAction::Autostart(on)));
    let choices: Vec<SharedString> = LOW_BATTERY_CHOICES
        .iter()
        .map(|p| format!("{p}%").into())
        .collect();
    w.set_low_battery_choices(ModelRc::new(VecModel::from(choices)));
    w.on_set_low_battery(move |i| post(SettingsAction::LowBattery(i)));
    w.on_set_notify_charged(move |on| post(SettingsAction::NotifyCharged(on)));
    w.on_set_update_on_start(move |on| post(SettingsAction::UpdateOnStart(on)));
    w.on_move_device(move |from, to| post(SettingsAction::MoveDevice(from, to)));
    w.on_show_device(move |i, on| post(SettingsAction::ShowDevice(i, on)));
    w.on_set_mouse_host(move |on| post(SettingsAction::MouseHost(on)));
    w.on_set_lift_off(move |v| post(SettingsAction::LiftOff(v)));
    w.on_set_auto_off(move |m| post(SettingsAction::AutoOff(m)));
    w.on_set_light_off(move |m| post(SettingsAction::LightingOff(m)));
    w.on_set_eq_band(move |i, gain| post(SettingsAction::EqBand(i, gain)));
    w.on_select_eq_preset(move |i| post(SettingsAction::EqPreset(i)));
    w.on_save_eq_preset(move |name| post(SettingsAction::SaveEqPreset(name.into())));
    w.on_delete_eq_preset(move |i| post(SettingsAction::DeleteEqPreset(i)));
    w.set_key_layout_width(crate::keyboard_layout::WIDTH);
    w.set_key_layout_height(crate::keyboard_layout::HEIGHT);
    let rgb = |c: slint::Color, erase: bool| (!erase).then(|| [c.red(), c.green(), c.blue()]);
    w.on_paint_key(move |x, y, c, erase| post(SettingsAction::PaintKey(x, y, rgb(c, erase))));
    w.on_fill_keys(move |c, erase| post(SettingsAction::FillKeys(rgb(c, erase))));
    w.on_set_key_layout(move |i| post(SettingsAction::KeyLayout(i)));
    w.set_eq_min(i32::from(*crate::headset_eq::GAIN_RANGE.start()));
    w.set_eq_max(i32::from(*crate::headset_eq::GAIN_RANGE.end()));
    w.on_select_profile(move |i| post(SettingsAction::Select(i)));
    w.on_create_profile(move || post(SettingsAction::Create));
    w.on_rename_profile(move |i, name| post(SettingsAction::Rename(i, name.into())));
    w.on_delete_profile(move |i| post(SettingsAction::Delete(i)));
    w.on_use_profile(move |i| post(SettingsAction::Use(i)));
    w.on_add_program(move |i, exe| post(SettingsAction::AddProgram(i, exe.into())));
    w.on_remove_program(move |i, exe| post(SettingsAction::RemoveProgram(i, exe.into())));
    w.on_refresh_running(move || post(SettingsAction::RefreshRunning));
    w.on_create_report(move || post(SettingsAction::CreateReport));
    w.on_check_updates(move || post(SettingsAction::CheckUpdates));
    w.on_open_update(move || post(SettingsAction::OpenUpdate));
    w.on_open_logs(move || post(SettingsAction::OpenLogs));
    w.on_delete_logs(move || post(SettingsAction::DeleteLogs));
    w.set_version(env!("CARGO_PKG_VERSION").into());
    w.on_open_reports(move || post(SettingsAction::OpenReports));
    w.on_copy_report(move || post(SettingsAction::CopyReport));
    w.on_open_issue(move || post(SettingsAction::OpenIssue));
    w.window().on_close_requested(move || {
        post(SettingsAction::Closed);
        slint::CloseRequestResponse::HideWindow
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_profile_name_skips_taken() {
        let names = vec!["Default".to_owned(), "Profile 2".to_owned()];
        assert_eq!(new_profile_name(&names), "Profile 3");
        assert_eq!(new_profile_name(&names[..1]), "Profile 2");
    }
}
