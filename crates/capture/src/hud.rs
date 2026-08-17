//! 冻结层 HUD：工具栏、选区装饰、放大镜。
//!
//! 三条硬规则：
//! 1. 始终深色玻璃，不跟随应用主题——HUD 会叠在任意桌面内容上，浅色工具栏不可读。
//! 2. 所有尺寸以逻辑像素声明，绘制时统一乘 `scale_factor`；Retina 上不能画成一半大小。
//! 3. 工具栏只用图标，命中测试与绘制共用同一份 layout，不允许两处各写一套魔法数字。

use tiny_skia::{
    Color, FillRule, LineCap, LineJoin, Paint, Path, PathBuilder, Pixmap, PremultipliedColorU8,
    Rect, Stroke, Transform,
};

use crate::annotation::draw_text;
use crate::overlay::ActiveTool;

/// 物理像素矩形。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Area {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

impl Area {
    pub fn contains(&self, x: f64, y: f64) -> bool {
        x >= self.x && x <= self.x + self.width && y >= self.y && y <= self.y + self.height
    }

    pub fn right(&self) -> f64 {
        self.x + self.width
    }

    pub fn bottom(&self) -> f64 {
        self.y + self.height
    }
}

// ——— 逻辑像素度量 ———
const BUTTON: f64 = 34.0;
const BUTTON_GAP: f64 = 2.0;
const DIVIDER_SLOT: f64 = 11.0;
const BAR_PAD: f64 = 6.0;
const BAR_HEIGHT: f64 = BUTTON + BAR_PAD * 2.0;
const BAR_RADIUS: f64 = 12.0;
const BAR_GAP: f64 = 10.0;
const ICON: f64 = 18.0;
const HANDLE: f64 = 5.0;
const BORDER: f64 = 1.5;
const TAG_HEIGHT: f64 = 22.0;
const TAG_PAD: f64 = 7.0;

// ——— 深色 HUD 配色 ———
const SURFACE: (u8, u8, u8, u8) = (28, 28, 30, 242);
const SURFACE_EDGE: (u8, u8, u8, u8) = (255, 255, 255, 26);
const ICON_FG: (u8, u8, u8, u8) = (235, 235, 245, 230);
const ICON_DISABLED: (u8, u8, u8, u8) = (235, 235, 245, 72);
const HOVER_BG: (u8, u8, u8, u8) = (255, 255, 255, 28);
const ACCENT: (u8, u8, u8, u8) = (10, 132, 255, 255);
const DANGER: (u8, u8, u8, u8) = (255, 69, 58, 255);
const SUCCESS: (u8, u8, u8, u8) = (48, 209, 88, 255);
const WHITE: (u8, u8, u8, u8) = (255, 255, 255, 255);
const TEXT_FG: (u8, u8, u8) = (235, 235, 245);
const TEXT_DIM: (u8, u8, u8) = (152, 152, 160);

type Rgba = (u8, u8, u8, u8);

fn rgba((r, g, b, a): Rgba) -> Color {
    Color::from_rgba8(r, g, b, a)
}

fn fade(color: Rgba, factor: f32) -> Rgba {
    let (r, g, b, a) = color;
    (r, g, b, ((f32::from(a) * factor).clamp(0.0, 255.0)) as u8)
}

/// 工具栏动作。命中测试直接返回它，避免"按索引猜按钮"。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToolbarAction {
    SetTool(ActiveTool),
    Scroll,
    Undo,
    Save,
    Pin,
    Cancel,
    Done,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Slot {
    Button(ToolbarAction),
    Divider,
}

const SLOTS: &[Slot] = &[
    Slot::Button(ToolbarAction::SetTool(ActiveTool::Rectangle)),
    Slot::Button(ToolbarAction::SetTool(ActiveTool::Ellipse)),
    Slot::Button(ToolbarAction::SetTool(ActiveTool::Arrow)),
    Slot::Button(ToolbarAction::SetTool(ActiveTool::Brush)),
    Slot::Button(ToolbarAction::SetTool(ActiveTool::Mosaic)),
    Slot::Button(ToolbarAction::SetTool(ActiveTool::Text)),
    Slot::Divider,
    Slot::Button(ToolbarAction::Scroll),
    Slot::Button(ToolbarAction::Undo),
    Slot::Divider,
    Slot::Button(ToolbarAction::Save),
    Slot::Button(ToolbarAction::Pin),
    Slot::Button(ToolbarAction::Cancel),
    Slot::Button(ToolbarAction::Done),
];

