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
        draw_annotation(&mut pixmap, &ann);
    }
    let mut img = RgbaImage::new(width, height);
    for y in 0..height {
        for x in 0..width {
            let p = pixmap.pixel(x, y).unwrap().demultiply();
            img.put_pixel(x, y, Rgba([p.red(), p.green(), p.blue(), p.alpha()]));
        }
    }
    let mut out = Vec::new();
    image::DynamicImage::ImageRgba8(img)
        .write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png)
        .map_err(|e| e.to_string())?;
    Ok(out)
}

/// Overlay 预览与最终导出共用同一套绘制，避免"看到的"与"导出的"不一致。
pub fn draw_annotation(pixmap: &mut Pixmap, ann: &Annotation) {
    let mut paint = Paint::default();
    // 默认墨色与前端 AnnotatePage 的 MARK_COLOR、overlay 工具一致（macOS systemRed），
    // 否则预览和导出会出现两种红。
    let r = ann.style.get("color_r").and_then(|v| v.as_u64()).unwrap_or(255) as u8;
    let g = ann.style.get("color_g").and_then(|v| v.as_u64()).unwrap_or(59) as u8;
    let b = ann.style.get("color_b").and_then(|v| v.as_u64()).unwrap_or(48) as u8;
    let width = ann.style.get("stroke_width").and_then(|v| v.as_f64()).unwrap_or(3.0) as f32;
    paint.set_color_rgba8(r, g, b, 255);
    paint.anti_alias = true;

    match ann.kind {
        AnnotationKind::Rectangle if ann.geometry.len() >= 4 => {
            let rw = ann.geometry[2] as f32;
            let rh = ann.geometry[3] as f32;
            if rw <= 0.0 || rh <= 0.0 {
                return;
            }
            if let Some(rect) =
                tiny_skia::Rect::from_xywh(ann.geometry[0] as f32, ann.geometry[1] as f32, rw, rh)
            {
                let path = PathBuilder::from_rect(rect);
                let stroke = Stroke { width, ..Stroke::default() };
                pixmap.stroke_path(&path, &paint, &stroke, Transform::identity(), None);
            }
        }
        AnnotationKind::Ellipse if ann.geometry.len() >= 4 => {
            let x = ann.geometry[0] as f32;
            let y = ann.geometry[1] as f32;
            let w = ann.geometry[2] as f32;
            let h = ann.geometry[3] as f32;
            if let Some(path) =
                path_ellipse(x + w / 2.0, y + h / 2.0, (w / 2.0).abs(), (h / 2.0).abs())
            {
                let stroke = Stroke { width, ..Stroke::default() };
                pixmap.stroke_path(&path, &paint, &stroke, Transform::identity(), None);
            }
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
                let stroke = Stroke { width, ..Stroke::default() };
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
                let stroke = Stroke { width, ..Stroke::default() };
                pixmap.stroke_path(&path, &paint, &stroke, Transform::identity(), None);
            }
        }
        AnnotationKind::Mosaic => {
            if let Some(mask) = mosaic_mask(ann) {
                apply_mosaic(pixmap, &mask);
            }
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
                draw_bitmap_text(
                    pixmap,
                    ann.geometry[0] as i32,
                    ann.geometry[1] as i32,
                    &text,
                    r,
                    g,
                    b,
                    text_pixel_scale(ann),
                );
            }
        }
        _ => {}
    }
}

fn path_ellipse(cx: f32, cy: f32, rx: f32, ry: f32) -> Option<tiny_skia::Path> {
    if rx <= 0.0 || ry <= 0.0 {
        return None;
    }
    let k = 0.552_284_8_f32;
    let ox = rx * k;
    let oy = ry * k;

    let mut pb = PathBuilder::new();
    pb.move_to(cx - rx, cy);
    pb.cubic_to(cx - rx, cy - oy, cx - ox, cy - ry, cx, cy - ry);
    pb.cubic_to(cx + ox, cy - ry, cx + rx, cy - oy, cx + rx, cy);
    pb.cubic_to(cx + rx, cy + oy, cx + ox, cy + ry, cx, cy + ry);
    pb.cubic_to(cx - ox, cy + ry, cx - rx, cy + oy, cx - rx, cy);
    pb.close();
    pb.finish()
}

fn annotation_text(ann: &Annotation) -> String {
    ann.style
        .get("text")
        .or_else(|| ann.style.get("label"))
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string()
}

