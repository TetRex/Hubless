// SPDX-License-Identifier: GPL-3.0-or-later
//! Монохромная иконка трея (контур мыши), рисуется в коде под цвет панели задач.

use tray_icon::Icon;

const SIZE: u32 = 32;
const STROKE: f32 = 2.4;
/// Цвет индикатора низкого заряда (системный красный Windows).
const LOW_BATTERY: [u8; 3] = [0xe8, 0x11, 0x23];

pub fn render(taskbar_light: bool, low_battery: bool) -> Icon {
    let fg: [u8; 3] = if taskbar_light { [0, 0, 0] } else { [0xff; 3] };
    let mut rgba = Vec::with_capacity((SIZE * SIZE * 4) as usize);
    for y in 0..SIZE {
        for x in 0..SIZE {
            let p = (x as f32 + 0.5, y as f32 + 0.5);
            let (color, alpha) = pixel(p, fg, low_battery);
            rgba.extend_from_slice(&[color[0], color[1], color[2], (alpha * 255.0).round() as u8]);
        }
    }
    Icon::from_rgba(rgba, SIZE, SIZE).expect("valid icon buffer")
}

fn pixel(p: (f32, f32), fg: [u8; 3], low_battery: bool) -> ([u8; 3], f32) {
    if low_battery {
        // Точка в правом нижнем углу с вырезом вокруг неё.
        let d = length(p.0 - 25.0, p.1 - 25.0);
        if d < 6.0 + 1.0 {
            return (LOW_BATTERY, coverage(d - 6.0));
        }
    }
    // Корпус: скруглённый прямоугольник 18×26, радиус 9.
    let body = rounded_rect(p, (16.0, 16.0), (9.0, 13.0), 9.0).abs() - STROKE / 2.0;
    // Колесо: короткий вертикальный отрезок сверху.
    let wheel = segment(p, (16.0, 7.5), (16.0, 12.0)) - STROKE / 2.0;
    (fg, coverage(body.min(wheel)))
}

/// Покрытие пикселя по знаковому расстоянию (простое сглаживание на 1 px).
fn coverage(d: f32) -> f32 {
    (0.5 - d).clamp(0.0, 1.0)
}

fn length(x: f32, y: f32) -> f32 {
    (x * x + y * y).sqrt()
}

fn rounded_rect(p: (f32, f32), c: (f32, f32), half: (f32, f32), r: f32) -> f32 {
    let qx = (p.0 - c.0).abs() - (half.0 - r);
    let qy = (p.1 - c.1).abs() - (half.1 - r);
    length(qx.max(0.0), qy.max(0.0)) + qx.max(qy).min(0.0) - r
}

fn segment(p: (f32, f32), a: (f32, f32), b: (f32, f32)) -> f32 {
    let (pax, pay) = (p.0 - a.0, p.1 - a.1);
    let (bax, bay) = (b.0 - a.0, b.1 - a.1);
    let h = ((pax * bax + pay * bay) / (bax * bax + bay * bay)).clamp(0.0, 1.0);
    length(pax - bax * h, pay - bay * h)
}