/// HUD 绘制尺度。所有 `px()` 结果都是物理像素。
#[derive(Clone, Copy, Debug)]
pub struct Hud {
    scale: f64,
}

impl Hud {
    pub fn new(scale: f64) -> Self {
        Self { scale: scale.clamp(1.0, 4.0) }
    }

    pub fn scale(&self) -> f64 {
        self.scale
    }

    pub fn px(&self, logical: f64) -> f64 {
        logical * self.scale
    }

    /// 位图字体的整数放大倍率。
    pub fn text_scale(&self) -> u32 {
        (self.scale.round() as u32).clamp(1, 4) * 2
    }

    pub fn stroke_px(&self) -> f64 {
        self.px(BORDER).max(1.0)
    }
}

/// 工具栏布局。绘制与命中测试都从这里取几何。
#[derive(Clone, Debug)]
pub struct ToolbarLayout {
    pub bar: Area,
    slots: Vec<(Slot, Area)>,
    hud: Hud,
}

impl ToolbarLayout {
    pub fn compute(selection: Area, surface_w: f64, surface_h: f64, hud: Hud) -> Self {
        let button = hud.px(BUTTON);
        let gap = hud.px(BUTTON_GAP);
        let divider = hud.px(DIVIDER_SLOT);
        let pad = hud.px(BAR_PAD);
        let height = hud.px(BAR_HEIGHT);
        let bar_gap = hud.px(BAR_GAP);

        let mut content = 0.0;
        for (index, slot) in SLOTS.iter().enumerate() {
            if index > 0 {
                content += gap;
            }
            content += match slot {
                Slot::Button(_) => button,
                Slot::Divider => divider,
            };
        }
        let width = content + pad * 2.0;

        // 优先贴在选区下方并右对齐；下方不够放上方；都不够就压在屏幕底部。
        let mut y = selection.bottom() + bar_gap;
        if y + height > surface_h - bar_gap {
            let above = selection.y - height - bar_gap;
            y = if above >= bar_gap { above } else { (surface_h - height - bar_gap).max(bar_gap) };
        }
        let x = (selection.right() - width).clamp(bar_gap, (surface_w - width - bar_gap).max(0.0));
        let bar = Area { x, y, width, height };

        let mut slots = Vec::with_capacity(SLOTS.len());
        let mut cursor = bar.x + pad;
        for (index, slot) in SLOTS.iter().enumerate() {
            if index > 0 {
                cursor += gap;
            }
            let slot_width = match slot {
                Slot::Button(_) => button,
                Slot::Divider => divider,
            };
            slots.push((
                *slot,
                Area { x: cursor, y: bar.y + pad, width: slot_width, height: button },
            ));
            cursor += slot_width;
        }
        Self { bar, slots, hud }
    }

    pub fn contains(&self, x: f64, y: f64) -> bool {
        self.bar.contains(x, y)
    }

    pub fn hit(&self, x: f64, y: f64) -> Option<ToolbarAction> {
        if !self.contains(x, y) {
            return None;
        }
        self.slots.iter().find_map(|(slot, area)| match slot {
            Slot::Button(action) if area.contains(x, y) => Some(*action),
            _ => None,
        })
    }
}

/// 工具栏状态。
#[derive(Clone, Copy, Debug)]
pub struct ToolbarState {
    pub active_tool: ActiveTool,
    pub hovered: Option<ToolbarAction>,
    pub can_undo: bool,
    pub can_scroll: bool,
}

