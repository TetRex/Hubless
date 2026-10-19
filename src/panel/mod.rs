// SPDX-License-Identifier: GPL-3.0-or-later
//! Жизненный цикл панели: создаётся лениво при первом открытии, уничтожается,
//! если скрыта дольше [`DESTROY_AFTER`]. Пока окна нет, UI не держит ресурсов.

mod placement;

use std::cell::Cell;
use std::rc::Rc;
use std::time::{Duration, Instant};

use slint::winit_030::winit::event::WindowEvent;
use slint::winit_030::winit::platform::windows::{CornerPreference, WindowAttributesExtWindows};
use slint::winit_030::winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
use slint::winit_030::winit::window::{
    Theme as WinitTheme, Window as WinitWindow, WindowAttributes, WindowLevel,
};
use slint::winit_030::{EventResult, WinitWindowAccessor};
use slint::{ComponentHandle, ModelRc, PhysicalPosition, TimerMode, VecModel};
use windows::Win32::Foundation::HWND;
use windows::Win32::UI::WindowsAndMessaging::{
    GWL_EXSTYLE, GetWindowLongPtrW, SWP_FRAMECHANGED, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE,
    SWP_NOZORDER, SetWindowLongPtrW, SetWindowPos, WS_EX_APPWINDOW, WS_EX_TOOLWINDOW,
};

use crate::app::{self, AppEvent};
use crate::state::{AppState, DeviceKind};
use crate::theme::{Fonts, SystemTheme};
use crate::ui;

const DESTROY_AFTER: Duration = Duration::from_secs(60);
/// Клик по иконке сначала снимает фокус с панели (и скрывает её), затем приходит сам клик.
/// Такой клик не должен открывать панель заново.
const REOPEN_GUARD: Duration = Duration::from_millis(400);

/// Хук атрибутов winit для окна панели (вызывается при его создании).
pub fn window_attributes(attrs: WindowAttributes) -> WindowAttributes {
    attrs
        .with_decorations(false)
        .with_resizable(false)
        .with_window_level(WindowLevel::AlwaysOnTop)
        .with_skip_taskbar(true)
        .with_corner_preference(CornerPreference::Round)
        .with_undecorated_shadow(true)
        .with_class_name("ProTray.Panel")
}

pub struct PanelController {
    ui: Option<ui::Panel>,
    /// winit-окно создано и настроено (стиль, обработчик фокуса).
    window_ready: bool,
    visible: bool,
    hidden_at: Option<Instant>,
    destroy_timer: slint::Timer,
    theme: SystemTheme,
    fonts: Fonts,
    /// Момент начала открытия — до первого кадра (замер «быстрее 150 мс»).
    opening: Rc<Cell<Option<(Instant, bool)>>>,
}

impl PanelController {
    pub fn new(theme: SystemTheme) -> Self {
        Self {
            ui: None,
            window_ready: false,
            visible: false,
            hidden_at: None,
            destroy_timer: slint::Timer::default(),
            theme,
            fonts: Fonts::detect(),
            opening: Rc::default(),
        }
    }

    pub fn toggle(&mut self, tray_rect: Option<tray_icon::Rect>, state: &AppState) {
        if self.visible {
            self.hide();
        } else if self.hidden_at.is_some_and(|t| t.elapsed() < REOPEN_GUARD) {
            tracing::debug!("клик, закрывший панель, — не открываем повторно");
        } else {
            self.show(tray_rect, state);
        }
    }

