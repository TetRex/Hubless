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

/// Фон иконки приложения — синий акцент Windows 11.
const APP_BACKGROUND: [u8; 3] = [0x00, 0x67, 0xc0];

/// Иконка приложения (exe, окно настроек): белый контур мыши на синем скруглённом квадрате.
/// Рисуется теми же фигурами, что и значок трея, в сетке 32×32, масштабированной до `size`.
pub fn app_icon_rgba(size: u32) -> Vec<u8> {
    let scale = size as f32 / SIZE as f32;
    let mut rgba = Vec::with_capacity((size * size * 4) as usize);
    for y in 0..size {
        for x in 0..size {
            let p = ((x as f32 + 0.5) / scale, (y as f32 + 0.5) / scale);
            // Расстояния считаются в сетке 32×32, сглаживание — в пикселях иконки.
            let bg = coverage(rounded_rect(p, (16.0, 16.0), (15.5, 15.5), 7.0) * scale);
            let body = rounded_rect(p, (16.0, 16.0), (8.0, 11.5), 8.0).abs() - STROKE / 2.0;
            let wheel = segment(p, (16.0, 8.5), (16.0, 12.5)) - STROKE / 2.0;
            let fg = coverage(body.min(wheel) * scale).min(bg);
            let mix = |b: u8| (f32::from(b) * (1.0 - fg) + 255.0 * fg).round() as u8;
            let [r, g, b] = APP_BACKGROUND;
            rgba.extend_from_slice(&[mix(r), mix(g), mix(b), (bg * 255.0).round() as u8]);
        }
    }
    rgba
}

/// Файл `.ico` с иконкой приложения нужных размеров (32-битные DIB, без PNG — без зависимостей).
/// Нужен только генератору `res/hubless.ico` (тест `write_exe_icon`).
#[cfg(test)]
fn app_icon_ico(sizes: &[u32]) -> Vec<u8> {
    let images: Vec<Vec<u8>> = sizes.iter().map(|&s| ico_dib(s)).collect();
    let mut out = Vec::new();
    out.extend_from_slice(&[0, 0, 1, 0]);
    out.extend_from_slice(&(sizes.len() as u16).to_le_bytes());
    let mut offset = 6 + 16 * sizes.len();
    for (&s, img) in sizes.iter().zip(&images) {
        let side = if s >= 256 { 0 } else { s as u8 };
        out.extend_from_slice(&[side, side, 0, 0]);
        out.extend_from_slice(&1u16.to_le_bytes());
        out.extend_from_slice(&32u16.to_le_bytes());
        out.extend_from_slice(&(img.len() as u32).to_le_bytes());
        out.extend_from_slice(&(offset as u32).to_le_bytes());
        offset += img.len();
    }
    for img in images {
        out.extend_from_slice(&img);
    }
    out
}

/// Одна картинка ICO: BITMAPINFOHEADER (высота удвоена — с маской), BGRA снизу вверх,
/// пустая маска AND (прозрачность — из альфы).
#[cfg(test)]
fn ico_dib(size: u32) -> Vec<u8> {
    let rgba = app_icon_rgba(size);
    let mask_row = size.div_ceil(32) * 4;
    let mut out = Vec::new();
    for v in [40u32, size, size * 2] {
        out.extend_from_slice(&v.to_le_bytes());
    }
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&32u16.to_le_bytes());
    let image_size = size * size * 4 + mask_row * size;
    for v in [0u32, image_size, 0, 0, 0, 0] {
        out.extend_from_slice(&v.to_le_bytes());
    }
    for y in (0..size).rev() {
        for x in 0..size {
            let i = ((y * size + x) * 4) as usize;
            out.extend_from_slice(&[rgba[i + 2], rgba[i + 1], rgba[i], rgba[i + 3]]);
        }
    }
    out.resize(out.len() + (mask_row * size) as usize, 0);
    out
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Размеры иконки exe: от значка в списке файлов до плитки «Огромные значки».
    const EXE_SIZES: [u32; 7] = [16, 20, 24, 32, 48, 64, 256];

    #[test]
    fn ico_layout() {
        let ico = app_icon_ico(&[16, 32]);
        assert_eq!(&ico[..6], &[0, 0, 1, 0, 2, 0]);
        // Первая картинка сразу за каталогом, её размер — заголовок + пиксели + маска.
        let len = u32::from_le_bytes(ico[14..18].try_into().unwrap());
        let offset = u32::from_le_bytes(ico[18..22].try_into().unwrap());
        assert_eq!((len, offset), (40 + 16 * 16 * 4 + 4 * 16, 6 + 32));
        // Угол прозрачный, центр контура мыши — белый.
        let rgba = app_icon_rgba(32);
        assert_eq!(rgba[3], 0);
        let at = |x: usize, y: usize| &rgba[(y * 32 + x) * 4..(y * 32 + x) * 4 + 4];
        assert_eq!(at(16, 10), &[255, 255, 255, 255]);
    }

    /// Перегенерировать `res/hubless.ico`: `cargo test --release write_exe_icon -- --ignored`.
    #[test]
    #[ignore]
    fn write_exe_icon() {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/res/hubless.ico");
        std::fs::write(path, app_icon_ico(&EXE_SIZES)).unwrap();
    }
}