pub fn draw_toolbar(
    buf: &mut [u32],
    surface_w: u32,
    surface_h: u32,
    layout: &ToolbarLayout,
    state: ToolbarState,
) {
    let hud = layout.hud;
    let bar = layout.bar;
    let Some(mut pixmap) = Pixmap::new(bar.width.ceil() as u32, bar.height.ceil() as u32) else {
        return;
    };
    let radius = hud.px(BAR_RADIUS) as f32;
    let hairline = (hud.scale() as f32).max(1.0);

    fill(&mut pixmap, rounded_rect(0.0, 0.0, bar.width as f32, bar.height as f32, radius), SURFACE);
    stroke(
        &mut pixmap,
        rounded_rect(
            hairline / 2.0,
            hairline / 2.0,
            bar.width as f32 - hairline,
            bar.height as f32 - hairline,
            radius,
        ),
        SURFACE_EDGE,
        hairline,
    );

    let icon = hud.px(ICON);
    for (slot, area) in &layout.slots {
        let local_x = area.x - bar.x;
        let local_y = area.y - bar.y;
        match slot {
            Slot::Divider => {
                let center = (local_x + area.width / 2.0) as f32;
                let inset = (area.height * 0.16) as f32;
                fill(
                    &mut pixmap,
                    rect_path(
                        center - hairline / 2.0,
                        local_y as f32 + inset,
                        hairline,
                        area.height as f32 - inset * 2.0,
                    ),
                    SURFACE_EDGE,
                );
            }
            Slot::Button(action) => {
                let enabled = match action {
                    ToolbarAction::Undo => state.can_undo,
                    ToolbarAction::Scroll => state.can_scroll,
                    _ => true,
                };
                let selected = matches!(action, ToolbarAction::SetTool(tool) if *tool == state.active_tool)
                    && state.active_tool != ActiveTool::None;
                let hovered = enabled && state.hovered == Some(*action);

                let shape = rounded_rect(
                    local_x as f32,
                    local_y as f32,
                    area.width as f32,
                    area.height as f32,
                    hud.px(9.0) as f32,
                );
                let mut fg = if enabled { ICON_FG } else { ICON_DISABLED };
                if selected {
                    fill(&mut pixmap, shape, ACCENT);
                    fg = WHITE;
                } else if matches!(action, ToolbarAction::Done) {
                    fill(&mut pixmap, shape, SUCCESS);
                    fg = WHITE;
                } else if hovered {
                    if matches!(action, ToolbarAction::Cancel) {
                        fill(&mut pixmap, shape, DANGER);
                        fg = WHITE;
                    } else {
                        fill(&mut pixmap, shape, HOVER_BG);
                    }
                }

                draw_icon(
                    &mut pixmap,
                    *action,
                    local_x + area.width / 2.0,
                    local_y + area.height / 2.0,
                    icon,
                    fg,
                    hud,
                );
            }
        }
    }

    blend_pixmap(buf, surface_w, surface_h, bar.x.round() as i32, bar.y.round() as i32, &pixmap);
}

/// 选区边框 + 8 个把手。
pub fn draw_selection_chrome(
    buf: &mut [u32],
    surface_w: u32,
    surface_h: u32,
    area: Area,
    hud: Hud,
) {
    stroke_area(buf, surface_w, surface_h, area, hud.stroke_px(), ACCENT);

    let r = hud.px(HANDLE) as f32;
    let size = (r * 2.0).ceil() as u32 + 4;
    let Some(mut handle) = Pixmap::new(size, size) else { return };
    let center = size as f32 / 2.0;
    let ring = (hud.scale() as f32 * 1.5).max(1.0);
    fill(&mut handle, circle_path(center, center, r), WHITE);
    stroke(&mut handle, circle_path(center, center, r - ring / 2.0), ACCENT, ring);

    for (hx, hy) in handle_points(area) {
        blend_pixmap(
            buf,
            surface_w,
            surface_h,
            (hx - f64::from(size) / 2.0).round() as i32,
            (hy - f64::from(size) / 2.0).round() as i32,
            &handle,
        );
    }
}

pub fn handle_points(area: Area) -> [(f64, f64); 8] {
    let (x, y, w, h) = (area.x, area.y, area.width, area.height);
    [
        (x, y),
        (x + w / 2.0, y),
        (x + w, y),
        (x, y + h / 2.0),
        (x + w, y + h / 2.0),
        (x, y + h),
        (x + w / 2.0, y + h),
        (x + w, y + h),
    ]
}

/// 吸附到窗口时的虚线提示框。
pub fn draw_snap_hint(buf: &mut [u32], surface_w: u32, surface_h: u32, area: Area, hud: Hud) {
    let thickness = hud.stroke_px().round().max(1.0) as i32;
    let dash = hud.px(6.0).round().max(2.0) as i32;
    let color = pack(ACCENT);
    let x0 = area.x.round() as i32;
    let y0 = area.y.round() as i32;
    let x1 = area.right().round() as i32;
    let y1 = area.bottom().round() as i32;
    for x in x0..=x1 {
        if (x - x0) / dash % 2 == 0 {
            for t in 0..thickness {
                put_pixel(buf, surface_w, surface_h, x, y0 + t, color);
                put_pixel(buf, surface_w, surface_h, x, y1 - t, color);
            }
        }
    }
    for y in y0..=y1 {
        if (y - y0) / dash % 2 == 0 {
            for t in 0..thickness {
                put_pixel(buf, surface_w, surface_h, x0 + t, y, color);
                put_pixel(buf, surface_w, surface_h, x1 - t, y, color);
            }
        }
    }
}

