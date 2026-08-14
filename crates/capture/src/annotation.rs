use image::{Rgba, RgbaImage};
use serde::{Deserialize, Serialize};
use tiny_skia::{Color, Paint, PathBuilder, Pixmap, Stroke, Transform};

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AnnotationKind {
    Rectangle,
    Ellipse,
    Arrow,
    Line,
    Brush,
    Text,
    Mosaic,
    Blur,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Annotation {
    pub id: String,
    pub kind: AnnotationKind,
    /// 图片逻辑坐标。
    pub geometry: Vec<f64>,
    pub style: serde_json::Value,
    pub z_index: i32,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct AnnotationScene {
    pub items: Vec<Annotation>,
}

/// Rust 负责最终导出。WebView 只做预览/交互。
pub fn export_png(
    width: u32,
    height: u32,
    bgra: &[u8],
    scene: &AnnotationScene,
) -> Result<Vec<u8>, String> {
    let mut pixmap = Pixmap::new(width, height).ok_or("invalid pixmap size")?;
    for y in 0..height {
        for x in 0..width {
            let i = ((y * width + x) * 4) as usize;
            if i + 3 >= bgra.len() {
                continue;
            }
            let color = Color::from_rgba8(bgra[i + 2], bgra[i + 1], bgra[i], bgra[i + 3]);
            pixmap.pixels_mut()[(y * width + x) as usize] = color.premultiply().to_color_u8();
        }
    }
    let mut items = scene.items.clone();
    items.sort_by_key(|a| a.z_index);
    for ann in items {
        draw(&mut pixmap, &ann);
    }
    let mut img = RgbaImage::new(width, height);
    for y in 0..height {
        for x in 0..width {
            let p = pixmap.pixel(x, y).unwrap();
            img.put_pixel(x, y, Rgba([p.red(), p.green(), p.blue(), p.alpha()]));
        }
    }
    let mut out = Vec::new();
    image::DynamicImage::ImageRgba8(img)
        .write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png)
        .map_err(|e| e.to_string())?;
    Ok(out)
}

fn draw(pixmap: &mut Pixmap, ann: &Annotation) {
    let mut paint = Paint::default();
    paint.set_color_rgba8(255, 70, 70, 255);
    paint.anti_alias = true;
    match ann.kind {
        AnnotationKind::Rectangle if ann.geometry.len() >= 4 => {
            let path = PathBuilder::from_rect(
                tiny_skia::Rect::from_xywh(
                    ann.geometry[0] as f32,
                    ann.geometry[1] as f32,
                    ann.geometry[2] as f32,
                    ann.geometry[3] as f32,
                )
                .unwrap_or(tiny_skia::Rect::from_ltrb(0.0, 0.0, 1.0, 1.0).unwrap()),
            );
            let stroke = Stroke { width: 3.0, ..Stroke::default() };
            pixmap.stroke_path(&path, &paint, &stroke, Transform::identity(), None);
        }
        AnnotationKind::Line | AnnotationKind::Brush if ann.geometry.len() >= 4 => {
            let mut pb = PathBuilder::new();
            pb.move_to(ann.geometry[0] as f32, ann.geometry[1] as f32);
            let mut rest = &ann.geometry[2..];
            while rest.len() >= 2 {
                pb.line_to(rest[0] as f32, rest[1] as f32);
                rest = &rest[2..];
            }
            if let Some(path) = pb.finish() {
                let stroke = Stroke { width: 3.0, ..Stroke::default() };
                pixmap.stroke_path(&path, &paint, &stroke, Transform::identity(), None);
            }
        }
        AnnotationKind::Arrow if ann.geometry.len() >= 4 => {
            let (x0, y0, x1, y1) = (
                ann.geometry[0] as f32,
                ann.geometry[1] as f32,
                ann.geometry[2] as f32,
                ann.geometry[3] as f32,
            );
            let angle = (y1 - y0).atan2(x1 - x0);
            let head = 14.0f32;
            let spread = std::f32::consts::FRAC_PI_6;
            let mut pb = PathBuilder::new();
            pb.move_to(x0, y0);
            pb.line_to(x1, y1);
            pb.move_to(x1, y1);
            pb.line_to(x1 - head * (angle - spread).cos(), y1 - head * (angle - spread).sin());
            pb.move_to(x1, y1);
            pb.line_to(x1 - head * (angle + spread).cos(), y1 - head * (angle + spread).sin());
            if let Some(path) = pb.finish() {
                let stroke = Stroke { width: 3.0, ..Stroke::default() };
                pixmap.stroke_path(&path, &paint, &stroke, Transform::identity(), None);
            }
        }
        AnnotationKind::Mosaic if ann.geometry.len() >= 4 => {
            apply_block(pixmap, ann.geometry[0], ann.geometry[1], ann.geometry[2], ann.geometry[3]);
        }
        AnnotationKind::Blur if ann.geometry.len() >= 4 => {
            let radius = ann.style.get("radius").and_then(|value| value.as_u64()).unwrap_or(6);
            apply_blur(
                pixmap,
                ann.geometry[0],
                ann.geometry[1],
                ann.geometry[2],
                ann.geometry[3],
                radius.clamp(1, 32) as u32,
            );
        }
        AnnotationKind::Text if ann.geometry.len() >= 2 => {
            let text = annotation_text(ann);
            if !text.is_empty() {
                draw_bitmap_text(pixmap, ann.geometry[0] as i32, ann.geometry[1] as i32, &text);
            }
        }
        _ => {}
    }
}

fn annotation_text(ann: &Annotation) -> String {
    ann.style
        .get("text")
        .or_else(|| ann.style.get("label"))
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string()
}

fn draw_bitmap_text(pixmap: &mut Pixmap, x: i32, y: i32, text: &str) {
    let mut cx = x;
    let cy = y;
    for ch in text.chars().take(128) {
        let glyph = glyph_for(ch);
        for (row, bits) in glyph.iter().enumerate() {
            for col in 0..5 {
                if bits & (1 << (4 - col)) != 0 {
                    put_pixel(pixmap, cx + col, cy + row as i32, 255, 70, 70);
                }
            }
        }
        cx += 6;
    }
}

fn put_pixel(pixmap: &mut Pixmap, x: i32, y: i32, r: u8, g: u8, b: u8) {
    if x < 0 || y < 0 {
        return;
    }
    let x = x as u32;
    let y = y as u32;
    if x >= pixmap.width() || y >= pixmap.height() {
        return;
    }
    let color = Color::from_rgba8(r, g, b, 255).premultiply().to_color_u8();
    let width = pixmap.width();
    pixmap.pixels_mut()[(y * width + x) as usize] = color;
}

fn glyph_for(ch: char) -> [u8; 7] {
    match ch.to_ascii_uppercase() {
        ' ' => [0; 7],
        'A' => [0x0E, 0x11, 0x11, 0x1F, 0x11, 0x11, 0x11],
        'E' => [0x1F, 0x10, 0x10, 0x1E, 0x10, 0x10, 0x1F],
        'O' => [0x0E, 0x11, 0x11, 0x11, 0x11, 0x11, 0x0E],
        'T' => [0x1F, 0x04, 0x04, 0x04, 0x04, 0x04, 0x04],
        _ => [0x1F, 0x11, 0x11, 0x11, 0x11, 0x11, 0x1F],
    }
}

fn apply_block(pixmap: &mut Pixmap, x: f64, y: f64, w: f64, h: f64) {
    let x = x.max(0.0) as u32;
    let y = y.max(0.0) as u32;
    let w = w.max(1.0) as u32;
    let h = h.max(1.0) as u32;
    let block = 12u32;
    for by in (y..y.saturating_add(h)).step_by(block as usize) {
        for bx in (x..x.saturating_add(w)).step_by(block as usize) {
            if let Some(p) = pixmap.pixel(bx, by) {
                let max_y = (by + block).min(pixmap.height());
                let max_x = (bx + block).min(pixmap.width());
                let width = pixmap.width();
                let pixels = pixmap.pixels_mut();
                for yy in by..max_y {
                    for xx in bx..max_x {
                        pixels[(yy * width + xx) as usize] = p;
                    }
                }
            }
        }
    }
}

fn apply_blur(pixmap: &mut Pixmap, x: f64, y: f64, w: f64, h: f64, radius: u32) {
    let left = x.max(0.0) as u32;
    let top = y.max(0.0) as u32;
    let right = (x + w.max(1.0)).max(0.0).min(f64::from(pixmap.width())) as u32;
    let bottom = (y + h.max(1.0)).max(0.0).min(f64::from(pixmap.height())) as u32;
    if left >= right || top >= bottom {
        return;
    }

    let source_left = left.saturating_sub(radius);
    let source_top = top.saturating_sub(radius);
    let source_right = right.saturating_add(radius).min(pixmap.width());
    let source_bottom = bottom.saturating_add(radius).min(pixmap.height());
    let source_width = source_right - source_left;
    let source_height = source_bottom - source_top;
    let stride = source_width as usize + 1;
    let mut sums = vec![[0u64; 4]; stride * (source_height as usize + 1)];

    for sy in 0..source_height {
        let mut row = [0u64; 4];
        for sx in 0..source_width {
            let pixel = pixmap.pixel(source_left + sx, source_top + sy).expect("bounded pixel");
            row[0] += u64::from(pixel.red());
            row[1] += u64::from(pixel.green());
            row[2] += u64::from(pixel.blue());
            row[3] += u64::from(pixel.alpha());
            let above = sums[sy as usize * stride + sx as usize + 1];
            sums[(sy as usize + 1) * stride + sx as usize + 1] =
                [row[0] + above[0], row[1] + above[1], row[2] + above[2], row[3] + above[3]];
        }
    }

    let width = pixmap.width();
    for py in top..bottom {
        for px in left..right {
            let x0 = px.saturating_sub(radius).max(source_left) - source_left;
            let y0 = py.saturating_sub(radius).max(source_top) - source_top;
            let x1 = px.saturating_add(radius + 1).min(source_right) - source_left;
            let y1 = py.saturating_add(radius + 1).min(source_bottom) - source_top;
            let a = sums[y0 as usize * stride + x0 as usize];
            let b = sums[y0 as usize * stride + x1 as usize];
            let c = sums[y1 as usize * stride + x0 as usize];
            let d = sums[y1 as usize * stride + x1 as usize];
            let count = u64::from((x1 - x0) * (y1 - y0));
            let channel =
                |index: usize| ((d[index] + a[index] - b[index] - c[index]) / count) as u8;
            let color = Color::from_rgba8(channel(0), channel(1), channel(2), channel(3));
            pixmap.pixels_mut()[(py * width + px) as usize] = color.premultiply().to_color_u8();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn export_empty_scene_is_png() {
        let bgra = vec![0u8, 0, 255, 255, 0, 0, 255, 255, 0, 0, 255, 255, 0, 0, 255, 255];
        let png = export_png(2, 2, &bgra, &AnnotationScene::default()).unwrap();
        assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n");
    }

    #[test]
    fn blur_is_not_the_mosaic_effect() {
        let mut bgra = Vec::new();
        for y in 0..24u8 {
            for x in 0..24u8 {
                bgra.extend_from_slice(&[x.wrapping_mul(9), y.wrapping_mul(7), x ^ y, 255]);
            }
        }
        let annotation = |kind| AnnotationScene {
            items: vec![Annotation {
                id: "effect".into(),
                kind,
                geometry: vec![2.0, 2.0, 20.0, 20.0],
                style: serde_json::json!({"radius": 3}),
                z_index: 0,
            }],
        };
        let mosaic = export_png(24, 24, &bgra, &annotation(AnnotationKind::Mosaic)).unwrap();
        let blur = export_png(24, 24, &bgra, &annotation(AnnotationKind::Blur)).unwrap();
        assert_ne!(mosaic, blur);
    }
}
