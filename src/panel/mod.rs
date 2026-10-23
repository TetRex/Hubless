// SPDX-License-Identifier: GPL-3.0-or-later
//! The panel's life cycle: created lazily on the first open and destroyed
//! if hidden for longer than [`DESTROY_AFTER`]. While there is no window, the UI holds no resources.

mod outside_click;
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
use slint::{ComponentHandle, Model, ModelRc, PhysicalPosition, TimerMode, VecModel};
use windows::Win32::Foundation::HWND;
use windows::Win32::Graphics::Dwm::{
    DWMSBT_MAINWINDOW, DWMWA_SYSTEMBACKDROP_TYPE, DwmExtendFrameIntoClientArea,
    DwmSetWindowAttribute,
};
use windows::Win32::UI::Controls::MARGINS;
use windows::Win32::UI::WindowsAndMessaging::{
    GWL_EXSTYLE, GWL_STYLE, GetWindowLongPtrW, SWP_FRAMECHANGED, SWP_NOACTIVATE, SWP_NOMOVE,
    SWP_NOSIZE, SWP_NOZORDER, SetWindowLongPtrW, SetWindowPos, WS_EX_APPWINDOW, WS_EX_TOOLWINDOW,
    WS_SYSMENU,
};
use windows::Win32::UI::WindowsAndMessaging::{
    SPI_GETCLIENTAREAANIMATION, SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS, SystemParametersInfoW,
};

use crate::app::PanelAction;
use crate::app::{self, AppEvent};
use crate::devices::manager::Controls;
use crate::state::{AppState, DeviceKind, Link};
use crate::theme::{Fonts, SystemTheme};
use crate::ui;

const DESTROY_AFTER: Duration = Duration::from_secs(60);
/// Open and close animations (section 7 of the plan; they match `Theme.anim-open/close`).
const OPEN_ANIMATION: Duration = Duration::from_millis(150);
const CLOSE_ANIMATION: Duration = Duration::from_millis(100);
/// The step of moving the window while opening (~60 fps, only for the duration of the animation).
const SLIDE_TICK: Duration = Duration::from_millis(16);
/// A click on the icon first takes focus away from the panel (and hides it), then the click itself arrives.
/// Such a click must not open the panel again.
const REOPEN_GUARD: Duration = Duration::from_millis(400);

thread_local! {
    /// The next Slint window to be created is the panel. The attributes hook is shared by all windows and
    /// called when a component is created, before it has a title, so we tell them apart by this flag.
    static NEXT_IS_PANEL: Cell<bool> = const { Cell::new(false) };
}

/// The winit attributes hook for Slint windows (called when a component is created): the panel is
/// frameless, always on top and without a taskbar button; the other windows are normal.
pub fn window_attributes(attrs: WindowAttributes) -> WindowAttributes {
    if !NEXT_IS_PANEL.replace(false) {
        // The settings window icon (title bar, taskbar, Alt+Tab) is the same as the exe's.
        const ICON_SIZE: u32 = 32;
        let icon = slint::winit_030::winit::window::Icon::from_rgba(
            crate::tray::icon::app_icon_rgba(ICON_SIZE),
            ICON_SIZE,
            ICON_SIZE,
        )
        .ok();
        return attrs
            .with_class_name("Hubless.Settings")
            .with_window_icon(icon);
    }
    attrs
        .with_decorations(false)
        .with_resizable(false)
        .with_window_level(WindowLevel::AlwaysOnTop)
        .with_skip_taskbar(true)
        .with_corner_preference(CornerPreference::Round)
        .with_undecorated_shadow(true)
        .with_class_name("Hubless.Panel")
}