/// 选区尺寸标签。
pub fn draw_size_tag(
    buf: &mut [u32],
    surface_w: u32,
    surface_h: u32,
    area: Area,
    frame_scale: (f64, f64),
    hud: Hud,
) {
    let label = format!(
        "{} x {}",
        (area.width * frame_scale.0).round() as i64,
        (area.height * frame_scale.1).round() as i64
    );
    let text_scale = hud.text_scale();
    let (text_w, text_h) = crate::annotation::measure_bitmap_text(&label, text_scale);
    let pad = hud.px(TAG_PAD);
    let width = f64::from(text_w) + pad * 2.0;
    let height = hud.px(TAG_HEIGHT).max(f64::from(text_h) + pad);
    let gap = hud.px(6.0);

    let mut y = area.y - height - gap;
    if y < gap {
        y = (area.y + gap).min(f64::from(surface_h) - height - gap);
    }
    let x = area.x.clamp(gap, (f64::from(surface_w) - width - gap).max(0.0));

    let Some(mut pixmap) = Pixmap::new(width.ceil() as u32, height.ceil() as u32) else { return };
    fill(
        &mut pixmap,
        rounded_rect(0.0, 0.0, width as f32, height as f32, hud.px(6.0) as f32),
        SURFACE,
    );
    draw_text(
        &mut pixmap,
        pad.round() as i32,
        ((height - f64::from(text_h)) / 2.0).round() as i32,
        &label,
        TEXT_FG,
        text_scale,
    );
    blend_pixmap(buf, surface_w, surface_h, x.round() as i32, y.round() as i32, &pixmap);
}

/// 放大镜 + 取色 HUD。
#[allow(clippy::too_many_arguments)]
pub fn draw_magnifier(
    buf: &mut [u32],
    surface_w: u32,
    surface_h: u32,
    sample: impl Fn(i32, i32) -> (u8, u8, u8),
    cursor: (f64, f64),
    frame_point: (i32, i32),
    hud: Hud,
) {
    let cell = hud.px(7.0).round().max(3.0);
    let grid = 9i32;
    let zoom = cell * f64::from(grid);
    let pad = hud.px(8.0);
    let text_scale = hud.text_scale();
    let line = f64::from(crate::annotation::GLYPH_HEIGHT * text_scale as i32);
    let line_gap = hud.px(4.0);
    let swatch = hud.px(22.0);

    let width = zoom + pad * 3.0 + swatch;
    let height = zoom + pad * 2.0 + (line + line_gap) * 2.0;
    let offset = hud.px(18.0);

    let mut x = cursor.0 + offset;
    let mut y = cursor.1 + offset;
    if x + width > f64::from(surface_w) - offset {
        x = cursor.0 - width - offset;
    }
    if y + height > f64::from(surface_h) - offset {
        y = cursor.1 - height - offset;
    }
    x = x.max(0.0);
    y = y.max(0.0);

    let Some(mut pixmap) = Pixmap::new(width.ceil() as u32, height.ceil() as u32) else { return };
    fill(
        &mut pixmap,
        rounded_rect(0.0, 0.0, width as f32, height as f32, hud.px(10.0) as f32),
        SURFACE,
    );

    let half = grid / 2;
    for gy in -half..=half {
        for gx in -half..=half {
            let (r, g, b) = sample(frame_point.0 + gx, frame_point.1 + gy);
            fill(
                &mut pixmap,
                rect_path(
                    (pad + f64::from(gx + half) * cell) as f32,
                    (pad + f64::from(gy + half) * cell) as f32,
                    cell as f32,
                    cell as f32,
                ),
                (r, g, b, 255),
            );
        }
    }

    let hairline = (hud.scale() as f32).max(1.0);
    let center = pad + f64::from(half) * cell;
    stroke(
        &mut pixmap,
        rect_path(center as f32, center as f32, cell as f32, cell as f32),
        ACCENT,
        hairline,
    );
    stroke(
        &mut pixmap,
        rect_path(pad as f32, pad as f32, zoom as f32, zoom as f32),
        SURFACE_EDGE,
        hairline,
    );

    let (r, g, b) = sample(frame_point.0, frame_point.1);
    let swatch_x = pad * 2.0 + zoom;
    fill(
        &mut pixmap,
        rect_path(swatch_x as f32, pad as f32, swatch as f32, swatch as f32),
        (r, g, b, 255),
    );
    stroke(
        &mut pixmap,
        rect_path(swatch_x as f32, pad as f32, swatch as f32, swatch as f32),
        SURFACE_EDGE,
        hairline,
    );

    let text_y = pad + zoom + line_gap;
    draw_text(
        &mut pixmap,
        pad.round() as i32,
        text_y.round() as i32,
        &format!("{} {}", frame_point.0, frame_point.1),
        TEXT_DIM,
        text_scale,
    );
    draw_text(
        &mut pixmap,
        pad.round() as i32,
        (text_y + line + line_gap).round() as i32,
        &format!("#{r:02X}{g:02X}{b:02X}"),
        TEXT_FG,
        text_scale,
    );

    blend_pixmap(buf, surface_w, surface_h, x.round() as i32, y.round() as i32, &pixmap);
}