/// 位图字体的整数放大倍率。Retina 截图上 1:1 绘制会小到看不见。
pub fn text_pixel_scale(ann: &Annotation) -> u32 {
    ann.style.get("font_scale").and_then(|value| value.as_u64()).unwrap_or(1).clamp(1, 16) as u32
}

/// 字形宽 5 像素 + 1 像素间距，高 7 像素，按 scale 整数放大。
pub const GLYPH_ADVANCE: i32 = 6;
pub const GLYPH_HEIGHT: i32 = 7;

pub fn measure_bitmap_text(text: &str, scale: u32) -> (i32, i32) {
    let count = text.chars().take(128).count() as i32;
    let width = (count * GLYPH_ADVANCE - 1).max(0) * scale as i32;
    (width, GLYPH_HEIGHT * scale as i32)
}

#[allow(clippy::too_many_arguments)]
fn draw_bitmap_text(
    pixmap: &mut Pixmap,
    x: i32,
    y: i32,
    text: &str,
    r: u8,
    g: u8,
    b: u8,
    scale: u32,
) {
    draw_text(pixmap, x, y, text, (r, g, b), scale);
}

/// HUD 与导出共用的位图文字绘制。
pub fn draw_text(pixmap: &mut Pixmap, x: i32, y: i32, text: &str, rgb: (u8, u8, u8), scale: u32) {
    let (r, g, b) = rgb;
    let step = scale.max(1) as i32;
    let mut cx = x;
    let cy = y;
    for ch in text.chars().take(128) {
        let glyph = glyph_for(ch);
        for (row, bits) in glyph.iter().enumerate() {
            for col in 0..5 {
                if bits & (1 << (4 - col)) != 0 {
                    for dy in 0..step {
                        for dx in 0..step {
                            put_pixel(
                                pixmap,
                                cx + col * step + dx,
                                cy + row as i32 * step + dy,
                                r,
                                g,
                                b,
                            );
                        }
                    }
                }
            }
        }
        cx += GLYPH_ADVANCE * step;
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
        'B' => [0x1E, 0x11, 0x11, 0x1E, 0x11, 0x11, 0x1E],
        'C' => [0x0E, 0x11, 0x10, 0x10, 0x10, 0x11, 0x0E],
        'D' => [0x1C, 0x12, 0x11, 0x11, 0x11, 0x12, 0x1C],
        'E' => [0x1F, 0x10, 0x10, 0x1E, 0x10, 0x10, 0x1F],
        'F' => [0x1F, 0x10, 0x10, 0x1E, 0x10, 0x10, 0x10],
        'G' => [0x0E, 0x11, 0x10, 0x17, 0x11, 0x11, 0x0F],
        'H' => [0x11, 0x11, 0x11, 0x1F, 0x11, 0x11, 0x11],
        'I' => [0x0E, 0x04, 0x04, 0x04, 0x04, 0x04, 0x0E],
        'J' => [0x07, 0x02, 0x02, 0x02, 0x12, 0x12, 0x0C],
        'K' => [0x11, 0x12, 0x14, 0x18, 0x14, 0x12, 0x11],
        'L' => [0x10, 0x10, 0x10, 0x10, 0x10, 0x10, 0x1F],
        'M' => [0x11, 0x1B, 0x15, 0x15, 0x11, 0x11, 0x11],
        'N' => [0x11, 0x19, 0x15, 0x13, 0x11, 0x11, 0x11],
        'O' => [0x0E, 0x11, 0x11, 0x11, 0x11, 0x11, 0x0E],
        'P' => [0x1E, 0x11, 0x11, 0x1E, 0x10, 0x10, 0x10],
        'Q' => [0x0E, 0x11, 0x11, 0x11, 0x15, 0x12, 0x0D],
        'R' => [0x1E, 0x11, 0x11, 0x1E, 0x14, 0x12, 0x11],
        'S' => [0x0F, 0x10, 0x10, 0x0E, 0x01, 0x01, 0x1E],
        'T' => [0x1F, 0x04, 0x04, 0x04, 0x04, 0x04, 0x04],
        'U' => [0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x0E],
        'V' => [0x11, 0x11, 0x11, 0x11, 0x11, 0x0A, 0x04],
        'W' => [0x11, 0x11, 0x11, 0x15, 0x15, 0x1B, 0x11],
        'X' => [0x11, 0x11, 0x0A, 0x04, 0x0A, 0x11, 0x11],
        'Y' => [0x11, 0x11, 0x0A, 0x04, 0x04, 0x04, 0x04],
        'Z' => [0x1F, 0x01, 0x02, 0x04, 0x08, 0x10, 0x1F],
        '0' => [0x0E, 0x11, 0x13, 0x15, 0x19, 0x11, 0x0E],
        '1' => [0x04, 0x0C, 0x04, 0x04, 0x04, 0x04, 0x0E],
        '2' => [0x0E, 0x11, 0x01, 0x06, 0x08, 0x10, 0x1F],
        '3' => [0x1F, 0x02, 0x04, 0x02, 0x01, 0x11, 0x0E],
        '4' => [0x02, 0x06, 0x0A, 0x12, 0x1F, 0x02, 0x02],
        '5' => [0x1F, 0x10, 0x1E, 0x01, 0x01, 0x11, 0x0E],
        '6' => [0x06, 0x08, 0x10, 0x1E, 0x11, 0x11, 0x0E],
        '7' => [0x1F, 0x01, 0x02, 0x04, 0x08, 0x08, 0x08],
        '8' => [0x0E, 0x11, 0x11, 0x0E, 0x11, 0x11, 0x0E],
        '9' => [0x0E, 0x11, 0x11, 0x0F, 0x01, 0x02, 0x0C],
        ':' => [0x00, 0x04, 0x00, 0x00, 0x04, 0x00, 0x00],
        '.' => [0x00, 0x00, 0x00, 0x00, 0x00, 0x04, 0x00],
        ',' => [0x00, 0x00, 0x00, 0x00, 0x00, 0x04, 0x08],
        '#' => [0x0A, 0x0A, 0x1F, 0x0A, 0x1F, 0x0A, 0x0A],
        '(' => [0x02, 0x04, 0x08, 0x08, 0x08, 0x04, 0x02],
        ')' => [0x08, 0x04, 0x02, 0x02, 0x02, 0x04, 0x08],
        '-' => [0x00, 0x00, 0x00, 0x1F, 0x00, 0x00, 0x00],
        '_' => [0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x1F],
        '+' => [0x00, 0x04, 0x04, 0x1F, 0x04, 0x04, 0x00],
        '=' => [0x00, 0x00, 0x1F, 0x00, 0x1F, 0x00, 0x00],
        '/' => [0x01, 0x02, 0x02, 0x04, 0x08, 0x08, 0x10],
        '\\' => [0x10, 0x08, 0x08, 0x04, 0x02, 0x02, 0x01],
        '!' => [0x04, 0x04, 0x04, 0x04, 0x04, 0x00, 0x04],
        '?' => [0x0E, 0x11, 0x01, 0x06, 0x04, 0x00, 0x04],
        '\'' => [0x04, 0x04, 0x00, 0x00, 0x00, 0x00, 0x00],
        '"' => [0x0A, 0x0A, 0x00, 0x00, 0x00, 0x00, 0x00],
        '*' => [0x00, 0x0A, 0x04, 0x1F, 0x04, 0x0A, 0x00],
        '%' => [0x11, 0x12, 0x04, 0x04, 0x04, 0x09, 0x11],
        ';' => [0x00, 0x04, 0x00, 0x00, 0x04, 0x04, 0x08],
        '<' => [0x02, 0x04, 0x08, 0x10, 0x08, 0x04, 0x02],
        '>' => [0x10, 0x08, 0x04, 0x02, 0x04, 0x08, 0x10],
        '[' => [0x0E, 0x08, 0x08, 0x08, 0x08, 0x08, 0x0E],
        ']' => [0x0E, 0x02, 0x02, 0x02, 0x02, 0x02, 0x0E],
        // 位图字体只覆盖 ASCII。输入侧已过滤非 ASCII，这里的方框只是最后兜底。
        _ => [0x1F, 0x11, 0x11, 0x11, 0x11, 0x11, 0x1F],
    }
}

