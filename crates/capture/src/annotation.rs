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
        AnnotationKind::Mosaic | AnnotationKind::Blur if ann.geometry.len() >= 4 => {
            apply_block(pixmap, ann.geometry[0], ann.geometry[1], ann.geometry[2], ann.geometry[3]);
        }
        _ => {}
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn export_empty_scene_is_png() {
        let bgra = vec![0u8, 0, 255, 255, 0, 0, 255, 255, 0, 0, 255, 255, 0, 0, 255, 255];
        let png = export_png(2, 2, &bgra, &AnnotationScene::default()).unwrap();
        assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n");
    }
}