// ——— 图标 ———

#[allow(clippy::too_many_arguments)]
fn draw_icon(
    pixmap: &mut Pixmap,
    action: ToolbarAction,
    cx: f64,
    cy: f64,
    size: f64,
    color: Rgba,
    hud: Hud,
) {
    let s = size as f32;
    let cx = cx as f32;
    let cy = cy as f32;
    let width = (s * 0.115).max(hud.scale() as f32);
    let at = |nx: f32, ny: f32| (cx + nx * s, cy + ny * s);

    match action {
        ToolbarAction::SetTool(ActiveTool::Rectangle) => {
            let (x, y) = at(-0.43, -0.34);
            stroke(pixmap, rounded_rect(x, y, s * 0.86, s * 0.68, s * 0.1), color, width);
        }
        ToolbarAction::SetTool(ActiveTool::Ellipse) => {
            stroke(pixmap, ellipse_path(cx, cy, s * 0.43, s * 0.33), color, width);
        }
        ToolbarAction::SetTool(ActiveTool::Arrow) => {
            let tail = at(-0.36, 0.36);
            let head = at(0.36, -0.36);
            stroke(pixmap, polyline(&[tail, head]), color, width);
            arrow_head(pixmap, tail, head, s * 0.32, color, width);
        }
        ToolbarAction::SetTool(ActiveTool::Brush) => {
            let mut pb = PathBuilder::new();
            let (x0, y0) = at(-0.42, 0.26);
            let (c1x, c1y) = at(-0.16, -0.46);
            let (c2x, c2y) = at(0.12, 0.44);
            let (x1, y1) = at(0.42, -0.3);
            pb.move_to(x0, y0);
            pb.cubic_to(c1x, c1y, c2x, c2y, x1, y1);
            stroke(pixmap, pb.finish(), color, width);
        }
        ToolbarAction::SetTool(ActiveTool::Mosaic) => {
            // 3x3 棋盘：一眼认出是马赛克，而不是"井"字或网格。
            let cell = s * 0.29;
            let origin = at(-0.44, -0.44);
            for row in 0..3 {
                for col in 0..3 {
                    let tone = if (row + col) % 2 == 0 { color } else { fade(color, 0.34) };
                    fill(
                        pixmap,
                        rect_path(
                            origin.0 + col as f32 * cell,
                            origin.1 + row as f32 * cell,
                            cell * 0.84,
                            cell * 0.84,
                        ),
                        tone,
                    );
                }
            }
        }
        ToolbarAction::SetTool(ActiveTool::Text) => {
            stroke(pixmap, polyline(&[at(-0.34, -0.32), at(0.34, -0.32)]), color, width);
            stroke(pixmap, polyline(&[at(0.0, -0.32), at(0.0, 0.36)]), color, width);
        }
        ToolbarAction::SetTool(ActiveTool::None) => {}
        ToolbarAction::Scroll => {
            stroke(pixmap, polyline(&[at(-0.3, -0.4), at(0.3, -0.4)]), color, width);
            stroke(pixmap, polyline(&[at(0.0, -0.22), at(0.0, 0.3)]), color, width);
            stroke(
                pixmap,
                polyline(&[at(-0.22, 0.06), at(0.0, 0.32), at(0.22, 0.06)]),
                color,
                width,
            );
        }
        ToolbarAction::Undo => {
            let points = arc_points(cx, cy, s * 0.33, 300.0, 20.0, 26);
            stroke(pixmap, polyline(&points), color, width);
            if points.len() >= 2 {
                let last = points[points.len() - 1];
                let prev = points[points.len() - 2];
                arrow_head(pixmap, prev, last, s * 0.26, color, width);
            }
        }
        ToolbarAction::Save => {
            stroke(pixmap, polyline(&[at(0.0, -0.4), at(0.0, 0.14)]), color, width);
            stroke(
                pixmap,
                polyline(&[at(-0.22, -0.08), at(0.0, 0.16), at(0.22, -0.08)]),
                color,
                width,
            );
            stroke(pixmap, polyline(&[at(-0.34, 0.36), at(0.34, 0.36)]), color, width);
        }
        ToolbarAction::Pin => {
            let head = at(0.0, -0.14);
            stroke(pixmap, circle_path(head.0, head.1, s * 0.17), color, width);
            stroke(pixmap, polyline(&[at(0.0, 0.04), at(0.0, 0.4)]), color, width);
        }
        ToolbarAction::Cancel => {
            stroke(pixmap, polyline(&[at(-0.28, -0.28), at(0.28, 0.28)]), color, width);
            stroke(pixmap, polyline(&[at(0.28, -0.28), at(-0.28, 0.28)]), color, width);
        }
        ToolbarAction::Done => {
            stroke(
                pixmap,
                polyline(&[at(-0.3, 0.02), at(-0.08, 0.26), at(0.32, -0.26)]),
                color,
                width,
            );
        }
    }
}

