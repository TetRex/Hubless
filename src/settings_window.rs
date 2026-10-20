// SPDX-License-Identifier: GPL-3.0-or-later
//! Окно настроек: язык интерфейса, автозапуск, профили и их привязка к программам.
//!
//! Создаётся при открытии (меню трея или шестерёнка в панели) и уничтожается при закрытии —
//! пока окна нет, оно не держит ресурсов. Все действия уходят в [`App`](crate::app) как
//! [`AppEvent::Settings`]: там же меняются файлы и применяются профили.

use slint::winit_030::WinitWindowAccessor;
use slint::winit_030::winit::window::Theme as WinitTheme;
use slint::{ComponentHandle, ModelRc, SharedString, VecModel};

use crate::app::{self, AppEvent};
use crate::config::Config;
use crate::i18n::{self, Choice, Lang};
use crate::settings::SettingsFile;
use crate::theme::{Fonts, SystemTheme};
use crate::tray::autostart;
use crate::ui;

/// Действие в окне настроек.
#[derive(Debug)]
pub enum SettingsAction {
    /// Индекс в списке языков: 0 — английский, 1 — русский.
    Language(i32),
    Autostart(bool),
    Select(i32),
    /// Новый профиль — копия действующего, имя «Профиль N» подбирается само.
    Create,
    Rename(i32, String),
    Delete(i32),
    Use(i32),
    AddProgram(i32, String),
    RemoveProgram(i32, String),
    RefreshRunning,
    Closed,
}

/// Язык по индексу списка в окне (English, Русский) и обратно. Пока язык не выбран
/// (`auto`), в списке показывается тот, что взят у Windows; выбор закрепляет язык.
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

/// Имя нового профиля: «Профиль N» с первым свободным N.
pub fn new_profile_name(existing: &[String]) -> String {
    let base = i18n::tr("Profile {}");
    (2..)
        .map(|n| base.replace("{}", &n.to_string()))
        .find(|name| !existing.contains(name))
        .expect("бесконечный ряд имён")
}

/// Отображаемое имя профиля: первый — «Обычный» на языке интерфейса.
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
    /// Запущенные программы для привязки (обновляются при открытии и по кнопке).
    running: Vec<String>,
}

impl SettingsController {
    /// Открыть (или поднять) окно; `page` — раздел: 0 «Общие», 1 «Профили».
    pub fn open(&mut self, theme: SystemTheme, page: Option<usize>) {
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
            w.set_page(page.min(1) as i32);
        }
        self.set_theme(theme);
        self.refresh(None);
        if let Err(e) = w.show() {
            tracing::error!("окно настроек: {e}");
            return;
        }
        w.window().with_winit_window(|win| win.focus_window());
    }

    /// Закрыть и уничтожить окно.
    pub fn close(&mut self) {
        if let Some(w) = self.ui.take() {
            let _ = w.hide();
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

    /// Перечитать файлы и обновить окно; `select` — выбрать профиль по индексу.
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
        // Первый пункт списка — подсказка, выбор остальных добавляет программу.
        let running: Vec<SharedString> =
            std::iter::once(SharedString::from(i18n::tr("Add a running program…")))
                .chain(self.running.iter().map(Into::into))
                .collect();
        w.set_profiles(ModelRc::new(VecModel::from(rows)));
        w.set_selected(selected as i32);
        w.set_running(ModelRc::new(VecModel::from(running)));
        w.set_language(index_of(Choice::parse(&Config::load().0.language)));
        w.set_autostart(autostart::is_enabled());
        w.set_error(SharedString::new());
    }

    pub fn set_error(&self, message: &str) {
        if let Some(w) = &self.ui {
            w.set_error(message.into());
        }
    }
}

fn wire(w: &ui::SettingsWindow) {
    let post = |a: SettingsAction| app::post(AppEvent::Settings(a));
    w.on_set_language(move |i| post(SettingsAction::Language(i)));
    w.on_set_autostart(move |on| post(SettingsAction::Autostart(on)));
    w.on_select_profile(move |i| post(SettingsAction::Select(i)));
    w.on_create_profile(move || post(SettingsAction::Create));
    w.on_rename_profile(move |i, name| post(SettingsAction::Rename(i, name.into())));
    w.on_delete_profile(move |i| post(SettingsAction::Delete(i)));
    w.on_use_profile(move |i| post(SettingsAction::Use(i)));
    w.on_add_program(move |i, exe| post(SettingsAction::AddProgram(i, exe.into())));
    w.on_remove_program(move |i, exe| post(SettingsAction::RemoveProgram(i, exe.into())));
    w.on_refresh_running(move || post(SettingsAction::RefreshRunning));
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
