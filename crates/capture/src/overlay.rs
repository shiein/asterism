use std::num::NonZeroU32;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, TryRecvError};

use serde::{Deserialize, Serialize};
use tiny_skia::Pixmap;
use winit::application::ApplicationHandler;
use winit::dpi::{PhysicalPosition, PhysicalSize};
use winit::event::{ElementState, MouseButton, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{Key, NamedKey};
use winit::window::{Window, WindowId, WindowLevel};

use crate::annotation::{Annotation, AnnotationKind, AnnotationScene};
use crate::backend::{CaptureError, CapturedFrame, WindowInfo};
use crate::hud::{self, Area, Hud, ToolbarAction, ToolbarLayout, ToolbarState};

/// 遮罩亮度。太暗会让冻结帧看起来像"另一张图"，太亮又分不清选区。
const DIM_FACTOR: u32 = 140; // out of 255 ≈ 0.55

/// Native Fast Overlay：物理像素坐标选区
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Selection {
    /// 捕获帧的物理像素坐标
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum OverlayOutcome {
    Complete { selection: Selection, scene: AnnotationScene },
    Download { selection: Selection, scene: AnnotationScene },
    Pin { selection: Selection, scene: AnnotationScene },
    Scroll { selection: Selection },
    Cancel,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ActiveTool {
    None,
    Rectangle,
    Ellipse,
    Arrow,
    Brush,
    Mosaic,
    Text,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DragMode {
    None,
    Creating,
    Moving,
    Resizing(HandlePos),
    Drawing,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum HandlePos {
    TopLeft,
    TopCenter,
    TopRight,
    MidLeft,
    MidRight,
    BottomLeft,
    BottomCenter,
    BottomRight,
}

pub trait FastOverlay: Send {
    fn show_frozen(&mut self, frame: &CapturedFrame) -> Result<(), CaptureError>;
    fn take_outcome(&mut self) -> Option<OverlayOutcome>;
    fn dismiss(&mut self);
}

/// 会话编排
pub struct OverlaySession {
    pub frame: CapturedFrame,
    pub selection: Option<Selection>,
}

impl OverlaySession {
    pub fn crop_bgra(&self) -> Option<(u32, u32, Vec<u8>)> {
        let sel = self.selection.as_ref()?;
        let x = sel.x.max(0.0) as u32;
        let y = sel.y.max(0.0) as u32;
        let w = sel.width.max(1.0) as u32;
        let h = sel.height.max(1.0) as u32;
        if x + w > self.frame.width || y + h > self.frame.height {
            return None;
        }
        let mut out = Vec::with_capacity((w * h * 4) as usize);
        for row in y..y + h {
            let start = ((row * self.frame.width + x) * 4) as usize;
            let end = start + (w * 4) as usize;
            out.extend_from_slice(&self.frame.bgra[start..end]);
        }
        Some((w, h, out))
    }
}

/// 冻结帧来源。`Pending` 让调用方可以先把子进程/事件循环拉起来，
/// 再把捕获结果送进来——进程启动与屏幕捕获因此可以重叠，
/// 少掉一段"按下快捷键后什么都没发生"的空窗。
pub enum FrameSource {
    Ready(Box<CapturedFrame>),
    Pending(Receiver<Result<CapturedFrame, String>>),
}

/// 窗口列表来源。`list_windows` 在 macOS 上可能要几百毫秒，
/// 必须挪出"显示 overlay"的关键路径。
pub enum WindowSource {
    Ready(Vec<WindowInfo>),
    Pending(Receiver<Vec<WindowInfo>>),
}

pub fn select_region(frame: &CapturedFrame) -> Result<Option<Selection>, CaptureError> {
    match select_region_with_windows(frame, &[])? {
        Some(OverlayOutcome::Complete { selection, .. })
        | Some(OverlayOutcome::Download { selection, .. })
        | Some(OverlayOutcome::Pin { selection, .. })
        | Some(OverlayOutcome::Scroll { selection }) => Ok(Some(selection)),
        _ => Ok(None),
    }
}

pub fn select_region_with_windows(
    frame: &CapturedFrame,
    windows: &[WindowInfo],
) -> Result<Option<OverlayOutcome>, CaptureError> {
    run_overlay(FrameSource::Ready(Box::new(frame.clone())), WindowSource::Ready(windows.to_vec()))
}

pub fn run_overlay(
    frame: FrameSource,
    windows: WindowSource,
) -> Result<Option<OverlayOutcome>, CaptureError> {
    let event_loop = EventLoop::new().map_err(|e| CaptureError::Failed(e.to_string()))?;
    let waiting_for_frame = matches!(frame, FrameSource::Pending(_));
    event_loop.set_control_flow(if waiting_for_frame {
        ControlFlow::Poll
    } else {
        ControlFlow::Wait
    });
    let mut app = OverlayApp::new(frame, windows);
    event_loop.run_app(&mut app).map_err(|e| CaptureError::Failed(e.to_string()))?;
    if let Some(err) = app.fail {
        return Err(CaptureError::Failed(err));
    }
    Ok(app.outcome)
}

struct OverlayApp {
    frame: Option<CapturedFrame>,
    frame_rx: Option<Receiver<Result<CapturedFrame, String>>>,
    windows: Vec<WindowInfo>,
    windows_rx: Option<Receiver<Vec<WindowInfo>>>,

    window: Option<Arc<Window>>,
    surface: Option<softbuffer::Surface<Arc<Window>, Arc<Window>>>,
    shown: bool,
    hud: Hud,

    /// 预合成的两层画面，重绘时只做 memcpy，不再逐像素重算整屏。
    layers: Option<Layers>,

    selection_rect: Option<Area>,
    hover_window_rect: Option<Area>,
    toolbar: Option<ToolbarLayout>,

    drag_mode: DragMode,
    drag_start: Option<(f64, f64)>,
    current_mouse: Option<(f64, f64)>,
    last_click_time: Option<std::time::Instant>,

    active_tool: ActiveTool,
    annotations: Vec<Annotation>,
    current_stroke: Vec<f64>,
    text_draft: Option<TextDraft>,

    outcome: Option<OverlayOutcome>,
    fail: Option<String>,
}

struct TextDraft {
    x: f64,
    y: f64,
    text: String,
}

struct Layers {
    width: u32,
    height: u32,
    bright: Vec<u32>,
    dimmed: Vec<u32>,
}

impl Layers {
    fn build(frame: &CapturedFrame, width: u32, height: u32) -> Self {
        let count = (width as usize) * (height as usize);
        let mut bright = vec![0u32; count];
        let mut dimmed = vec![0u32; count];
        let same_size = frame.width == width && frame.height == height;
        for y in 0..height {
            let source_y =
                if same_size { y } else { (y as u64 * frame.height as u64 / height as u64) as u32 };
            for x in 0..width {
                let source_x = if same_size {
                    x
                } else {
                    (x as u64 * frame.width as u64 / width as u64) as u32
                };
                let i = ((source_y * frame.width + source_x) * 4) as usize;
                let (b, g, r) = match frame.bgra.get(i..i + 3) {
                    Some(px) => (u32::from(px[0]), u32::from(px[1]), u32::from(px[2])),
                    None => (0, 0, 0),
                };
                let index = (y * width + x) as usize;
                bright[index] = 0xFF00_0000 | (r << 16) | (g << 8) | b;
                dimmed[index] = 0xFF00_0000
                    | ((r * DIM_FACTOR / 255) << 16)
                    | ((g * DIM_FACTOR / 255) << 8)
                    | (b * DIM_FACTOR / 255);
            }
        }
        Self { width, height, bright, dimmed }
    }
}

impl OverlayApp {
    fn new(frame: FrameSource, windows: WindowSource) -> Self {
        let (frame, frame_rx) = match frame {
            FrameSource::Ready(frame) => (Some(*frame), None),
            FrameSource::Pending(rx) => (None, Some(rx)),
        };
        let (windows, windows_rx) = match windows {
            WindowSource::Ready(list) => (list, None),
            WindowSource::Pending(rx) => (Vec::new(), Some(rx)),
        };
        Self {
            frame,
            frame_rx,
            windows,
            windows_rx,
            window: None,
            surface: None,
            shown: false,
            hud: Hud::new(1.0),
            layers: None,
            selection_rect: None,
            hover_window_rect: None,
            toolbar: None,
            drag_mode: DragMode::None,
            drag_start: None,
            current_mouse: None,
            last_click_time: None,
            active_tool: ActiveTool::None,
            annotations: Vec::new(),
            current_stroke: Vec::new(),
            text_draft: None,
            outcome: None,
            fail: None,
        }
    }

    fn fail_and_exit(&mut self, event_loop: &ActiveEventLoop, err: String) {
        self.fail = Some(err);
        self.outcome = None;
        event_loop.exit();
    }

    /// 创建冻结窗口。窗口先隐藏、画完首帧再显示，
    /// 避免用户看到"空白/上一帧窗口先出现，然后才盖上截图"的两段式过程。
    fn create_window(&mut self, event_loop: &ActiveEventLoop) {
        let Some(frame) = self.frame.as_ref() else { return };
        let target = overlay_monitor(event_loop, frame.monitor.origin_physical);
        let (pos, size) = match target.as_ref() {
            Some(monitor) => (monitor.position(), monitor.size()),
            None => (
                PhysicalPosition::new(
                    frame.monitor.origin_physical.0,
                    frame.monitor.origin_physical.1,
                ),
                PhysicalSize::new(frame.width.max(1), frame.height.max(1)),
            ),
        };
        #[allow(unused_mut)]
        let mut attrs = Window::default_attributes()
            .with_title("Asterism")
            .with_decorations(false)
            .with_resizable(false)
            .with_transparent(false)
            .with_visible(false)
            .with_window_level(WindowLevel::AlwaysOnTop)
            .with_position(pos)
            .with_inner_size(size);
        #[cfg(target_os = "macos")]
        {
            use winit::platform::macos::WindowAttributesExtMacOS;
            attrs = attrs
                .with_has_shadow(false)
                .with_accepts_first_mouse(true)
                .with_titlebar_hidden(true)
                .with_movable_by_window_background(false);
        }
        let window = match event_loop.create_window(attrs) {
            Ok(window) => Arc::new(window),
            Err(err) => {
                self.fail_and_exit(event_loop, err.to_string());
                return;
            }
        };
        #[cfg(target_os = "macos")]
        {
            use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
            if let Ok(handle) = window.window_handle()
                && let RawWindowHandle::AppKit(appkit) = handle.as_raw()
            {
                crate::macos_perm::elevate_overlay_ns_view(appkit.ns_view.as_ptr());
            }
        }
        let context = match softbuffer::Context::new(window.clone()) {
            Ok(context) => context,
            Err(err) => {
                self.fail_and_exit(event_loop, err.to_string());
                return;
            }
        };
        let surface = match softbuffer::Surface::new(&context, window.clone()) {
            Ok(surface) => surface,
            Err(err) => {
                self.fail_and_exit(event_loop, err.to_string());
                return;
            }
        };
        self.hud = Hud::new(window.scale_factor());
        self.window = Some(window);
        self.surface = Some(surface);
        event_loop.set_control_flow(ControlFlow::Wait);
        self.redraw();
        self.reveal();
    }

    fn reveal(&mut self) {
        if self.shown {
            return;
        }
        if let Some(window) = self.window.clone() {
            window.set_visible(true);
            window.focus_window();
            self.shown = true;
            // 显示后再提交一帧，规避某些平台上"隐藏窗口的首帧被丢弃"。
            self.redraw();
        }
    }

    fn surface_size(&self) -> (u32, u32) {
        self.window
            .as_ref()
            .map(|window| {
                let size = window.inner_size();
                (size.width, size.height)
            })
            .unwrap_or((0, 0))
    }

    /// 帧像素 / 窗口像素。正常情况是 1.0，只有捕获分辨率与窗口不一致时才生效。
    fn frame_scale(&self) -> (f64, f64) {
        let (w, h) = self.surface_size();
        match (self.frame.as_ref(), w, h) {
            (Some(frame), w, h) if w > 0 && h > 0 => {
                (f64::from(frame.width) / f64::from(w), f64::from(frame.height) / f64::from(h))
            }
            _ => (1.0, 1.0),
        }
    }

    fn translate_annotations_to_selection(&self) -> Vec<Annotation> {
        let mut translated = self.annotations.clone();
        let (Some(area), (sx, sy)) = (self.selection_rect, self.frame_scale()) else {
            return translated;
        };
        for ann in &mut translated {
            match ann.kind {
                AnnotationKind::Rectangle | AnnotationKind::Ellipse => {
                    if ann.geometry.len() >= 4 {
                        ann.geometry[0] = (ann.geometry[0] - area.x) * sx;
                        ann.geometry[1] = (ann.geometry[1] - area.y) * sy;
                        ann.geometry[2] *= sx;
                        ann.geometry[3] *= sy;
                    }
                }
                AnnotationKind::Arrow
                | AnnotationKind::Brush
                | AnnotationKind::Mosaic
                | AnnotationKind::Text => {
                    for chunk in ann.geometry.chunks_exact_mut(2) {
                        chunk[0] = (chunk[0] - area.x) * sx;
                        chunk[1] = (chunk[1] - area.y) * sy;
                    }
                }
                _ => {}
            }
        }
        translated
    }

    fn commit_outcome_and_exit(
        &mut self,
        event_loop: &ActiveEventLoop,
        mut outcome: OverlayOutcome,
    ) {
        self.commit_text_draft();
        let translated = self.translate_annotations_to_selection();
        match &mut outcome {
            OverlayOutcome::Complete { scene, .. }
            | OverlayOutcome::Download { scene, .. }
            | OverlayOutcome::Pin { scene, .. } => {
                scene.items = translated;
            }
            _ => {}
        }
        self.outcome = Some(outcome);
        event_loop.exit();
    }

    fn complete(&mut self, event_loop: &ActiveEventLoop) {
        self.commit_text_draft();
        if let Some(selection) = self.current_selection_in_frame() {
            self.commit_outcome_and_exit(
                event_loop,
                OverlayOutcome::Complete {
                    selection,
                    scene: AnnotationScene { items: self.annotations.clone() },
                },
            );
        }
    }

    fn cancel_and_exit(&mut self, event_loop: &ActiveEventLoop) {
        self.outcome = Some(OverlayOutcome::Cancel);
        event_loop.exit();
    }

    /// Esc / 右键的统一退避顺序：文字草稿 → 工具 → 选区 → 退出。
    fn step_back(&mut self, event_loop: &ActiveEventLoop) {
        if self.text_draft.is_some() {
            self.text_draft = None;
        } else if self.active_tool != ActiveTool::None {
            self.active_tool = ActiveTool::None;
        } else if self.selection_rect.is_some() {
            self.selection_rect = None;
            self.toolbar = None;
            self.annotations.clear();
        } else {
            self.cancel_and_exit(event_loop);
            return;
        }
        self.redraw();
    }

    fn current_selection_in_frame(&self) -> Option<Selection> {
        let area = self.selection_rect?;
        let frame = self.frame.as_ref()?;
        let (sx, sy) = self.frame_scale();
        let fx = (area.x * sx).floor().clamp(0.0, frame.width.saturating_sub(1) as f64);
        let fy = (area.y * sy).floor().clamp(0.0, frame.height.saturating_sub(1) as f64);
        let fw = (area.width * sx).round().clamp(1.0, (f64::from(frame.width) - fx).max(1.0));
        let fh = (area.height * sy).round().clamp(1.0, (f64::from(frame.height) - fy).max(1.0));
        Some(Selection { x: fx, y: fy, width: fw, height: fh })
    }

    fn find_window_under_cursor(&self, px: f64, py: f64) -> Option<Area> {
        let frame = self.frame.as_ref()?;
        let (win_w, win_h) = self.surface_size();
        if win_w == 0 || win_h == 0 {
            return None;
        }
        let origin_x = f64::from(frame.monitor.origin_physical.0);
        let origin_y = f64::from(frame.monitor.origin_physical.1);
        let (fx, fy) = self.frame_scale();
        let phys_x = origin_x + px * fx;
        let phys_y = origin_y + py * fy;

        #[cfg(target_os = "macos")]
        let scale = frame.monitor.scale_factor.max(1.0);
        #[cfg(not(target_os = "macos"))]
        let scale = 1.0;

        let mut best: Option<Area> = None;
        for win in self.windows.iter() {
            let wx = f64::from(win.position.0) * scale;
            let wy = f64::from(win.position.1) * scale;
            let ww = f64::from(win.size.0) * scale;
            let wh = f64::from(win.size.1) * scale;
            if ww <= 30.0
                || wh <= 30.0
                || phys_x < wx
                || phys_x > wx + ww
                || phys_y < wy
                || phys_y > wy + wh
            {
                continue;
            }
            let sx = f64::from(win_w) / f64::from(frame.width);
            let sy = f64::from(win_h) / f64::from(frame.height);
            let lx = ((wx - origin_x) * sx).clamp(0.0, f64::from(win_w));
            let ly = ((wy - origin_y) * sy).clamp(0.0, f64::from(win_h));
            let lw = (ww * sx).clamp(0.0, f64::from(win_w) - lx);
            let lh = (wh * sy).clamp(0.0, f64::from(win_h) - ly);
            if lw >= 20.0 && lh >= 20.0 {
                best = Some(Area { x: lx, y: ly, width: lw, height: lh });
                break;
            }
        }
        best
    }

    fn clamp_to_selection(&self, x: f64, y: f64) -> (f64, f64) {
        match self.selection_rect {
            Some(area) => (x.clamp(area.x, area.right()), y.clamp(area.y, area.bottom())),
            None => (x, y),
        }
    }

    fn mosaic_block(&self) -> u64 {
        let (sx, _) = self.frame_scale();
        ((10.0 * self.hud.px(1.0) * sx).round() as u64).clamp(8, 64)
    }

    fn commit_text_draft(&mut self) {
        let Some(draft) = self.text_draft.take() else { return };
        if draft.text.is_empty() {
            return;
        }
        let index = self.annotations.len();
        self.annotations.push(Annotation {
            id: format!("ann_{index}"),
            kind: AnnotationKind::Text,
            geometry: vec![draft.x, draft.y],
            style: serde_json::json!({
                "text": draft.text,
                "color_r": 255, "color_g": 59, "color_b": 48,
                "font_scale": self.hud.text_scale(),
            }),
            z_index: index as i32,
        });
    }

    fn undo(&mut self) {
        if self.text_draft.is_some() {
            self.text_draft = None;
        } else {
            self.annotations.pop();
        }
        self.redraw();
    }

    fn set_tool(&mut self, tool: ActiveTool) {
        self.commit_text_draft();
        self.active_tool = if self.active_tool == tool { ActiveTool::None } else { tool };
        self.redraw();
    }

    fn handle_toolbar(&mut self, event_loop: &ActiveEventLoop, action: ToolbarAction) {
        match action {
            ToolbarAction::SetTool(tool) => self.set_tool(tool),
            ToolbarAction::Undo => self.undo(),
            ToolbarAction::Scroll => {
                if let Some(selection) = self.current_selection_in_frame() {
                    self.commit_outcome_and_exit(event_loop, OverlayOutcome::Scroll { selection });
                }
            }
            ToolbarAction::Save => {
                self.commit_text_draft();
                if let Some(selection) = self.current_selection_in_frame() {
                    self.commit_outcome_and_exit(
                        event_loop,
                        OverlayOutcome::Download {
                            selection,
                            scene: AnnotationScene { items: self.annotations.clone() },
                        },
                    );
                }
            }
            ToolbarAction::Pin => {
                self.commit_text_draft();
                if let Some(selection) = self.current_selection_in_frame() {
                    self.commit_outcome_and_exit(
                        event_loop,
                        OverlayOutcome::Pin {
                            selection,
                            scene: AnnotationScene { items: self.annotations.clone() },
                        },
                    );
                }
            }
            ToolbarAction::Cancel => self.cancel_and_exit(event_loop),
            ToolbarAction::Done => self.complete(event_loop),
        }
    }

    fn on_key(&mut self, event_loop: &ActiveEventLoop, key: &Key, text: Option<&str>) {
        // 文字草稿优先吃掉所有按键，否则输入 "r" 会切成矩形工具。
        if self.text_draft.is_some() {
            match key {
                Key::Named(NamedKey::Escape) => {
                    self.text_draft = None;
                    self.redraw();
                }
                Key::Named(NamedKey::Enter) => {
                    self.commit_text_draft();
                    self.redraw();
                }
                Key::Named(NamedKey::Backspace) => {
                    if let Some(draft) = self.text_draft.as_mut() {
                        draft.text.pop();
                    }
                    self.redraw();
                }
                Key::Named(NamedKey::Space) => {
                    if let Some(draft) = self.text_draft.as_mut() {
                        draft.text.push(' ');
                    }
                    self.redraw();
                }
                _ => {
                    // 位图字体只有 ASCII。直接丢弃其他输入，
                    // 不让 overlay 里显示的内容和导出结果不一致。
                    if let Some(text) = text {
                        let printable: String =
                            text.chars().filter(|c| c.is_ascii_graphic() || *c == ' ').collect();
                        if !printable.is_empty()
                            && let Some(draft) = self.text_draft.as_mut()
                        {
                            draft.text.push_str(&printable);
                            self.redraw();
                        }
                    }
                }
            }
            return;
        }

        match key {
            Key::Named(NamedKey::Escape) => self.step_back(event_loop),
            Key::Named(NamedKey::Enter) => self.complete(event_loop),
            Key::Character(ch) => match ch.as_str().to_ascii_lowercase().as_str() {
                "r" => self.set_tool(ActiveTool::Rectangle),
                "e" => self.set_tool(ActiveTool::Ellipse),
                "a" => self.set_tool(ActiveTool::Arrow),
                "p" => self.set_tool(ActiveTool::Brush),
                "m" => self.set_tool(ActiveTool::Mosaic),
                "t" => self.set_tool(ActiveTool::Text),
                "z" => self.undo(),
                "s" => {
                    if let Some(selection) = self.current_selection_in_frame() {
                        self.commit_outcome_and_exit(
                            event_loop,
                            OverlayOutcome::Scroll { selection },
                        );
                    }
                }
                _ => {}
            },
            _ => {}
        }
    }

    fn on_press(&mut self, event_loop: &ActiveEventLoop) {
        let Some((mx, my)) = self.current_mouse else { return };
        let now = std::time::Instant::now();
        let double_click =
            self.last_click_time.is_some_and(|last| now.duration_since(last).as_millis() < 300);
        self.last_click_time = Some(now);
        self.drag_start = Some((mx, my));

        if let Some(area) = self.selection_rect {
            if let Some(action) = self.toolbar.as_ref().and_then(|bar| bar.hit(mx, my)) {
                self.handle_toolbar(event_loop, action);
                return;
            }
            // 点在工具栏空白处不应该穿透成"画一笔"。
            if self.toolbar.as_ref().is_some_and(|bar| bar.contains(mx, my)) {
                return;
            }

            if double_click && self.active_tool == ActiveTool::None && area.contains(mx, my) {
                self.complete(event_loop);
                return;
            }

            if self.active_tool == ActiveTool::Text {
                self.commit_text_draft();
                if area.contains(mx, my) {
                    let (x, y) = self.clamp_to_selection(mx, my);
                    self.text_draft = Some(TextDraft { x, y, text: String::new() });
                    self.redraw();
                }
                return;
            }

            if self.active_tool != ActiveTool::None {
                let (x, y) = self.clamp_to_selection(mx, my);
                self.drag_mode = DragMode::Drawing;
                self.current_stroke = vec![x, y];
                return;
            }

            if let Some(handle) = self.hit_handle(mx, my, area) {
                self.drag_mode = DragMode::Resizing(handle);
                return;
            }

            if area.contains(mx, my) {
                self.drag_mode = DragMode::Moving;
                return;
            }
        }

        if let Some(hover) = self.hover_window_rect {
            self.selection_rect = Some(hover);
        }
        self.annotations.clear();
        self.text_draft = None;
        self.hover_window_rect = None;
        self.drag_mode = DragMode::Creating;
    }

    fn on_release(&mut self) {
        match self.drag_mode {
            DragMode::Creating => {
                if let (Some(a), Some(b)) = (self.drag_start, self.current_mouse) {
                    let dx = (a.0 - b.0).abs();
                    let dy = (a.1 - b.1).abs();
                    if dx > 8.0 && dy > 8.0 {
                        self.selection_rect =
                            Some(Area { x: a.0.min(b.0), y: a.1.min(b.1), width: dx, height: dy });
                    } else {
                        // 点一下没拖动：优先吸附窗口，否则回到"没有选区"，
                        // 而不是留下一个几像素大的废选区。
                        self.selection_rect = self
                            .hover_window_rect
                            .or_else(|| self.find_window_under_cursor(b.0, b.1));
                    }
                }
            }
            DragMode::Drawing => {
                if let (Some(a), Some(b)) = (self.drag_start, self.current_mouse) {
                    let start = self.clamp_to_selection(a.0, a.1);
                    let end = self.clamp_to_selection(b.0, b.1);
                    if let Some(ann) = self.create_annotation(start, end, self.annotations.len()) {
                        self.annotations.push(ann);
                    }
                    self.current_stroke.clear();
                }
            }
            _ => {}
        }
        self.drag_mode = DragMode::None;
        self.drag_start = None;
        self.redraw();
    }

    fn hit_handle(&self, mx: f64, my: f64, area: Area) -> Option<HandlePos> {
        let tolerance = self.hud.px(11.0);
        let order = [
            HandlePos::TopLeft,
            HandlePos::TopCenter,
            HandlePos::TopRight,
            HandlePos::MidLeft,
            HandlePos::MidRight,
            HandlePos::BottomLeft,
            HandlePos::BottomCenter,
            HandlePos::BottomRight,
        ];
        hud::handle_points(area)
            .iter()
            .zip(order)
            .find(|((hx, hy), _)| (mx - hx).abs() <= tolerance && (my - hy).abs() <= tolerance)
            .map(|(_, pos)| pos)
    }

    fn create_annotation(
        &self,
        start: (f64, f64),
        end: (f64, f64),
        index: usize,
    ) -> Option<Annotation> {
        let id = format!("ann_{index}");
        let stroke_width = self.hud.px(3.0);
        let color = serde_json::json!({"color_r": 255, "color_g": 59, "color_b": 48});
        let style = |extra: serde_json::Value| {
            let mut merged = color.clone();
            if let (Some(base), Some(extra)) = (merged.as_object_mut(), extra.as_object()) {
                for (key, value) in extra {
                    base.insert(key.clone(), value.clone());
                }
            }
            merged
        };
        match self.active_tool {
            ActiveTool::Rectangle | ActiveTool::Ellipse => {
                let x = start.0.min(end.0);
                let y = start.1.min(end.1);
                let w = (start.0 - end.0).abs();
                let h = (start.1 - end.1).abs();
                if w < 2.0 || h < 2.0 {
                    return None;
                }
                Some(Annotation {
                    id,
                    kind: if self.active_tool == ActiveTool::Rectangle {
                        AnnotationKind::Rectangle
                    } else {
                        AnnotationKind::Ellipse
                    },
                    geometry: vec![x, y, w, h],
                    style: style(serde_json::json!({"stroke_width": stroke_width})),
                    z_index: index as i32,
                })
            }
            ActiveTool::Arrow => {
                if (start.0 - end.0).abs() < 3.0 && (start.1 - end.1).abs() < 3.0 {
                    return None;
                }
                Some(Annotation {
                    id,
                    kind: AnnotationKind::Arrow,
                    geometry: vec![start.0, start.1, end.0, end.1],
                    style: style(serde_json::json!({"stroke_width": stroke_width})),
                    z_index: index as i32,
                })
            }
            ActiveTool::Brush => {
                if self.current_stroke.len() < 4 {
                    return None;
                }
                Some(Annotation {
                    id,
                    kind: AnnotationKind::Brush,
                    geometry: self.current_stroke.clone(),
                    style: style(serde_json::json!({"stroke_width": stroke_width})),
                    z_index: index as i32,
                })
            }
            ActiveTool::Mosaic => {
                if self.current_stroke.len() < 2 {
                    return None;
                }
                let block = self.mosaic_block();
                Some(Annotation {
                    id,
                    kind: AnnotationKind::Mosaic,
                    geometry: self.current_stroke.clone(),
                    style: serde_json::json!({
                        "brush_radius": (block * 3 / 2) as f64,
                        "block_size": block,
                    }),
                    z_index: index as i32,
                })
            }
            ActiveTool::Text | ActiveTool::None => None,
        }
    }
}

impl ApplicationHandler for OverlayApp {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_none() && self.frame.is_some() {
            self.create_window(event_loop);
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        if let Some(rx) = self.windows_rx.as_ref() {
            match rx.try_recv() {
                Ok(list) => {
                    self.windows = list;
                    self.windows_rx = None;
                }
                Err(TryRecvError::Disconnected) => self.windows_rx = None,
                Err(TryRecvError::Empty) => {}
            }
        }
        if self.window.is_some() || self.frame.is_some() {
            if self.window.is_none() {
                self.create_window(event_loop);
            }
            return;
        }
        let Some(rx) = self.frame_rx.as_ref() else {
            self.fail_and_exit(event_loop, "overlay frame source closed".into());
            return;
        };
        match rx.try_recv() {
            Ok(Ok(frame)) => {
                self.frame = Some(frame);
                self.frame_rx = None;
                self.create_window(event_loop);
            }
            Ok(Err(err)) => {
                self.frame_rx = None;
                self.fail_and_exit(event_loop, err);
            }
            Err(TryRecvError::Disconnected) => {
                self.frame_rx = None;
                self.fail_and_exit(event_loop, "overlay frame producer disconnected".into());
            }
            Err(TryRecvError::Empty) => {
                std::thread::sleep(std::time::Duration::from_millis(2));
            }
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => self.cancel_and_exit(event_loop),
            WindowEvent::KeyboardInput { event, .. } if event.state == ElementState::Pressed => {
                let key = event.logical_key.clone();
                let text = event.text.as_ref().map(|text| text.to_string());
                self.on_key(event_loop, &key, text.as_deref());
            }
            WindowEvent::MouseInput { state, button, .. } => match (button, state) {
                (MouseButton::Right, ElementState::Pressed) => self.step_back(event_loop),
                (MouseButton::Left, ElementState::Pressed) => self.on_press(event_loop),
                (MouseButton::Left, ElementState::Released) => self.on_release(),
                _ => {}
            },
            WindowEvent::CursorMoved { position, .. } => {
                let (px, py) = (position.x, position.y);
                self.current_mouse = Some((px, py));
                match self.drag_mode {
                    DragMode::Creating => {
                        if let Some(start) = self.drag_start {
                            self.selection_rect = Some(Area {
                                x: start.0.min(px),
                                y: start.1.min(py),
                                width: (start.0 - px).abs(),
                                height: (start.1 - py).abs(),
                            });
                        }
                    }
                    DragMode::Moving => {
                        if let (Some(start), Some(area)) = (self.drag_start, self.selection_rect) {
                            let (sw, sh) = self.surface_size();
                            let dx = px - start.0;
                            let dy = py - start.1;
                            let x = (area.x + dx).clamp(0.0, (f64::from(sw) - area.width).max(0.0));
                            let y =
                                (area.y + dy).clamp(0.0, (f64::from(sh) - area.height).max(0.0));
                            self.selection_rect =
                                Some(Area { x, y, width: area.width, height: area.height });
                            self.drag_start = Some((px, py));
                        }
                    }
                    DragMode::Resizing(handle) => {
                        if let Some(area) = self.selection_rect {
                            self.selection_rect = Some(resize_area(area, handle, px, py));
                        }
                    }
                    DragMode::Drawing => {
                        let (x, y) = self.clamp_to_selection(px, py);
                        self.current_stroke.push(x);
                        self.current_stroke.push(y);
                    }
                    DragMode::None => {
                        if self.selection_rect.is_none() {
                            self.hover_window_rect = self.find_window_under_cursor(px, py);
                        }
                    }
                }
                self.redraw();
            }
            WindowEvent::RedrawRequested => self.redraw(),
            _ => {}
        }
    }
}

fn resize_area(area: Area, handle: HandlePos, mx: f64, my: f64) -> Area {
    let min = 12.0;
    let (left, top, right, bottom) = (area.x, area.y, area.right(), area.bottom());
    let (nx, ny, nr, nb) = match handle {
        HandlePos::TopLeft => (mx.min(right - min), my.min(bottom - min), right, bottom),
        HandlePos::TopCenter => (left, my.min(bottom - min), right, bottom),
        HandlePos::TopRight => (left, my.min(bottom - min), mx.max(left + min), bottom),
        HandlePos::MidLeft => (mx.min(right - min), top, right, bottom),
        HandlePos::MidRight => (left, top, mx.max(left + min), bottom),
        HandlePos::BottomLeft => (mx.min(right - min), top, right, my.max(top + min)),
        HandlePos::BottomCenter => (left, top, right, my.max(top + min)),
        HandlePos::BottomRight => (left, top, mx.max(left + min), my.max(top + min)),
    };
    Area { x: nx, y: ny, width: nr - nx, height: nb - ny }
}

impl OverlayApp {
    fn redraw(&mut self) {
        let Some(window) = self.window.clone() else { return };
        let size = window.inner_size();
        let Some(nz_w) = NonZeroU32::new(size.width) else { return };
        let Some(nz_h) = NonZeroU32::new(size.height) else { return };
        let (dw, dh) = (size.width, size.height);

        if self.layers.as_ref().is_none_or(|layers| layers.width != dw || layers.height != dh) {
            let Some(frame) = self.frame.as_ref() else { return };
            self.layers = Some(Layers::build(frame, dw, dh));
        }

        let hud = self.hud;
        let selection = self.selection_rect;
        let hover = self.hover_window_rect;
        let active_tool = self.active_tool;
        let can_undo = !self.annotations.is_empty() || self.text_draft.is_some();
        let hovered_action = self
            .current_mouse
            .zip(self.toolbar.as_ref())
            .and_then(|((mx, my), bar)| bar.hit(mx, my));

        // 预览时把"正在画的一笔"当成临时标注，和最终导出走同一套绘制。
        let live = if self.drag_mode == DragMode::Drawing {
            match (self.drag_start, self.current_mouse) {
                (Some(a), Some(b)) => {
                    let start = self.clamp_to_selection(a.0, a.1);
                    let end = self.clamp_to_selection(b.0, b.1);
                    self.create_annotation(start, end, usize::MAX)
                }
                _ => None,
            }
        } else {
            None
        };
        let draft_preview = self.text_draft.as_ref().map(|draft| Annotation {
            id: "draft".into(),
            kind: AnnotationKind::Text,
            geometry: vec![draft.x, draft.y],
            style: serde_json::json!({
                "text": format!("{}_", draft.text),
                "color_r": 255, "color_g": 59, "color_b": 48,
                "font_scale": hud.text_scale(),
            }),
            z_index: i32::MAX,
        });

        let annotations = self.annotations.clone();
        let frame_scale = self.frame_scale();
        let magnifier_source = self.frame.clone();

        let Some(surface) = self.surface.as_mut() else { return };
        if surface.resize(nz_w, nz_h).is_err() {
            return;
        }
        let Ok(mut buf) = surface.buffer_mut() else { return };
        let Some(layers) = self.layers.as_ref() else { return };

        // 1. 背景：整屏 memcpy 变暗层，再把选区行拷回原亮度。
        let cutout = selection.or(hover);
        if buf.len() == layers.dimmed.len() {
            buf.copy_from_slice(&layers.dimmed);
            if let Some(area) = cutout {
                let x0 = area.x.max(0.0).floor() as u32;
                let y0 = area.y.max(0.0).floor() as u32;
                let x1 = area.right().min(f64::from(dw)).ceil() as u32;
                let y1 = area.bottom().min(f64::from(dh)).ceil() as u32;
                for y in y0..y1.min(dh) {
                    let start = (y * dw + x0) as usize;
                    let end = (y * dw + x1.min(dw)) as usize;
                    if start < end && end <= buf.len() {
                        buf[start..end].copy_from_slice(&layers.bright[start..end]);
                    }
                }
            }
        }

        // 2. 马赛克直接在合成结果上按格子做块平均，和导出用同一份 mask。
        for ann in annotations.iter().chain(live.iter()) {
            if matches!(ann.kind, AnnotationKind::Mosaic)
                && let Some(mask) = crate::annotation::mosaic_mask(ann)
            {
                apply_buffer_mosaic(&mut buf, dw, dh, &mask);
            }
        }

        // 3. 其余标注交给 tiny-skia，抗锯齿且与导出完全一致。
        for ann in annotations.iter().chain(live.iter()).chain(draft_preview.iter()) {
            if !matches!(ann.kind, AnnotationKind::Mosaic) {
                render_annotation(&mut buf, dw, dh, ann);
            }
        }

        // 4. 选区装饰。
        if let Some(area) = selection {
            hud::draw_selection_chrome(&mut buf, dw, dh, area, hud);
            hud::draw_size_tag(&mut buf, dw, dh, area, frame_scale, hud);
        } else if let Some(area) = hover {
            hud::draw_snap_hint(&mut buf, dw, dh, area, hud);
        }

        // 5. 放大镜只在还没定下选区时出现，定好后不再遮挡内容。
        if selection.is_none()
            && let (Some((mx, my)), Some(frame)) = (self.current_mouse, magnifier_source.as_ref())
        {
            let fx = (mx * frame_scale.0).round() as i32;
            let fy = (my * frame_scale.1).round() as i32;
            hud::draw_magnifier(
                &mut buf,
                dw,
                dh,
                |x, y| sample_frame(frame, x, y),
                (mx, my),
                (fx, fy),
                hud,
            );
        }

        let toolbar =
            selection.map(|area| ToolbarLayout::compute(area, f64::from(dw), f64::from(dh), hud));
        if let Some(layout) = toolbar.as_ref() {
            hud::draw_toolbar(
                &mut buf,
                dw,
                dh,
                layout,
                ToolbarState { active_tool, hovered: hovered_action, can_undo, can_scroll: true },
            );
        }

        let _ = buf.present();
        self.toolbar = toolbar;
    }
}

fn sample_frame(frame: &CapturedFrame, x: i32, y: i32) -> (u8, u8, u8) {
    if x < 0 || y < 0 || x >= frame.width as i32 || y >= frame.height as i32 {
        return (0, 0, 0);
    }
    let index = (y as usize * frame.width as usize + x as usize) * 4;
    match frame.bgra.get(index..index + 3) {
        Some(px) => (px[2], px[1], px[0]),
        None => (0, 0, 0),
    }
}

/// 与导出完全一致的块平均马赛克，只是目标换成 u32 缓冲。
fn apply_buffer_mosaic(buf: &mut [u32], dw: u32, dh: u32, mask: &crate::MosaicMask) {
    let block = mask.block.max(1) as i32;
    for &(cell_x, cell_y) in &mask.cells {
        let x0 = (cell_x * block).clamp(0, dw as i32);
        let y0 = (cell_y * block).clamp(0, dh as i32);
        let x1 = (cell_x * block + block).clamp(0, dw as i32);
        let y1 = (cell_y * block + block).clamp(0, dh as i32);
        if x0 >= x1 || y0 >= y1 {
            continue;
        }
        let mut sum = [0u64; 3];
        let mut count = 0u64;
        for y in y0..y1 {
            for x in x0..x1 {
                let pixel = buf[(y as u32 * dw + x as u32) as usize];
                sum[0] += u64::from((pixel >> 16) & 0xFF);
                sum[1] += u64::from((pixel >> 8) & 0xFF);
                sum[2] += u64::from(pixel & 0xFF);
                count += 1;
            }
        }
        if count == 0 {
            continue;
        }
        let packed = 0xFF00_0000
            | ((sum[0] / count) as u32) << 16
            | ((sum[1] / count) as u32) << 8
            | (sum[2] / count) as u32;
        for y in y0..y1 {
            for x in x0..x1 {
                buf[(y as u32 * dw + x as u32) as usize] = packed;
            }
        }
    }
}

/// 把单个标注画到自己的包围盒 Pixmap 里再合成，避免每帧分配整屏 Pixmap。
fn render_annotation(buf: &mut [u32], dw: u32, dh: u32, ann: &Annotation) {
    let Some((ox, oy, w, h)) = annotation_bounds(ann, dw, dh) else { return };
    let Some(mut pixmap) = Pixmap::new(w, h) else { return };
    let mut local = ann.clone();
    translate_geometry(&mut local, -f64::from(ox), -f64::from(oy));
    crate::annotation::draw_annotation(&mut pixmap, &local);
    hud::blend_pixmap(buf, dw, dh, ox, oy, &pixmap);
}

fn translate_geometry(ann: &mut Annotation, dx: f64, dy: f64) {
    match ann.kind {
        AnnotationKind::Rectangle | AnnotationKind::Ellipse | AnnotationKind::Blur => {
            if ann.geometry.len() >= 4 {
                ann.geometry[0] += dx;
                ann.geometry[1] += dy;
            }
        }
        _ => {
            for chunk in ann.geometry.chunks_exact_mut(2) {
                chunk[0] += dx;
                chunk[1] += dy;
            }
        }
    }
}

fn annotation_bounds(ann: &Annotation, dw: u32, dh: u32) -> Option<(i32, i32, u32, u32)> {
    let stroke = ann.style.get("stroke_width").and_then(|v| v.as_f64()).unwrap_or(3.0);
    let (mut min_x, mut min_y, mut max_x, mut max_y) = match ann.kind {
        AnnotationKind::Rectangle | AnnotationKind::Ellipse | AnnotationKind::Blur => {
            if ann.geometry.len() < 4 {
                return None;
            }
            (
                ann.geometry[0],
                ann.geometry[1],
                ann.geometry[0] + ann.geometry[2],
                ann.geometry[1] + ann.geometry[3],
            )
        }
        AnnotationKind::Text => {
            if ann.geometry.len() < 2 {
                return None;
            }
            let text = ann.style.get("text").and_then(|v| v.as_str()).unwrap_or("");
            let scale = crate::annotation::text_pixel_scale(ann);
            let (tw, th) = crate::annotation::measure_bitmap_text(text, scale);
            (
                ann.geometry[0],
                ann.geometry[1],
                ann.geometry[0] + f64::from(tw),
                ann.geometry[1] + f64::from(th),
            )
        }
        _ => {
            if ann.geometry.len() < 2 {
                return None;
            }
            let mut bounds = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
            for chunk in ann.geometry.chunks_exact(2) {
                bounds.0 = bounds.0.min(chunk[0]);
                bounds.1 = bounds.1.min(chunk[1]);
                bounds.2 = bounds.2.max(chunk[0]);
                bounds.3 = bounds.3.max(chunk[1]);
            }
            bounds
        }
    };
    // 箭头头部、圆角描边都会溢出几何范围，统一留出余量。
    let pad = stroke * 2.0 + 24.0;
    min_x -= pad;
    min_y -= pad;
    max_x += pad;
    max_y += pad;

    let x0 = min_x.floor().clamp(0.0, f64::from(dw)) as i32;
    let y0 = min_y.floor().clamp(0.0, f64::from(dh)) as i32;
    let x1 = max_x.ceil().clamp(0.0, f64::from(dw)) as i32;
    let y1 = max_y.ceil().clamp(0.0, f64::from(dh)) as i32;
    if x1 <= x0 || y1 <= y0 {
        return None;
    }
    Some((x0, y0, (x1 - x0) as u32, (y1 - y0) as u32))
}

fn overlay_monitor(
    event_loop: &ActiveEventLoop,
    origin: (i32, i32),
) -> Option<winit::monitor::MonitorHandle> {
    event_loop
        .available_monitors()
        .find(|monitor| {
            let pos = monitor.position();
            pos.x == origin.0 && pos.y == origin.1
        })
        .or_else(|| event_loop.primary_monitor())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::MonitorInfo;

    fn monitor(width: u32, height: u32) -> MonitorInfo {
        MonitorInfo {
            id: 0,
            name: "test".into(),
            origin_physical: (0, 0),
            origin_logical: (0.0, 0.0),
            scale_factor: 2.0,
            capture_size: (width, height),
        }
    }

    fn frame(width: u32, height: u32) -> CapturedFrame {
        let mut bgra = Vec::with_capacity((width * height * 4) as usize);
        for y in 0..height {
            for x in 0..width {
                bgra.extend_from_slice(&[(x % 256) as u8, (y % 256) as u8, 200, 255]);
            }
        }
        CapturedFrame { width, height, bgra, monitor: monitor(width, height) }
    }

    #[test]
    fn maps_overlay_outcome_roundtrip() {
        let outcome = OverlayOutcome::Complete {
            selection: Selection { x: 10.0, y: 20.0, width: 300.0, height: 200.0 },
            scene: AnnotationScene::default(),
        };
        let json = serde_json::to_string(&outcome).unwrap();
        assert!(json.contains("\"action\":\"complete\""));
        let parsed: OverlayOutcome = serde_json::from_str(&json).unwrap();
        match parsed {
            OverlayOutcome::Complete { selection, .. } => assert_eq!(selection.width, 300.0),
            _ => panic!("unexpected outcome"),
        }
    }

    #[test]
    fn dim_layer_is_darker_but_keeps_the_picture() {
        let frame = frame(16, 8);
        let layers = Layers::build(&frame, 16, 8);
        for (bright, dim) in layers.bright.iter().zip(layers.dimmed.iter()) {
            assert_eq!(bright >> 24, 0xFF);
            assert_eq!(dim >> 24, 0xFF);
            for shift in [16, 8, 0] {
                let b = (bright >> shift) & 0xFF;
                let d = (dim >> shift) & 0xFF;
                assert!(d <= b, "遮罩层不能比原图更亮");
            }
        }
        // 不能暗到看不见内容：遮罩仍需保留原图的相对差异。
        assert_ne!(layers.dimmed.first(), layers.dimmed.last());
    }

    #[test]
    fn layers_resample_when_surface_differs_from_capture() {
        let frame = frame(32, 16);
        let layers = Layers::build(&frame, 16, 8);
        assert_eq!(layers.bright.len(), 16 * 8);
        assert_eq!(layers.dimmed.len(), 16 * 8);
    }

    #[test]
    fn buffer_mosaic_flattens_each_grid_cell() {
        let (dw, dh) = (32u32, 32u32);
        let mut buf: Vec<u32> = (0..dw * dh).map(|i| 0xFF00_0000 | i).collect();
        let ann = Annotation {
            id: "m".into(),
            kind: AnnotationKind::Mosaic,
            geometry: vec![0.0, 0.0, 16.0, 16.0],
            style: serde_json::json!({"block_size": 8}),
            z_index: 0,
        };
        let mask = crate::annotation::mosaic_mask(&ann).unwrap();
        apply_buffer_mosaic(&mut buf, dw, dh, &mask);
        for cell in 0..2u32 {
            let anchor = buf[(cell * 8 * dw) as usize];
            for y in 0..8 {
                for x in 0..8 {
                    assert_eq!(buf[((cell * 8 + y) * dw + x) as usize], anchor);
                }
            }
            assert_eq!(anchor >> 24, 0xFF, "马赛克必须保持不透明");
        }
        // 覆盖区外保持原值。
        assert_eq!(buf[(20 * dw + 20) as usize], 0xFF00_0000 | (20 * dw + 20));
    }

    #[test]
    fn annotation_bounds_pad_for_stroke_and_arrow_heads() {
        let ann = Annotation {
            id: "a".into(),
            kind: AnnotationKind::Arrow,
            geometry: vec![100.0, 100.0, 140.0, 140.0],
            style: serde_json::json!({"stroke_width": 4.0}),
            z_index: 0,
        };
        let (x, y, w, h) = annotation_bounds(&ann, 500, 500).unwrap();
        assert!(x < 100 && y < 100);
        assert!(w > 40 && h > 40);
    }

    #[test]
    fn resize_keeps_minimum_size_when_handle_crosses_opposite_edge() {
        let area = Area { x: 100.0, y: 100.0, width: 200.0, height: 200.0 };
        let resized = resize_area(area, HandlePos::TopLeft, 999.0, 999.0);
        assert!(resized.width >= 12.0 && resized.height >= 12.0);
        let resized = resize_area(area, HandlePos::BottomRight, -999.0, -999.0);
        assert!(resized.width >= 12.0 && resized.height >= 12.0);
    }
}