fn arrow_head(
    pixmap: &mut Pixmap,
    from: (f32, f32),
    to: (f32, f32),
    len: f32,
    color: Rgba,
    width: f32,
) {
    let dx = to.0 - from.0;
    let dy = to.1 - from.1;
    let norm = (dx * dx + dy * dy).sqrt();
    if norm <= f32::EPSILON {
        return;
    }
    let (ux, uy) = (dx / norm, dy / norm);
    let spread = 0.55_f32;
    let (c, s) = (spread.cos(), spread.sin());
    let left = (to.0 - len * (ux * c - uy * s), to.1 - len * (ux * s + uy * c));
    let right = (to.0 - len * (ux * c + uy * s), to.1 - len * (-ux * s + uy * c));
    stroke(pixmap, polyline(&[left, to, right]), color, width);
}

fn arc_points(
    cx: f32,
    cy: f32,
    r: f32,
    start_deg: f32,
    end_deg: f32,
    segments: usize,
) -> Vec<(f32, f32)> {
    let sweep =
        if end_deg >= start_deg { end_deg - start_deg } else { end_deg + 360.0 - start_deg };
    (0..=segments)
        .map(|i| {
            let angle = (start_deg + sweep * (i as f32 / segments as f32)).to_radians();
            (cx + r * angle.cos(), cy + r * angle.sin())
        })
        .collect()
}

// ——— tiny-skia 绘制原语 ———

fn fill(pixmap: &mut Pixmap, path: Option<Path>, color: Rgba) {
    let Some(path) = path else { return };
    let mut paint = Paint { anti_alias: true, ..Paint::default() };
    paint.set_color(rgba(color));
    pixmap.fill_path(&path, &paint, FillRule::Winding, Transform::identity(), None);
}

fn stroke(pixmap: &mut Pixmap, path: Option<Path>, color: Rgba, width: f32) {
    let Some(path) = path else { return };
    let mut paint = Paint { anti_alias: true, ..Paint::default() };
    paint.set_color(rgba(color));
    let stroke = Stroke {
        width: width.max(0.5),
        line_cap: LineCap::Round,
        line_join: LineJoin::Round,
        ..Stroke::default()
    };
    pixmap.stroke_path(&path, &paint, &stroke, Transform::identity(), None);
}

fn polyline(points: &[(f32, f32)]) -> Option<Path> {
    if points.len() < 2 {
        return None;
    }
    let mut pb = PathBuilder::new();
    pb.move_to(points[0].0, points[0].1);
    for point in &points[1..] {
        pb.line_to(point.0, point.1);
    }
    pb.finish()
}