/// 马赛克覆盖格子。格子对齐到图片原点，因此重叠笔画不会互相错位，
/// 也不会因为反复采样而出现"越涂越糊"的拖影。
#[derive(Clone, Debug)]
pub struct MosaicMask {
    pub block: u32,
    /// 覆盖到的格子坐标（格子索引，不是像素坐标）。
    pub cells: Vec<(i32, i32)>,
}

/// 默认块边长。相对源图尺寸取值，保证马赛克在 Retina 截图上也足够粗。
pub const DEFAULT_MOSAIC_BLOCK: u32 = 16;

/// 把马赛克标注的几何转换成对齐格子集合。Overlay 实时预览与导出共用，
/// 保证"涂到哪里"和"最终糊掉哪里"完全一致。
pub fn mosaic_mask(ann: &Annotation) -> Option<MosaicMask> {
    if !matches!(ann.kind, AnnotationKind::Mosaic) {
        return None;
    }
    let block = ann
        .style
        .get("block_size")
        .and_then(|v| v.as_u64())
        .unwrap_or(u64::from(DEFAULT_MOSAIC_BLOCK))
        .clamp(4, 96) as u32;
    let is_brush = ann.style.get("brush_radius").is_some();

    let mut rects: Vec<(f64, f64, f64, f64)> = Vec::new();
    if !is_brush && ann.geometry.len() == 4 {
        rects.push((ann.geometry[0], ann.geometry[1], ann.geometry[2], ann.geometry[3]));
    } else if ann.geometry.len() >= 2 {
        let radius = ann
            .style
            .get("brush_radius")
            .and_then(|v| v.as_f64())
            .unwrap_or(f64::from(DEFAULT_MOSAIC_BLOCK))
            .max(1.0);
        let points: Vec<(f64, f64)> =
            ann.geometry.chunks_exact(2).map(|chunk| (chunk[0], chunk[1])).collect();
        // 逐点画方块会在快速拖动时留下断点，所以沿相邻点之间补插。
        for pair in points.windows(2) {
            let (x0, y0) = pair[0];
            let (x1, y1) = pair[1];
            let distance = ((x1 - x0).powi(2) + (y1 - y0).powi(2)).sqrt();
            let steps = (distance / (f64::from(block).max(1.0) / 2.0)).ceil().max(1.0);
            for step in 0..=(steps as u32) {
                let t = f64::from(step) / steps;
                let cx = x0 + (x1 - x0) * t;
                let cy = y0 + (y1 - y0) * t;
                rects.push((cx - radius, cy - radius, radius * 2.0, radius * 2.0));
            }
        }
        if points.len() == 1 {
            let (cx, cy) = points[0];
            rects.push((cx - radius, cy - radius, radius * 2.0, radius * 2.0));
        }
    }
    if rects.is_empty() {
        return None;
    }

    let mut cells: Vec<(i32, i32)> = Vec::new();
    let block_f = f64::from(block);
    for (x, y, w, h) in rects {
        if w <= 0.0 || h <= 0.0 {
            continue;
        }
        let cx0 = (x / block_f).floor() as i32;
        let cy0 = (y / block_f).floor() as i32;
        let cx1 = ((x + w) / block_f).ceil() as i32;
        let cy1 = ((y + h) / block_f).ceil() as i32;
        for cy in cy0..cy1 {
            for cx in cx0..cx1 {
                cells.push((cx, cy));
            }
        }
    }
    cells.sort_unstable();
    cells.dedup();
    Some(MosaicMask { block, cells })
}