    pub fn show(&mut self, tray_rect: Option<tray_icon::Rect>, state: &AppState) {
        let started = Instant::now();
        self.destroy_timer.stop();
        let created = self.ui.is_none();
        self.opening.set(Some((started, created)));
        if created {
            match ui::Panel::new() {
                Ok(panel) => {
                    panel.on_dismiss(|| app::post(AppEvent::PanelDismiss));
                    self.ui = Some(panel);
                    self.window_ready = false;
                }
                Err(e) => {
                    tracing::error!("не удалось создать панель: {e}");
                    return;
                }
            }
        }
        let Some(panel) = self.ui.as_ref() else {
            return;
        };
        apply_theme(panel, &self.theme, &self.fonts);
        panel.set_devices(devices_model(state));

        // Размер в физических px считаем по DPI монитора, на котором окажется панель.
        let anchor = placement::anchor(tray_rect);
        let mon = placement::monitor_for(anchor);
        let edge = placement::taskbar_edge(&mon);
        let theme = panel.global::<ui::Theme>();
        let to_px = |v: f32| (v * mon.scale).round() as i32;
        let margin = to_px(theme.get_panel_margin());
        let size = (
            to_px(theme.get_panel_width()),
            to_px(panel.get_content_height()),
        );
        let (x, y) = placement::position(anchor, &mon, edge, size, margin);
        panel.window().set_position(PhysicalPosition::new(x, y));

        if let Err(e) = panel.show() {
            tracing::error!("show: {e}");
            return;
        }
        self.visible = true;

        let dark = self.theme.apps_dark;
        let first = !self.window_ready;
        panel.window().with_winit_window(|w| {
            // winit пересобирает ex-style при каждой смене видимости (и всегда ставит
            // WS_EX_APPWINDOW), поэтому стиль применяем после каждого показа.
            make_tool_window(w);
            w.set_theme(Some(if dark {
                WinitTheme::Dark
            } else {
                WinitTheme::Light
            }));
            w.focus_window();
        });
        if first {
            let opening = self.opening.clone();
            panel.window().on_winit_window_event(move |window, ev| {
                match ev {
                    WindowEvent::Focused(false) => app::post(AppEvent::PanelFocusLost),
                    WindowEvent::RedrawRequested => {
                        if let Some((t, created)) = opening.take() {
                            // При создании winit применяет флаги окна позже `show()` —
                            // повторяем стиль, когда окно уже точно настроено.
                            window.with_winit_window(make_tool_window);
                            tracing::info!(
                                ms = t.elapsed().as_secs_f32() * 1000.0,
                                created,
                                "первый кадр панели"
                            );
                        }
                    }
                    _ => {}
                }
                EventResult::Propagate
            });
            self.window_ready = true;
        }

        // Окно создано на другом мониторе и пересчитало масштаб — поправить позицию по факту.
        let actual = panel.window().size();
        let actual = (actual.width as i32, actual.height as i32);
        if actual != size {
            let (x, y) = placement::position(anchor, &mon, edge, actual, margin);
            panel.window().set_position(PhysicalPosition::new(x, y));
            tracing::debug!(?size, ?actual, "размер панели уточнён после показа");
        }

        tracing::info!(
            ms = started.elapsed().as_secs_f32() * 1000.0,
            created,
            ?edge,
            scale = mon.scale,
            "панель открыта"
        );
    }

    pub fn hide(&mut self) {
        if !self.visible {
            return;
        }
        if let Some(panel) = &self.ui {
            let _ = panel.hide();
        }
        self.visible = false;
        self.hidden_at = Some(Instant::now());
        self.destroy_timer
            .start(TimerMode::SingleShot, DESTROY_AFTER, || {
                app::post(AppEvent::PanelIdle)
            });
    }

    /// Уничтожить скрытую панель (по таймеру простоя).
    pub fn destroy_if_hidden(&mut self) {
        if !self.visible && self.ui.take().is_some() {
            self.window_ready = false;
            tracing::info!("панель уничтожена после простоя");
        }
    }

    pub fn destroy(&mut self) {
        self.hide();
        self.destroy_timer.stop();
        self.ui = None;
    }

    pub fn set_theme(&mut self, theme: SystemTheme) {
        self.theme = theme;
        if let Some(panel) = &self.ui {
            apply_theme(panel, &theme, &self.fonts);
            panel.window().with_winit_window(|w| {
                w.set_theme(Some(if theme.apps_dark {
                    WinitTheme::Dark
                } else {
                    WinitTheme::Light
                }));
            });
        }
    }
}

fn apply_theme(panel: &ui::Panel, theme: &SystemTheme, fonts: &Fonts) {
    let t = panel.global::<ui::Theme>();
    t.set_dark(theme.apps_dark);
    let (r, g, b) = theme.accent;
    t.set_accent(slint::Color::from_rgb_u8(r, g, b));
    t.set_font_family(fonts.text.into());
    t.set_icon_font(fonts.icons.into());
}

fn devices_model(state: &AppState) -> ModelRc<ui::DeviceInfo> {
    let items: Vec<_> = state
        .devices
        .iter()
        .map(|d| ui::DeviceInfo {
            kind: match d.kind {
                DeviceKind::Mouse => ui::DeviceKind::Mouse,
                DeviceKind::Keyboard => ui::DeviceKind::Keyboard,
                DeviceKind::Headset => ui::DeviceKind::Headset,
            },
            name: d.name.as_str().into(),
            connected: d.connected,
            link: Default::default(),
        })
        .collect();
    ModelRc::new(VecModel::from(items))
}

/// `WS_EX_TOOLWINDOW`: без кнопки на панели задач и без Alt+Tab.
fn make_tool_window(w: &WinitWindow) {
    let Ok(handle) = w.window_handle() else {
        return;
    };
    let RawWindowHandle::Win32(h) = handle.as_raw() else {
        return;
    };
    let hwnd = HWND(h.hwnd.get() as *mut _);
    // SAFETY: hwnd принадлежит живому окну этого потока.
    unsafe {
        let style = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
        let style = (style | WS_EX_TOOLWINDOW.0 as isize) & !(WS_EX_APPWINDOW.0 as isize);
        SetWindowLongPtrW(hwnd, GWL_EXSTYLE, style);
        let _ = SetWindowPos(
            hwnd,
            None,
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE | SWP_FRAMECHANGED,
        );
    }
}