fn rounded_rect(x: f32, y: f32, w: f32, h: f32, r: f32) -> Option<Path> {
    if w <= 0.0 || h <= 0.0 {
        return None;
    }
    let r = r.min(w / 2.0).min(h / 2.0).max(0.0);
    let k = r * 0.5523;
    let mut pb = PathBuilder::new();
    pb.move_to(x + r, y);
    pb.line_to(x + w - r, y);
    pb.cubic_to(x + w - r + k, y, x + w, y + r - k, x + w, y + r);
    pb.line_to(x + w, y + h - r);
    pb.cubic_to(x + w, y + h - r + k, x + w - r + k, y + h, x + w - r, y + h);
    pb.line_to(x + r, y + h);
    pb.cubic_to(x + r - k, y + h, x, y + h - r + k, x, y + h - r);
    pb.line_to(x, y + r);
    pb.cubic_to(x, y + r - k, x + r - k, y, x + r, y);
    pb.close();
    pb.finish()
}

fn rect_path(x: f32, y: f32, w: f32, h: f32) -> Option<Path> {
    Rect::from_xywh(x, y, w.max(0.01), h.max(0.01)).map(PathBuilder::from_rect)
}

fn ellipse_path(cx: f32, cy: f32, rx: f32, ry: f32) -> Option<Path> {
    if rx <= 0.0 || ry <= 0.0 {
        return None;
    }
    let k = 0.5523;
    let (ox, oy) = (rx * k, ry * k);
    let mut pb = PathBuilder::new();
    pb.move_to(cx - rx, cy);
    pb.cubic_to(cx - rx, cy - oy, cx - ox, cy - ry, cx, cy - ry);
    pb.cubic_to(cx + ox, cy - ry, cx + rx, cy - oy, cx + rx, cy);
    pb.cubic_to(cx + rx, cy + oy, cx + ox, cy + ry, cx, cy + ry);
    pb.cubic_to(cx - ox, cy + ry, cx - rx, cy + oy, cx - rx, cy);
    pb.close();
    pb.finish()
}

fn circle_path(cx: f32, cy: f32, r: f32) -> Option<Path> {
    ellipse_path(cx, cy, r, r)
}

/// 把 premultiplied 的 Pixmap 以 src-over 合成到不透明的 u32 缓冲。
pub fn blend_pixmap(
    buf: &mut [u32],
    surface_w: u32,
    surface_h: u32,
    origin_x: i32,
    origin_y: i32,
    pixmap: &Pixmap,
) {
    let pw = pixmap.width() as i32;
    let ph = pixmap.height() as i32;
    let pixels = pixmap.pixels();
    for py in 0..ph {
        let dy = origin_y + py;
        if dy < 0 || dy >= surface_h as i32 {
            continue;
        }
        for px in 0..pw {
            let dx = origin_x + px;
            if dx < 0 || dx >= surface_w as i32 {
                continue;
            }
            let src = pixels[(py * pw + px) as usize];
            if src.alpha() == 0 {
                continue;
            }
            let index = (dy as u32 * surface_w + dx as u32) as usize;
            if let Some(slot) = buf.get_mut(index) {
                *slot = blend_over(src, *slot);
            }
        }
    }
}

fn blend_over(src: PremultipliedColorU8, dst: u32) -> u32 {
    let inv = 255 - u32::from(src.alpha());
    let dr = (dst >> 16) & 0xFF;
    let dg = (dst >> 8) & 0xFF;
    let db = dst & 0xFF;
    let r = u32::from(src.red()) + (dr * inv + 127) / 255;
    let g = u32::from(src.green()) + (dg * inv + 127) / 255;
    let b = u32::from(src.blue()) + (db * inv + 127) / 255;
    0xFF00_0000 | (r.min(255) << 16) | (g.min(255) << 8) | b.min(255)
}

fn pack(color: Rgba) -> u32 {
    let (r, g, b, _) = color;
    0xFF00_0000 | (u32::from(r) << 16) | (u32::from(g) << 8) | u32::from(b)
}

pub fn put_pixel(buf: &mut [u32], surface_w: u32, surface_h: u32, x: i32, y: i32, color: u32) {
    if x < 0 || y < 0 || x >= surface_w as i32 || y >= surface_h as i32 {
        return;
    }
    let index = (y as u32 * surface_w + x as u32) as usize;
    if let Some(slot) = buf.get_mut(index) {
        *slot = color;
    }
}