/// 对每个格子做块内平均后整块填充：这才是不透明、看不出原内容的马赛克。
pub fn apply_mosaic(pixmap: &mut Pixmap, mask: &MosaicMask) {
    let width = pixmap.width();
    let height = pixmap.height();
    let block = mask.block.max(1) as i32;
    for &(cell_x, cell_y) in &mask.cells {
        let x0 = (cell_x * block).clamp(0, width as i32);
        let y0 = (cell_y * block).clamp(0, height as i32);
        let x1 = (cell_x * block + block).clamp(0, width as i32);
        let y1 = (cell_y * block + block).clamp(0, height as i32);
        if x0 >= x1 || y0 >= y1 {
            continue;
        }
        let mut sum = [0u64; 3];
        let mut count = 0u64;
        for y in y0..y1 {
            for x in x0..x1 {
                let p = pixmap.pixels()[(y as u32 * width + x as u32) as usize].demultiply();
                sum[0] += u64::from(p.red());
                sum[1] += u64::from(p.green());
                sum[2] += u64::from(p.blue());
                count += 1;
            }
        }
        if count == 0 {
            continue;
        }
        let color = Color::from_rgba8(
            (sum[0] / count) as u8,
            (sum[1] / count) as u8,
            (sum[2] / count) as u8,
            255,
        )
        .premultiply()
        .to_color_u8();
        for y in y0..y1 {
            for x in x0..x1 {
                pixmap.pixels_mut()[(y as u32 * width + x as u32) as usize] = color;
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

    fn gradient_bgra(size: u32) -> Vec<u8> {
        let mut bgra = Vec::with_capacity((size * size * 4) as usize);
        for y in 0..size {
            for x in 0..size {
                bgra.extend_from_slice(&[(x * 3) as u8, (y * 3) as u8, (x ^ y) as u8, 255]);
            }
        }
        bgra
    }

    fn mosaic_annotation(style: serde_json::Value, geometry: Vec<f64>) -> Annotation {
        Annotation {
            id: "mosaic".into(),
            kind: AnnotationKind::Mosaic,
            geometry,
            style,
            z_index: 0,
        }
    }

    #[test]
    fn mosaic_blocks_are_uniform_and_fully_opaque() {
        let size = 64;
        let mut pixmap = Pixmap::new(size, size).unwrap();
        for y in 0..size {
            for x in 0..size {
                let color = Color::from_rgba8((x * 4) as u8, (y * 4) as u8, 90, 255);
                pixmap.pixels_mut()[(y * size + x) as usize] = color.premultiply().to_color_u8();
            }
        }
        let ann =
            mosaic_annotation(serde_json::json!({"block_size": 16}), vec![0.0, 0.0, 32.0, 32.0]);
        draw_annotation(&mut pixmap, &ann);

        // 每个 16x16 格子内所有像素必须完全一致，且 alpha 不透明。
        for cell_y in 0..2u32 {
            for cell_x in 0..2u32 {
                let anchor = pixmap.pixel(cell_x * 16, cell_y * 16).unwrap();
                assert_eq!(anchor.alpha(), 255, "mosaic must stay opaque");
                for y in 0..16 {
                    for x in 0..16 {
                        let p = pixmap.pixel(cell_x * 16 + x, cell_y * 16 + y).unwrap();
                        assert_eq!(p, anchor, "mosaic block must be a single flat colour");
                    }
                }
            }
        }
        // 未覆盖区域保持原样。
        assert_ne!(pixmap.pixel(40, 40).unwrap(), pixmap.pixel(0, 0).unwrap());
    }

    #[test]
    fn mosaic_grid_is_anchored_to_image_origin_regardless_of_stroke_offset() {
        let size = 48;
        let bgra = gradient_bgra(size);
        let render = |points: Vec<f64>| {
            let mut pixmap = Pixmap::new(size, size).unwrap();
            for y in 0..size {
                for x in 0..size {
                    let i = ((y * size + x) * 4) as usize;
                    let color = Color::from_rgba8(bgra[i + 2], bgra[i + 1], bgra[i], bgra[i + 3]);
                    pixmap.pixels_mut()[(y * size + x) as usize] =
                        color.premultiply().to_color_u8();
                }
            }
            let ann = mosaic_annotation(
                serde_json::json!({"block_size": 12, "brush_radius": 12.0}),
                points,
            );
            draw_annotation(&mut pixmap, &ann);
            pixmap
        };
        // 两条同区域但采样点不同的笔画，落到同一格子网格上，中心结果一致。
        let sparse = render(vec![14.0, 24.0, 34.0, 24.0]);
        let dense = render(vec![14.0, 24.0, 24.0, 24.0, 34.0, 24.0]);
        assert_eq!(sparse.pixel(24, 24), dense.pixel(24, 24));
    }

    #[test]
    fn brush_mosaic_fills_gaps_between_sparse_stroke_points() {
        let size = 96;
        let bgra = gradient_bgra(size);
        let scene = AnnotationScene {
            items: vec![mosaic_annotation(
                serde_json::json!({"block_size": 8, "brush_radius": 8.0}),
                // 快速拖动只会采到相距很远的两点。
                vec![16.0, 48.0, 80.0, 48.0],
            )],
        };
        let png = export_png(size, size, &bgra, &scene).unwrap();
        let out = image::load_from_memory(&png).unwrap().to_rgba8();
        let mid = out.get_pixel(48, 48);
        let original_index = ((48 * size + 48) * 4) as usize;
        let original = [bgra[original_index + 2], bgra[original_index + 1], bgra[original_index]];
        assert_ne!(
            [mid[0], mid[1], mid[2]],
            original,
            "stroke midpoint must be mosaicked, not skipped"
        );
    }
}
