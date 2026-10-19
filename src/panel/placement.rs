// SPDX-License-Identifier: GPL-3.0-or-later
//! Позиционирование панели у иконки в трее с учётом стороны панели задач, мониторов и DPI.

use windows::Win32::Foundation::{POINT, RECT};
use windows::Win32::Graphics::Gdi::{
    GetMonitorInfoW, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromRect,
};
use windows::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};
use windows::Win32::UI::Shell::{
    ABE_LEFT, ABE_RIGHT, ABE_TOP, ABM_GETTASKBARPOS, APPBARDATA, SHAppBarMessage,
};
use windows::Win32::UI::WindowsAndMessaging::GetCursorPos;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Edge {
    Left,
    Top,
    Right,
    Bottom,
}

#[derive(Debug, Clone, Copy)]
pub struct Monitor {
    pub rect: RECT,
    pub work: RECT,
    pub scale: f32,
}

/// Прямоугольник иконки в трее, иначе точка под курсором.
pub fn anchor(tray_rect: Option<tray_icon::Rect>) -> RECT {
    if let Some(r) = tray_rect.filter(|r| r.size.width > 0) {
        let (x, y) = (r.position.x.round() as i32, r.position.y.round() as i32);
        return RECT {
            left: x,
            top: y,
            right: x + r.size.width as i32,
            bottom: y + r.size.height as i32,
        };
    }
    let mut p = POINT::default();
    // SAFETY: запись в локальную структуру.
    let _ = unsafe { GetCursorPos(&mut p) };
    RECT {
        left: p.x,
        top: p.y,
        right: p.x + 1,
        bottom: p.y + 1,
    }
}

pub fn monitor_for(anchor: RECT) -> Monitor {
    // SAFETY: MonitorFromRect с NEAREST всегда возвращает монитор; структуры локальные.
    unsafe {
        let hmon = MonitorFromRect(&anchor, MONITOR_DEFAULTTONEAREST);
        let mut info = MONITORINFO {
            cbSize: size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        let _ = GetMonitorInfoW(hmon, &mut info);
        let (mut dx, mut dy) = (96u32, 96u32);
        let _ = GetDpiForMonitor(hmon, MDT_EFFECTIVE_DPI, &mut dx, &mut dy);
        Monitor {
            rect: info.rcMonitor,
            work: info.rcWork,
            scale: dx as f32 / 96.0,
        }
    }
}

/// Сторона панели задач: `ABM_GETTASKBARPOS`, иначе — по разнице рабочей области и монитора.
pub fn taskbar_edge(mon: &Monitor) -> Edge {
    let mut data = APPBARDATA {
        cbSize: size_of::<APPBARDATA>() as u32,
        ..Default::default()
    };
    // SAFETY: data — локальная структура с корректным cbSize.
    if unsafe { SHAppBarMessage(ABM_GETTASKBARPOS, &mut data) } != 0 {
        return match data.uEdge {
            ABE_LEFT => Edge::Left,
            ABE_TOP => Edge::Top,
            ABE_RIGHT => Edge::Right,
            _ => Edge::Bottom,
        };
    }
    let (r, w) = (mon.rect, mon.work);
    if w.top > r.top {
        Edge::Top
    } else if w.left > r.left {
        Edge::Left
    } else if w.right < r.right {
        Edge::Right
    } else {
        Edge::Bottom
    }
}

/// Левый верхний угол панели размером `size` (физ. px). Панель прижата к панели задач
/// с отступом `margin`, вдоль неё — центрирована по иконке и не выходит за рабочую область.
pub fn position(
    anchor: RECT,
    mon: &Monitor,
    edge: Edge,
    size: (i32, i32),
    margin: i32,
) -> (i32, i32) {
    let (w, h) = size;
    let (r, work) = (mon.rect, mon.work);
    let center_x = (anchor.left + anchor.right) / 2;
    let center_y = (anchor.top + anchor.bottom) / 2;
    let clamp = |v: i32, lo: i32, hi: i32| v.min(hi).max(lo);
    let along_x = clamp(
        center_x - w / 2,
        work.left + margin,
        work.right - margin - w,
    );
    let along_y = clamp(
        center_y - h / 2,
        work.top + margin,
        work.bottom - margin - h,
    );

    // Если рабочая область не уменьшена (автоскрытие), границей служит сама иконка.
    let (x, y) = match edge {
        Edge::Bottom => {
            let limit = if work.bottom < r.bottom {
                work.bottom
            } else {
                anchor.top.min(r.bottom)
            };
            (along_x, limit - margin - h)
        }
        Edge::Top => {
            let limit = if work.top > r.top {
                work.top
            } else {
                anchor.bottom.max(r.top)
            };
            (along_x, limit + margin)
        }
        Edge::Left => {
            let limit = if work.left > r.left {
                work.left
            } else {
                anchor.right.max(r.left)
            };
            (limit + margin, along_y)
        }
        Edge::Right => {
            let limit = if work.right < r.right {
                work.right
            } else {
                anchor.left.min(r.right)
            };
            (limit - margin - w, along_y)
        }
    };
    // Панель выше экрана — прижимаем к верху рабочей области.
    (x.max(work.left), y.max(work.top))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(left: i32, top: i32, right: i32, bottom: i32) -> RECT {
        RECT {
            left,
            top,
            right,
            bottom,
        }
    }

    const FHD: RECT = RECT {
        left: 0,
        top: 0,
        right: 1920,
        bottom: 1080,
    };

    fn mon(work: RECT) -> Monitor {
        Monitor {
            rect: FHD,
            work,
            scale: 1.0,
        }
    }

    #[test]
    fn bottom_taskbar_clamps_to_right_edge() {
        let m = mon(rect(0, 0, 1920, 1032));
        let icon = rect(1800, 1040, 1824, 1064);
        assert_eq!(
            position(icon, &m, Edge::Bottom, (360, 400), 12),
            (1920 - 12 - 360, 1032 - 12 - 400)
        );
    }

    #[test]
    fn top_taskbar() {
        let m = mon(rect(0, 48, 1920, 1080));
        let icon = rect(1000, 12, 1024, 36);
        assert_eq!(
            position(icon, &m, Edge::Top, (360, 400), 12),
            (1012 - 180, 48 + 12)
        );
    }

    #[test]
    fn left_and_right_taskbar_center_vertically_on_icon() {
        let icon = rect(10, 900, 34, 924);
        let m = mon(rect(48, 0, 1920, 1080));
        assert_eq!(position(icon, &m, Edge::Left, (360, 200), 12), (60, 812));
        let icon = rect(1886, 500, 1910, 524);
        let m = mon(rect(0, 0, 1872, 1080));
        assert_eq!(
            position(icon, &m, Edge::Right, (360, 200), 12),
            (1872 - 12 - 360, 412)
        );
    }

    #[test]
    fn autohide_taskbar_uses_icon_as_limit() {
        let m = mon(FHD);
        let icon = rect(1800, 1040, 1824, 1064);
        assert_eq!(
            position(icon, &m, Edge::Bottom, (360, 400), 12).1,
            1040 - 12 - 400
        );
    }

    #[test]
    fn secondary_monitor_with_negative_coords() {
        let m = Monitor {
            rect: rect(-2560, 0, 0, 1440),
            work: rect(-2560, 0, 0, 1392),
            scale: 1.5,
        };
        let icon = rect(-120, 1400, -84, 1436);
        assert_eq!(
            position(icon, &m, Edge::Bottom, (540, 600), 18),
            (-18 - 540, 1392 - 18 - 600)
        );
    }
}