fn stroke_area(
    buf: &mut [u32],
    surface_w: u32,
    surface_h: u32,
    area: Area,
    thickness: f64,
    color: Rgba,
) {
    let packed = pack(color);
    let t = thickness.round().max(1.0) as i32;
    let x0 = area.x.round() as i32;
    let y0 = area.y.round() as i32;
    let x1 = area.right().round() as i32;
    let y1 = area.bottom().round() as i32;
    for x in x0..=x1 {
        for offset in 0..t {
            put_pixel(buf, surface_w, surface_h, x, y0 + offset, packed);
            put_pixel(buf, surface_w, surface_h, x, y1 - offset, packed);
        }
    }
    for y in y0..=y1 {
        for offset in 0..t {
            put_pixel(buf, surface_w, surface_h, x0 + offset, y, packed);
            put_pixel(buf, surface_w, surface_h, x1 - offset, y, packed);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hud() -> Hud {
        Hud::new(2.0)
    }

    fn layout() -> ToolbarLayout {
        ToolbarLayout::compute(
            Area { x: 400.0, y: 400.0, width: 900.0, height: 600.0 },
            2880.0,
            1800.0,
            hud(),
        )
    }

    #[test]
    fn toolbar_scales_with_device_pixel_ratio() {
        let selection = Area { x: 100.0, y: 100.0, width: 800.0, height: 400.0 };
        let one_x = ToolbarLayout::compute(selection, 2000.0, 1400.0, Hud::new(1.0));
        let two_x = ToolbarLayout::compute(selection, 2000.0, 1400.0, Hud::new(2.0));
        assert!(
            (two_x.bar.height - one_x.bar.height * 2.0).abs() < f64::EPSILON,
            "Retina 上 HUD 必须按 scale_factor 放大，否则物理像素绘制只有一半大小"
        );
        assert!(one_x.bar.height >= 44.0, "工具栏逻辑高度不能低于可点击阈值");
        assert!(one_x.bar.width >= 400.0, "12 个按钮 + 分隔线的逻辑宽度下限");
    }

    #[test]
    fn every_button_slot_is_hit_testable_at_its_center() {
        let layout = layout();
        let mut buttons = 0;
        for (slot, area) in &layout.slots {
            if let Slot::Button(action) = slot {
                buttons += 1;
                let hit = layout.hit(area.x + area.width / 2.0, area.y + area.height / 2.0);
                assert_eq!(hit, Some(*action), "按钮中心必须命中它自己");
            }
        }
        assert_eq!(buttons, 12, "工具栏按钮数量变化时同步更新交互与文档");
    }

    #[test]
    fn dividers_are_not_clickable() {
        let layout = layout();
        for (slot, area) in &layout.slots {
            if matches!(slot, Slot::Divider) {
                assert_eq!(layout.hit(area.x + area.width / 2.0, area.y + area.height / 2.0), None);
            }
        }
    }

    #[test]
    fn toolbar_stays_inside_surface_for_fullscreen_selection() {
        let layout = ToolbarLayout::compute(
            Area { x: 0.0, y: 0.0, width: 2880.0, height: 1800.0 },
            2880.0,
            1800.0,
            hud(),
        );
        assert!(layout.bar.x >= 0.0);
        assert!(layout.bar.y >= 0.0);
        assert!(layout.bar.right() <= 2880.0);
        assert!(layout.bar.bottom() <= 1800.0);
    }

    #[test]
    fn toolbar_flips_above_selection_when_bottom_has_no_room() {
        let selection = Area { x: 100.0, y: 900.0, width: 600.0, height: 860.0 };
        let layout = ToolbarLayout::compute(selection, 2880.0, 1800.0, hud());
        assert!(layout.bar.bottom() <= selection.y, "下方空间不足时工具栏应翻到选区上方");
    }

    #[test]
    fn blend_keeps_destination_opaque() {
        let mut buf = vec![0xFF00_0000u32; 4];
        let mut pixmap = Pixmap::new(2, 2).unwrap();
        fill(&mut pixmap, rect_path(0.0, 0.0, 2.0, 2.0), (255, 0, 0, 128));
        blend_pixmap(&mut buf, 2, 2, 0, 0, &pixmap);
        for pixel in buf {
            assert_eq!(pixel >> 24, 0xFF, "合成结果必须不透明，否则 softbuffer 会显示脏数据");
            assert!((pixel >> 16) & 0xFF > 100, "半透明红色应当混进目标像素");
        }
    }
}