pub struct PanelController {
    ui: Option<ui::Panel>,
    /// The winit window has been created and set up (style, focus handler).
    window_ready: bool,
    visible: bool,
    hidden_at: Option<Instant>,
    destroy_timer: slint::Timer,
    theme: SystemTheme,
    fonts: Fonts,
    /// When opening started, before the first frame (for the "under 150 ms" measurement).
    opening: Rc<Cell<Option<(Instant, bool)>>>,
    /// The icon whose panel is open (for recomputing the position when the height changes).
    tray_rect: Option<tray_icon::Rect>,
    /// Moving the window while opening (ticks only during the ~150 ms animation).
    slide_timer: Rc<slint::Timer>,
    /// Where the window slides to. Updated on every placement: the panel height may change
    /// during the slide (when the window is created, the layout is ready only after it is shown).
    slide_target: Rc<Cell<PhysicalPosition>>,
    /// Hides the window after the close animation.
    close_timer: slint::Timer,
    /// A mouse hook while the panel is shown: close it on a click on another app's window, even if the
    /// panel didn't get focus (see `outside_click`).
    outside: Option<outside_click::OutsideClick>,
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
            tray_rect: None,
            slide_timer: Rc::default(),
            slide_target: Rc::new(Cell::new(PhysicalPosition::default())),
            close_timer: slint::Timer::default(),
            outside: None,
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
            NEXT_IS_PANEL.set(true);
            match ui::Panel::new() {
                Ok(panel) => {
                    panel.on_dismiss(|| app::post(AppEvent::PanelDismiss));
                    panel.on_open_settings(|| app::post(AppEvent::OpenSettings(None)));
                    panel.on_exit(|| app::post(AppEvent::Exit));
                    panel.on_resized(|| app::post(AppEvent::PanelResized));
                    panel.on_drag_dpi(|v| {
                        app::post(AppEvent::Panel(PanelAction::DpiDrag(v)));
                    });
                    panel.on_enter_dpi(|v| {
                        app::post(AppEvent::Panel(PanelAction::DpiExact(v)));
                    });
                    panel.on_select_dpi(|v| {
                        app::post(AppEvent::Panel(PanelAction::Dpi(
                            v.clamp(0, i32::from(u16::MAX)) as u16,
                        )));
                    });
                    panel.on_set_brightness(|v| {
                        app::post(AppEvent::Panel(PanelAction::Brightness(
                            v.clamp(0, i32::from(u16::MAX)) as u16,
                        )));
                    });
                    panel.on_select_rate(|hz| {
                        app::post(AppEvent::Panel(PanelAction::Rate(hz.max(0) as u32)));
                    });
                    panel.on_set_sidetone(|v| {
                        app::post(AppEvent::Panel(
                            PanelAction::Sidetone(v.clamp(0, 100) as u8),
                        ));
                    });
                    panel.on_select_eq_preset(|i| {
                        app::post(AppEvent::Panel(PanelAction::EqPreset(i)));
                    });
                    panel.on_select_profile(|i| {
                        app::post(AppEvent::Panel(PanelAction::Profile(i)));
                    });
                    panel.on_set_effect(|i| {
                        app::post(AppEvent::Panel(PanelAction::Effect(i)));
                    });
                    panel.on_pick_color(|c| {
                        app::post(AppEvent::Panel(PanelAction::Color([
                            c.red(),
                            c.green(),
                            c.blue(),
                        ])));
                    });
                    panel.on_add_color(|r, g, b| {
                        let c = |v: i32| v.clamp(0, 255) as u8;
                        app::post(AppEvent::Panel(PanelAction::AddColor([c(r), c(g), c(b)])));
                    });
                    panel.on_remove_color(|c| {
                        app::post(AppEvent::Panel(PanelAction::RemoveColor([
                            c.red(),
                            c.green(),
                            c.blue(),
                        ])));
                    });
                    panel.set_max_colors(crate::settings::MAX_COLORS as i32);
                    self.ui = Some(panel);
                    self.window_ready = false;
                    // Slint translations are selected after the component is created.
                    crate::i18n::apply_to_slint();
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
        update_devices(panel, state);
        update_profiles(panel, state);
        self.tray_rect = tray_rect;
        let placed = place(panel, tray_rect, None);
        self.slide_target.set(placed.pos);

        // The window is still fading out after closing: just bring it back, without the slide.
        let closing = self.close_timer.running();
        self.close_timer.stop();
        let animated = client_area_animation();
        panel.set_animated(animated);
        if animated && !closing {
            self.slide_in(panel, placed.edge);
        }
        if let Err(e) = panel.show() {
            tracing::error!("show: {e}");
            return;
        }
        panel.set_shown(true);
        self.visible = true;
        if self.outside.is_none() {
            self.outside = outside_click::OutsideClick::install();
        }

        let dark = self.theme.apps_dark;
        let first = !self.window_ready;
        panel.window().with_winit_window(|w| {
            // winit rebuilds the ex-style on every visibility change (and always sets
            // WS_EX_APPWINDOW), so we apply the style after every show.
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
            let weak = panel.as_weak();
            let backdrop_done = Cell::new(false);
            panel.window().on_winit_window_event(move |window, ev| {
                match ev {
                    WindowEvent::Focused(false) => app::post(AppEvent::PanelFocusLost),
                    WindowEvent::RedrawRequested => {
                        // The Acrylic background, once per window, when the winit window surely exists
                        // (at the first `show()` it doesn't yet).
                        if !backdrop_done.replace(true) {
                            let backdrop =
                                window.with_winit_window(apply_backdrop).unwrap_or(false);
                            tracing::info!(backdrop, "фон панели");
                            if let Some(panel) = weak.upgrade() {
                                panel.set_backdrop(backdrop);
                            }
                        }
                        if let Some((t, created)) = opening.take() {
                            // On creation winit applies the window flags later than `show()`, so we
                            // repeat the style when the window is surely set up.
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

        // The window was created on another monitor and recomputed its scale: correct the position by the actual size.
        let actual = panel.window().size();
        let actual = (actual.width as i32, actual.height as i32);
        if actual != placed.size {
            self.slide_target
                .set(place(panel, tray_rect, Some(actual)).pos);
            tracing::debug!(size = ?placed.size, ?actual, "размер панели уточнён после показа");
        }

        tracing::info!(
            ms = started.elapsed().as_secs_f32() * 1000.0,
            created,
            edge = ?placed.edge,
            scale = placed.scale,
            "панель открыта"
        );
    }

    pub fn hide(&mut self) {
        if !self.visible {
            return;
        }
        self.outside = None;
        self.slide_timer.stop();
        if let Some(panel) = &self.ui {
            panel.set_shown(false);
            if panel.get_animated() {
                // The window hides when its content has faded out.
                let weak = panel.as_weak();
                self.close_timer
                    .start(TimerMode::SingleShot, CLOSE_ANIMATION, move || {
                        if let Some(panel) = weak.upgrade() {
                            let _ = panel.hide();
                        }
                    });
            } else {
                let _ = panel.hide();
            }
        }
        self.visible = false;
        self.hidden_at = Some(Instant::now());
        self.destroy_timer
            .start(TimerMode::SingleShot, DESTROY_AFTER, || {
                app::post(AppEvent::PanelIdle)
            });
    }

    /// Destroys the hidden panel (on the idle timer).
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

    /// The window slides in from the taskbar edge to where [`place`] put it: a cubic ease-out
    /// over [`OPEN_ANIMATION`]. The timer ticks only during the animation.
    fn slide_in(&self, panel: &ui::Panel, edge: placement::Edge) {
        let end = self.slide_target.get();
        let offset = (panel.global::<ui::Theme>().get_anim_open_offset()
            * panel.window().scale_factor())
        .round() as i32;
        let (dx, dy) = match edge {
            placement::Edge::Bottom => (0, offset),
            placement::Edge::Top => (0, -offset),
            placement::Edge::Left => (-offset, 0),
            placement::Edge::Right => (offset, 0),
        };
        let start = PhysicalPosition::new(end.x + dx, end.y + dy);
        panel.window().set_position(start);
        let weak = panel.as_weak();
        let began = Instant::now();
        let timer = Rc::clone(&self.slide_timer);
        let target = Rc::clone(&self.slide_target);
        self.slide_timer
            .start(TimerMode::Repeated, SLIDE_TICK, move || {
                let Some(panel) = weak.upgrade() else {
                    timer.stop();
                    return;
                };
                let t = (began.elapsed().as_secs_f32() / OPEN_ANIMATION.as_secs_f32()).min(1.0);
                let eased = 1.0 - (1.0 - t).powi(3);
                let k = 1.0 - eased;
                let end = target.get();
                panel.window().set_position(PhysicalPosition::new(
                    end.x + (dx as f32 * k).round() as i32,
                    end.y + (dy as f32 * k).round() as i32,
                ));
                if t >= 1.0 {
                    timer.stop();
                }
            });
    }

    /// The panel height changed: bring it back to the taskbar edge.
    pub fn reposition(&self) {
        if let Some(panel) = self.ui.as_ref().filter(|_| self.visible) {
            self.slide_target
                .set(place(panel, self.tray_rect, None).pos);
        }
    }

    /// Updates the data of the open (or newly created) panel.
    pub fn refresh(&self, state: &AppState) {
        if let Some(panel) = &self.ui {
            update_devices(panel, state);
            update_profiles(panel, state);
        }
    }

    pub fn theme(&self) -> SystemTheme {
        self.theme
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

struct Placed {
    pos: PhysicalPosition,
    size: (i32, i32),
    edge: placement::Edge,
    scale: f32,
}

/// Places the panel next to the tray icon for its current height (in physical px for the monitor's DPI).
/// `size` is the actual window size if it is already known.
fn place(
    panel: &ui::Panel,
    tray_rect: Option<tray_icon::Rect>,
    size: Option<(i32, i32)>,
) -> Placed {
    let anchor = placement::anchor(tray_rect);
    let mon = placement::monitor_for(anchor);
    let edge = placement::taskbar_edge(&mon);
    let theme = panel.global::<ui::Theme>();
    let to_px = |v: f32| (v * mon.scale).round() as i32;
    let margin = to_px(theme.get_panel_margin());
    // The panel grows with its content up to the height of the work area; beyond that the cards scroll.
    let work_height = mon.work.bottom - mon.work.top - 2 * margin;
    theme.set_panel_max_height(work_height.max(0) as f32 / mon.scale);
    let size = size.unwrap_or((
        to_px(theme.get_panel_width()),
        to_px(panel.get_content_height()),
    ));
    let (x, y) = placement::position(anchor, &mon, edge, size, margin);
    panel.window().set_position(PhysicalPosition::new(x, y));
    Placed {
        pos: PhysicalPosition::new(x, y),
        size,
        edge,
        scale: mon.scale,
    }
}

fn apply_theme(panel: &ui::Panel, theme: &SystemTheme, fonts: &Fonts) {
    apply_theme_global(&panel.global::<ui::Theme>(), theme, fonts);
}

/// The system theme into the window's `Theme` global (every Slint window has its own globals).
pub fn apply_theme_global(t: &ui::Theme<'_>, theme: &SystemTheme, fonts: &Fonts) {
    t.set_dark(theme.apps_dark);
    let (r, g, b) = theme.accent;
    t.set_accent(slint::Color::from_rgb_u8(r, g, b));
    t.set_font_family(fonts.text.into());
    t.set_icon_font(fonts.icons.into());
}

/// Updates the cards in place: the model lives as long as the window, and only the rows that changed
/// are replaced, so expanded blocks and a slider being dragged aren't reset.
fn update_devices(panel: &ui::Panel, state: &AppState) {
    panel.set_has_devices(!state.devices.is_empty());
    let colors: Vec<slint::Color> = state
        .colors
        .iter()
        .map(|&[r, g, b]| slint::Color::from_rgb_u8(r, g, b))
        .collect();
    let current = panel.get_custom_colors();
    if current.row_count() != colors.len()
        || colors
            .iter()
            .enumerate()
            .any(|(i, c)| current.row_data(i).as_ref() != Some(c))
    {
        panel.set_custom_colors(ModelRc::new(VecModel::from(colors)));
    }
    let items = device_infos(state);
    let model = panel.get_devices();
    match model.as_any().downcast_ref::<VecModel<ui::DeviceInfo>>() {
        Some(rows) if rows.row_count() == items.len() => {
            for (i, item) in items.into_iter().enumerate() {
                if rows.row_data(i).as_ref() != Some(&item) {
                    rows.set_row_data(i, item);
                }
            }
        }
        _ => panel.set_devices(ModelRc::new(VecModel::from(items))),
    }
}

/// The profile list in the panel header.
fn update_profiles(panel: &ui::Panel, state: &AppState) {
    let names: Vec<slint::SharedString> = state.profiles.iter().map(Into::into).collect();
    let current = panel.get_profiles();
    if current.row_count() != names.len()
        || names
            .iter()
            .enumerate()
            .any(|(i, n)| current.row_data(i).as_ref() != Some(n))
    {
        panel.set_profiles(ModelRc::new(VecModel::from(names)));
    }
    panel.set_profile(state.profile as i32);
}

fn device_infos(state: &AppState) -> Vec<ui::DeviceInfo> {
    let devices = state.panel_devices();
    devices
        .iter()
        .map(|d| ui::DeviceInfo {
            kind: match d.kind {
                DeviceKind::Mouse => ui::DeviceKind::Mouse,
                DeviceKind::Keyboard => ui::DeviceKind::Keyboard,
                DeviceKind::Headset => ui::DeviceKind::Headset,
            },
            name: d.display_name().into(),
            connected: d.connected,
            wired: d.link == Some(Link::Usb),
            // The error details are in the log; the card shows a short message in the interface language.
            error: if d.error.is_some() {
                crate::i18n::tr("Couldn't apply the setting").into()
            } else {
                slint::SharedString::new()
            },
            dpi: -1,
            dpi_presets: ModelRc::default(),
            dpi_min: 0,
            dpi_max: 0,
            rate: -1,
            rate_presets: ModelRc::default(),
            rate_index: -1,
            brightness: -1,
            brightness_min: 0,
            brightness_max: 100,
            effect: 0,
            color_custom: false,
            sidetone: -1,
            eq_presets: ModelRc::default(),
            eq_preset: -1,
            color: slint::Color::default(),
            battery: d.battery.map_or(-1, |b| i32::from(b.percent)),
            charging: d.battery.is_some_and(|b| b.charging),
            low: d.battery.is_some_and(|b| b.is_low(state.low_battery)),
        })
        .zip(&devices)
        .map(|(mut info, d)| {
            fill_controls(&mut info, &d.controls);
            if let Controls::Keyboard { lighting, .. } = &d.controls {
                info.color_custom = lighting
                    .and_then(|l| l.color())
                    .is_some_and(|c| state.colors.contains(&c));
            }
            if d.kind == DeviceKind::Headset
                && let Some(gains) = state.headset_eq()
            {
                let presets = crate::eq_presets::all(&state.eq_presets);
                let (names, index) = crate::eq_presets::choices(&presets, &gains);
                let names: Vec<slint::SharedString> = names.iter().map(Into::into).collect();
                info.eq_presets = ModelRc::new(VecModel::from(names));
                info.eq_preset = index as i32;
            }
            info
        })
        .collect()
}

/// The card's control values from the device snapshot.
fn fill_controls(info: &mut ui::DeviceInfo, c: &Controls) {
    match c {
        Controls::None => {}
        Controls::Headset { sidetone, .. } => {
            info.sidetone = sidetone.map_or(-1, i32::from);
        }
        Controls::Mouse {
            dpi,
            dpi_presets,
            dpi_supported,
            rate_hz,
            rate_presets,
            ..
        } => {
            info.dpi_min = dpi_supported.first().map_or(0, |&v| i32::from(v));
            info.dpi_max = dpi_supported.last().map_or(0, |&v| i32::from(v));
            info.rate = rate_hz.map_or(-1, |hz| hz as i32);
            let rates: Vec<i32> = rate_presets.iter().map(|&hz| hz as i32).collect();
            info.rate_index = rates
                .iter()
                .position(|&hz| hz == info.rate)
                .map_or(-1, |i| i as i32);
            info.rate_presets = ModelRc::new(VecModel::from(rates));
            info.dpi = dpi.map_or(-1, i32::from);
            let presets: Vec<i32> = dpi_presets.iter().map(|&v| i32::from(v)).collect();
            info.dpi_presets = ModelRc::new(VecModel::from(presets));
        }
        Controls::Keyboard {
            brightness,
            brightness_range,
            lighting,
            ..
        } => {
            info.brightness = brightness.map_or(-1, i32::from);
            if let Some((min, max)) = brightness_range {
                info.brightness_min = i32::from(*min);
                info.brightness_max = i32::from(*max);
            }
            info.effect = app::effect_index(*lighting);
            if let Some([r, g, b]) = lighting.and_then(|l| l.color()) {
                info.color = slint::Color::from_rgb_u8(r, g, b);
            }
        }
    }
}

/// The system "Animation effects" setting (`SPI_GETCLIENTAREAANIMATION`).
fn client_area_animation() -> bool {
    let mut on = windows::core::BOOL(1);
    // SAFETY: writes a BOOL into a local variable.
    let ok = unsafe {
        SystemParametersInfoW(
            SPI_GETCLIENTAREAANIMATION,
            0,
            Some((&raw mut on).cast()),
            SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
        )
    };
    ok.is_err() || on.as_bool()
}

fn hwnd_of(w: &WinitWindow) -> Option<HWND> {
    let handle = w.window_handle().ok()?;
    let RawWindowHandle::Win32(h) = handle.as_raw() else {
        return None;
    };
    Some(HWND(h.hwnd.get() as *mut _))
}

/// The system Mica background (`DWMSBT_MAINWINDOW`, Windows 11 22H2+).
/// The DWM frame is extended to the whole window: then DWM uses the alpha of the pixels drawn by
/// Slint's software renderer (premultiplied ARGB), and the panel's transparent background shows the backdrop.
/// `false`: the system can't do it, and a solid background from the theme remains.
fn apply_backdrop(w: &WinitWindow) -> bool {
    let Some(hwnd) = hwnd_of(w) else {
        return false;
    };
    // DWM doesn't draw Acrylic (`DWMSBT_TRANSIENTWINDOW`) on this window: the background is just transparent
    // (verified 2026-10-20); Mica is drawn and is cheaper: its tone comes from the wallpaper, with no live blur.
    let kind = DWMSBT_MAINWINDOW;
    let margins = MARGINS {
        cxLeftWidth: -1,
        cxRightWidth: -1,
        cyTopHeight: -1,
        cyBottomHeight: -1,
    };
    // SAFETY: hwnd belongs to a live window of this thread; the attribute and fields are local values.
    unsafe {
        DwmSetWindowAttribute(
            hwnd,
            DWMWA_SYSTEMBACKDROP_TYPE,
            (&raw const kind).cast(),
            size_of_val(&kind) as u32,
        )
        .and_then(|()| DwmExtendFrameIntoClientArea(hwnd, &margins))
        .inspect_err(|e| tracing::info!("Mica недоступен, сплошной фон: {e}"))
        .is_ok()
    }
}

/// `WS_EX_TOOLWINDOW`: no taskbar button and no Alt+Tab.
fn make_tool_window(w: &WinitWindow) {
    let Some(hwnd) = hwnd_of(w) else {
        return;
    };
    // SAFETY: hwnd belongs to a live window of this thread.
    unsafe {
        let style = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
        let style = (style | WS_EX_TOOLWINDOW.0 as isize) & !(WS_EX_APPWINDOW.0 as isize);
        SetWindowLongPtrW(hwnd, GWL_EXSTYLE, style);
        // No system menu: otherwise DWM draws a "×" button in the frame extended for the Mica background.
        let style = GetWindowLongPtrW(hwnd, GWL_STYLE) & !(WS_SYSMENU.0 as isize);
        SetWindowLongPtrW(hwnd, GWL_STYLE, style);
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
