use std::num::NonZeroU32;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use winit::application::ApplicationHandler;
use winit::dpi::{PhysicalPosition, PhysicalSize};
use winit::event::{ElementState, MouseButton, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{Key, NamedKey};
use winit::window::{Window, WindowId, WindowLevel};

use crate::annotation::{Annotation, AnnotationKind, AnnotationScene};
use crate::backend::{CaptureError, CapturedFrame, WindowInfo};

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

#[derive(Clone, Copy, Debug)]
struct RectArea {
    x: f64,
    y: f64,
    width: f64,
    height: f64,
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
    let event_loop = EventLoop::new().map_err(|e| CaptureError::Failed(e.to_string()))?;
    event_loop.set_control_flow(ControlFlow::Wait);
    let mut app = OverlayApp::new(frame.clone(), windows.to_vec());
    event_loop.run_app(&mut app).map_err(|e| CaptureError::Failed(e.to_string()))?;
    if let Some(err) = app.fail {
        return Err(CaptureError::Failed(err));
    }
    Ok(app.outcome)
}

struct OverlayApp {
    frame: CapturedFrame,
    windows: Vec<WindowInfo>,
    window: Option<Arc<Window>>,
    surface: Option<softbuffer::Surface<Arc<Window>, Arc<Window>>>,

    // Selection state in Window coordinates
    selection_rect: Option<RectArea>,
    hover_window_rect: Option<RectArea>,

    // Mouse state
    drag_mode: DragMode,
    drag_start: Option<(f64, f64)>,
    current_mouse: Option<(f64, f64)>,
    last_click_time: Option<std::time::Instant>,

    // Tools and annotations
    active_tool: ActiveTool,
    annotations: Vec<Annotation>,
    current_stroke: Vec<f64>,

    // Final outcome
    outcome: Option<OverlayOutcome>,
    fail: Option<String>,
}

impl OverlayApp {
    fn new(frame: CapturedFrame, windows: Vec<WindowInfo>) -> Self {
        Self {
            frame,
            windows,
            window: None,
            surface: None,
            selection_rect: None,
            hover_window_rect: None,
            drag_mode: DragMode::None,
            drag_start: None,
            current_mouse: None,
            last_click_time: None,
            active_tool: ActiveTool::None,
            annotations: Vec::new(),
            current_stroke: Vec::new(),
            outcome: None,
            fail: None,
        }
    }

    fn fail_and_exit(&mut self, event_loop: &ActiveEventLoop, err: String) {
        self.fail = Some(err);
        self.outcome = None;
        event_loop.exit();
    }

    fn translate_annotations_to_selection(&self) -> Vec<Annotation> {
        let mut translated = self.annotations.clone();
        if let (Some(area), Some(window)) = (self.selection_rect, &self.window) {
            let win_size = window.inner_size();
            if win_size.width > 0 && win_size.height > 0 {
                let sx = self.frame.width as f64 / win_size.width as f64;
                let sy = self.frame.height as f64 / win_size.height as f64;
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
            }
        }
        translated
    }

    fn commit_outcome_and_exit(
        &mut self,
        event_loop: &ActiveEventLoop,
        mut outcome: OverlayOutcome,
    ) {
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

    fn cancel_and_exit(&mut self, event_loop: &ActiveEventLoop) {
        self.outcome = Some(OverlayOutcome::Cancel);
        event_loop.exit();
    }

    fn current_selection_in_frame(&self) -> Option<Selection> {
        let area = self.selection_rect?;
        let window = self.window.as_ref()?;
        let win_size = window.inner_size();
        if win_size.width == 0 || win_size.height == 0 {
            return None;
        }
        let sx = self.frame.width as f64 / win_size.width as f64;
        let sy = self.frame.height as f64 / win_size.height as f64;
        let fx = (area.x * sx).floor().clamp(0.0, self.frame.width.saturating_sub(1) as f64);
        let fy = (area.y * sy).floor().clamp(0.0, self.frame.height.saturating_sub(1) as f64);
        let fw = (area.width * sx).round().clamp(1.0, (self.frame.width as f64 - fx).max(1.0));
        let fh = (area.height * sy).round().clamp(1.0, (self.frame.height as f64 - fy).max(1.0));
        Some(Selection { x: fx, y: fy, width: fw, height: fh })
    }

    fn find_window_under_cursor(&self, px: f64, py: f64) -> Option<RectArea> {
        let window = self.window.as_ref()?;
        let win_size = window.inner_size();
        if win_size.width == 0 || win_size.height == 0 {
            return None;
        }
        let origin_x = self.frame.monitor.origin_physical.0 as f64;
        let origin_y = self.frame.monitor.origin_physical.1 as f64;
        let phys_x = origin_x + (px * (self.frame.width as f64 / win_size.width as f64));
        let phys_y = origin_y + (py * (self.frame.height as f64 / win_size.height as f64));

        for win in self.windows.iter() {
            let wx = win.position.0 as f64;
            let wy = win.position.1 as f64;
            let ww = win.size.0 as f64;
            let wh = win.size.1 as f64;
            if ww > 30.0
                && wh > 30.0
                && phys_x >= wx
                && phys_x <= wx + ww
                && phys_y >= wy
                && phys_y <= wy + wh
            {
                let sx = win_size.width as f64 / self.frame.width as f64;
                let sy = win_size.height as f64 / self.frame.height as f64;
                let lx = ((wx - origin_x) * sx).clamp(0.0, win_size.width as f64);
                let ly = ((wy - origin_y) * sy).clamp(0.0, win_size.height as f64);
                let lw = (ww * sx).clamp(0.0, win_size.width as f64 - lx);
                let lh = (wh * sy).clamp(0.0, win_size.height as f64 - ly);
                if lw >= 20.0 && lh >= 20.0 {
                    return Some(RectArea { x: lx, y: ly, width: lw, height: lh });
                }
            }
        }
        None
    }
}

impl ApplicationHandler for OverlayApp {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        let target = overlay_monitor(event_loop, self.frame.monitor.origin_physical);
        let (pos, size) = match target.as_ref() {
            Some(monitor) => (monitor.position(), monitor.size()),
            None => (
                PhysicalPosition::new(
                    self.frame.monitor.origin_physical.0,
                    self.frame.monitor.origin_physical.1,
                ),
                PhysicalSize::new(self.frame.width.max(1), self.frame.height.max(1)),
            ),
        };
        let mut attrs = Window::default_attributes()
            .with_title("Asterism")
            .with_decorations(false)
            .with_resizable(false)
            .with_transparent(false)
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
        self.window = Some(window);
        self.surface = Some(surface);
        self.redraw();
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => {
                self.cancel_and_exit(event_loop);
            }
            WindowEvent::KeyboardInput { event, .. } if event.state == ElementState::Pressed => {
                match event.logical_key {
                    Key::Named(NamedKey::Escape) => {
                        if self.active_tool != ActiveTool::None {
                            self.active_tool = ActiveTool::None;
                            self.redraw();
                        } else if self.selection_rect.is_some() {
                            self.selection_rect = None;
                            self.annotations.clear();
                            self.redraw();
                        } else {
                            self.cancel_and_exit(event_loop);
                        }
                    }
                    Key::Named(NamedKey::Enter) => {
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
                    Key::Character(ref ch) => match ch.as_str().to_lowercase().as_str() {
                        "r" => {
                            self.active_tool = ActiveTool::Rectangle;
                            self.redraw();
                        }
                        "e" => {
                            self.active_tool = ActiveTool::Ellipse;
                            self.redraw();
                        }
                        "a" => {
                            self.active_tool = ActiveTool::Arrow;
                            self.redraw();
                        }
                        "p" => {
                            self.active_tool = ActiveTool::Brush;
                            self.redraw();
                        }
                        "m" => {
                            self.active_tool = ActiveTool::Mosaic;
                            self.redraw();
                        }
                        "t" => {
                            self.active_tool = ActiveTool::Text;
                            self.redraw();
                        }
                        "s" => {
                            if let Some(selection) = self.current_selection_in_frame() {
                                self.commit_outcome_and_exit(
                                    event_loop,
                                    OverlayOutcome::Scroll { selection },
                                );
                            }
                        }
                        "z" => {
                            self.annotations.pop();
                            self.redraw();
                        }
                        _ => {}
                    },
                    _ => {}
                }
            }
            WindowEvent::MouseInput { state, button, .. } => {
                if button == MouseButton::Right && state == ElementState::Pressed {
                    if self.active_tool != ActiveTool::None {
                        self.active_tool = ActiveTool::None;
                        self.redraw();
                    } else if self.selection_rect.is_some() {
                        self.selection_rect = None;
                        self.annotations.clear();
                        self.redraw();
                    } else {
                        self.cancel_and_exit(event_loop);
                    }
                    return;
                }

                if button == MouseButton::Left {
                    match state {
                        ElementState::Pressed => {
                            let now = std::time::Instant::now();
                            if let (Some(last), Some((mx, my))) =
                                (self.last_click_time, self.current_mouse)
                                && now.duration_since(last).as_millis() < 300
                                && let Some(area) = self.selection_rect
                                && mx >= area.x
                                && mx <= area.x + area.width
                                && my >= area.y
                                && my <= area.y + area.height
                                && self.active_tool == ActiveTool::None
                                && let Some(selection) = self.current_selection_in_frame()
                            {
                                self.commit_outcome_and_exit(
                                    event_loop,
                                    OverlayOutcome::Complete {
                                        selection,
                                        scene: AnnotationScene { items: self.annotations.clone() },
                                    },
                                );
                                return;
                            }
                            self.last_click_time = Some(now);

                            let Some((mx, my)) = self.current_mouse else { return };
                            self.drag_start = Some((mx, my));

                            // If selection is established, check if clicking on toolbar buttons
                            if let Some(area) = self.selection_rect {
                                let dh = self
                                    .window
                                    .as_ref()
                                    .map(|w| w.inner_size().height as f64)
                                    .unwrap_or(2160.0);
                                if let Some(action) = check_toolbar_click(mx, my, area, dh) {
                                    match action {
                                        ToolbarAction::SetTool(t) => {
                                            self.active_tool = if self.active_tool == t {
                                                ActiveTool::None
                                            } else {
                                                t
                                            };
                                            self.redraw();
                                        }
                                        ToolbarAction::Scroll => {
                                            if let Some(selection) =
                                                self.current_selection_in_frame()
                                            {
                                                self.commit_outcome_and_exit(
                                                    event_loop,
                                                    OverlayOutcome::Scroll { selection },
                                                );
                                            }
                                        }
                                        ToolbarAction::Undo => {
                                            self.annotations.pop();
                                            self.redraw();
                                        }
                                        ToolbarAction::Download => {
                                            if let Some(selection) =
                                                self.current_selection_in_frame()
                                            {
                                                self.commit_outcome_and_exit(
                                                    event_loop,
                                                    OverlayOutcome::Download {
                                                        selection,
                                                        scene: AnnotationScene {
                                                            items: self.annotations.clone(),
                                                        },
                                                    },
                                                );
                                            }
                                        }
                                        ToolbarAction::Pin => {
                                            if let Some(selection) =
                                                self.current_selection_in_frame()
                                            {
                                                self.commit_outcome_and_exit(
                                                    event_loop,
                                                    OverlayOutcome::Pin {
                                                        selection,
                                                        scene: AnnotationScene {
                                                            items: self.annotations.clone(),
                                                        },
                                                    },
                                                );
                                            }
                                        }
                                        ToolbarAction::Cancel => {
                                            self.cancel_and_exit(event_loop);
                                        }
                                        ToolbarAction::Done => {
                                            if let Some(selection) =
                                                self.current_selection_in_frame()
                                            {
                                                self.commit_outcome_and_exit(
                                                    event_loop,
                                                    OverlayOutcome::Complete {
                                                        selection,
                                                        scene: AnnotationScene {
                                                            items: self.annotations.clone(),
                                                        },
                                                    },
                                                );
                                            }
                                        }
                                    }
                                    return;
                                }

                                if self.active_tool != ActiveTool::None {
                                    // Start drawing annotation within selection
                                    self.drag_mode = DragMode::Drawing;
                                    self.current_stroke = vec![mx, my];
                                    return;
                                }

                                // Check resize handles
                                if let Some(handle) = check_handle_hit(mx, my, area) {
                                    self.drag_mode = DragMode::Resizing(handle);
                                    return;
                                }

                                // Check inside selection for moving
                                if mx >= area.x
                                    && mx <= area.x + area.width
                                    && my >= area.y
                                    && my <= area.y + area.height
                                {
                                    self.drag_mode = DragMode::Moving;
                                    return;
                                }
                            }

                            // Otherwise, if hovering over snapped window, snap immediately or start creating
                            if let Some(hover) = self.hover_window_rect {
                                self.selection_rect = Some(hover);
                            }
                            self.annotations.clear();
                            self.hover_window_rect = None;
                            self.drag_mode = DragMode::Creating;
                        }
                        ElementState::Released => {
                            match self.drag_mode {
                                DragMode::Creating => {
                                    if let (Some(a), Some(b)) =
                                        (self.drag_start, self.current_mouse)
                                    {
                                        let dx = (a.0 - b.0).abs();
                                        let dy = (a.1 - b.1).abs();
                                        if dx > 8.0 && dy > 8.0 {
                                            let x = a.0.min(b.0);
                                            let y = a.1.min(b.1);
                                            self.selection_rect =
                                                Some(RectArea { x, y, width: dx, height: dy });
                                        } else if let Some(hover) = self.hover_window_rect {
                                            self.selection_rect = Some(hover);
                                        }
                                    }
                                }
                                DragMode::Drawing => {
                                    if let (Some(a), Some(b)) =
                                        (self.drag_start, self.current_mouse)
                                    {
                                        if let Some(ann) = create_annotation(
                                            self.active_tool,
                                            a,
                                            b,
                                            &self.current_stroke,
                                            self.annotations.len(),
                                        ) {
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
                    }
                }
            }
            WindowEvent::CursorMoved { position, .. } => {
                let px = position.x;
                let py = position.y;
                self.current_mouse = Some((px, py));

                match self.drag_mode {
                    DragMode::Creating => {
                        if let Some(start) = self.drag_start {
                            let x = start.0.min(px);
                            let y = start.1.min(py);
                            let w = (start.0 - px).abs();
                            let h = (start.1 - py).abs();
                            self.selection_rect = Some(RectArea { x, y, width: w, height: h });
                        }
                        self.redraw();
                    }
                    DragMode::Moving => {
                        if let (Some(start), Some(area)) = (self.drag_start, self.selection_rect) {
                            let dx = px - start.0;
                            let dy = py - start.1;
                            self.selection_rect = Some(RectArea {
                                x: area.x + dx,
                                y: area.y + dy,
                                width: area.width,
                                height: area.height,
                            });
                            self.drag_start = Some((px, py));
                        }
                        self.redraw();
                    }
                    DragMode::Resizing(handle) => {
                        if let Some(area) = self.selection_rect {
                            let next = resize_rect(area, handle, px, py);
                            self.selection_rect = Some(next);
                        }
                        self.redraw();
                    }
                    DragMode::Drawing => {
                        self.current_stroke.push(px);
                        self.current_stroke.push(py);
                        self.redraw();
                    }
                    DragMode::None => {
                        if self.selection_rect.is_none() {
                            self.hover_window_rect = self.find_window_under_cursor(px, py);
                        }
                        self.redraw();
                    }
                }
            }
            WindowEvent::RedrawRequested => self.redraw(),
            _ => {}
        }
    }
}

enum ToolbarAction {
    SetTool(ActiveTool),
    Scroll,
    Undo,
    Download,
    Pin,
    Cancel,
    Done,
}

fn check_toolbar_click(mx: f64, my: f64, area: RectArea, dh: f64) -> Option<ToolbarAction> {
    let tb = toolbar_bounds(area, dh);
    if mx < tb.x || mx > tb.x + tb.width || my < tb.y || my > tb.y + tb.height {
        return None;
    }
    let offset = mx - tb.x - 4.0;
    if offset < 192.0 {
        let idx = (offset / 32.0).floor() as usize;
        match idx {
            0 => Some(ToolbarAction::SetTool(ActiveTool::Rectangle)),
            1 => Some(ToolbarAction::SetTool(ActiveTool::Ellipse)),
            2 => Some(ToolbarAction::SetTool(ActiveTool::Arrow)),
            3 => Some(ToolbarAction::SetTool(ActiveTool::Brush)),
            4 => Some(ToolbarAction::SetTool(ActiveTool::Mosaic)),
            5 => Some(ToolbarAction::SetTool(ActiveTool::Text)),
            _ => None,
        }
    } else if offset >= 200.0 {
        let idx = ((offset - 200.0) / 32.0).floor() as usize;
        match idx {
            0 => Some(ToolbarAction::Scroll),
            1 => Some(ToolbarAction::Undo),
            2 => Some(ToolbarAction::Download),
            3 => Some(ToolbarAction::Pin),
            4 => Some(ToolbarAction::Cancel),
            5 => Some(ToolbarAction::Done),
            _ => None,
        }
    } else {
        None
    }
}

fn toolbar_bounds(area: RectArea, dh: f64) -> RectArea {
    let tw = 400.0;
    let th = 38.0;
    let tx = (area.x + area.width - tw).max(area.x).max(8.0);
    let mut ty = area.y + area.height + 8.0;
    if ty + th > dh {
        ty = (area.y - th - 8.0).max(8.0);
    }
    RectArea { x: tx, y: ty, width: tw, height: th }
}

fn check_handle_hit(mx: f64, my: f64, area: RectArea) -> Option<HandlePos> {
    let size = 10.0;
    let (sx, sy, sw, sh) = (area.x, area.y, area.width, area.height);
    let handles = [
        (HandlePos::TopLeft, sx, sy),
        (HandlePos::TopCenter, sx + sw / 2.0, sy),
        (HandlePos::TopRight, sx + sw, sy),
        (HandlePos::MidLeft, sx, sy + sh / 2.0),
        (HandlePos::MidRight, sx + sw, sy + sh / 2.0),
        (HandlePos::BottomLeft, sx, sy + sh),
        (HandlePos::BottomCenter, sx + sw / 2.0, sy + sh),
        (HandlePos::BottomRight, sx + sw, sy + sh),
    ];
    for (pos, hx, hy) in handles {
        if (mx - hx).abs() <= size && (my - hy).abs() <= size {
            return Some(pos);
        }
    }
    None
}

fn resize_rect(area: RectArea, handle: HandlePos, mx: f64, my: f64) -> RectArea {
    let (sx, sy, sw, sh) = (area.x, area.y, area.width, area.height);
    match handle {
        HandlePos::TopLeft => {
            let right = sx + sw;
            let bottom = sy + sh;
            let nx = mx.min(right - 10.0);
            let ny = my.min(bottom - 10.0);
            RectArea { x: nx, y: ny, width: right - nx, height: bottom - ny }
        }
        HandlePos::TopCenter => {
            let bottom = sy + sh;
            let ny = my.min(bottom - 10.0);
            RectArea { x: sx, y: ny, width: sw, height: bottom - ny }
        }
        HandlePos::TopRight => {
            let bottom = sy + sh;
            let nw = (mx - sx).max(10.0);
            let ny = my.min(bottom - 10.0);
            RectArea { x: sx, y: ny, width: nw, height: bottom - ny }
        }
        HandlePos::MidLeft => {
            let right = sx + sw;
            let nx = mx.min(right - 10.0);
            RectArea { x: nx, y: sy, width: right - nx, height: sh }
        }
        HandlePos::MidRight => {
            let nw = (mx - sx).max(10.0);
            RectArea { x: sx, y: sy, width: nw, height: sh }
        }
        HandlePos::BottomLeft => {
            let right = sx + sw;
            let nx = mx.min(right - 10.0);
            let nh = (my - sy).max(10.0);
            RectArea { x: nx, y: sy, width: right - nx, height: nh }
        }
        HandlePos::BottomCenter => {
            let nh = (my - sy).max(10.0);
            RectArea { x: sx, y: sy, width: sw, height: nh }
        }
        HandlePos::BottomRight => {
            let nw = (mx - sx).max(10.0);
            let nh = (my - sy).max(10.0);
            RectArea { x: sx, y: sy, width: nw, height: nh }
        }
    }
}

fn create_annotation(
    tool: ActiveTool,
    start: (f64, f64),
    end: (f64, f64),
    stroke: &[f64],
    index: usize,
) -> Option<Annotation> {
    let id = format!("ann_{}", index);
    match tool {
        ActiveTool::Rectangle => {
            let x = start.0.min(end.0);
            let y = start.1.min(end.1);
            let w = (start.0 - end.0).abs();
            let h = (start.1 - end.1).abs();
            Some(Annotation {
                id,
                kind: AnnotationKind::Rectangle,
                geometry: vec![x, y, w, h],
                style: serde_json::json!({"stroke_width": 3.0, "color_r": 255, "color_g": 70, "color_b": 70}),
                z_index: index as i32,
            })
        }
        ActiveTool::Ellipse => {
            let x = start.0.min(end.0);
            let y = start.1.min(end.1);
            let w = (start.0 - end.0).abs();
            let h = (start.1 - end.1).abs();
            Some(Annotation {
                id,
                kind: AnnotationKind::Ellipse,
                geometry: vec![x, y, w, h],
                style: serde_json::json!({"stroke_width": 3.0, "color_r": 255, "color_g": 70, "color_b": 70}),
                z_index: index as i32,
            })
        }
        ActiveTool::Arrow => Some(Annotation {
            id,
            kind: AnnotationKind::Arrow,
            geometry: vec![start.0, start.1, end.0, end.1],
            style: serde_json::json!({"stroke_width": 3.5, "color_r": 255, "color_g": 70, "color_b": 70}),
            z_index: index as i32,
        }),
        ActiveTool::Brush => {
            if stroke.len() >= 4 {
                Some(Annotation {
                    id,
                    kind: AnnotationKind::Brush,
                    geometry: stroke.to_vec(),
                    style: serde_json::json!({"stroke_width": 3.0, "color_r": 255, "color_g": 70, "color_b": 70}),
                    z_index: index as i32,
                })
            } else {
                None
            }
        }
        ActiveTool::Mosaic => {
            if stroke.len() >= 2 {
                Some(Annotation {
                    id,
                    kind: AnnotationKind::Mosaic,
                    geometry: stroke.to_vec(),
                    style: serde_json::json!({"brush_radius": 14.0, "block_size": 10}),
                    z_index: index as i32,
                })
            } else {
                None
            }
        }
        ActiveTool::Text => Some(Annotation {
            id,
            kind: AnnotationKind::Text,
            geometry: vec![start.0, start.1],
            style: serde_json::json!({"text": "TEXT", "color_r": 255, "color_g": 70, "color_b": 70}),
            z_index: index as i32,
        }),
        ActiveTool::None => None,
    }
}

impl OverlayApp {
    fn redraw(&mut self) {
        let Some(window) = &self.window else { return };
        let Some(surface) = &mut self.surface else { return };
        let size = window.inner_size();
        let Some(w) = NonZeroU32::new(size.width) else { return };
        let Some(h) = NonZeroU32::new(size.height) else { return };
        if surface.resize(w, h).is_err() {
            return;
        }
        let Ok(mut buf) = surface.buffer_mut() else { return };

        let dw = size.width;
        let dh = size.height;

        let hole = self.selection_rect.or(self.hover_window_rect).map(|area| {
            (
                area.x.max(0.0) as u32,
                area.y.max(0.0) as u32,
                (area.x + area.width).min(dw as f64 - 1.0) as u32,
                (area.y + area.height).min(dh as f64 - 1.0) as u32,
            )
        });

        // 1. Blit dimmed background with clear cutout
        blit_dimmed(&self.frame, &mut buf, dw, dh, hole);

        // 2. Render real-time mosaic & annotations
        for ann in &self.annotations {
            render_overlay_annotation(&mut buf, dw, dh, ann);
        }

        // 3. Render active stroke preview
        if self.drag_mode == DragMode::Drawing
            && let (Some(a), Some(b)) = (self.drag_start, self.current_mouse)
            && let Some(ann) = create_annotation(self.active_tool, a, b, &self.current_stroke, 999)
        {
            render_overlay_annotation(&mut buf, dw, dh, &ann);
        }

        // 4. Render selection border or hover snap border
        if let Some(area) = self.selection_rect {
            stroke_selection_rect(&mut buf, dw, dh, area);
            render_handles(&mut buf, dw, dh, area);
            render_dimension_tag(&mut buf, dw, dh, area);
            render_floating_toolbar(&mut buf, dw, dh, area, self.active_tool);
        } else if let Some(hover) = self.hover_window_rect {
            stroke_hover_rect(&mut buf, dw, dh, hover);
        }

        // 5. Render Magnifier / Color Picker HUD when hovering or selecting
        if (self.selection_rect.is_none() || self.drag_mode == DragMode::Creating)
            && let Some((mx, my)) = self.current_mouse
        {
            render_magnifier_hud(&self.frame, &mut buf, dw, dh, mx, my);
        }

        let _ = buf.present();
    }
}

fn blit_dimmed(
    frame: &CapturedFrame,
    buf: &mut [u32],
    dw: u32,
    dh: u32,
    hole: Option<(u32, u32, u32, u32)>,
) {
    for y in 0..dh {
        let sy = (y as u64 * frame.height as u64 / dh as u64) as u32;
        for x in 0..dw {
            let sx = (x as u64 * frame.width as u64 / dw as u64) as u32;
            let i = ((sy * frame.width + sx) * 4) as usize;
            if i + 2 >= frame.bgra.len() {
                continue;
            }
            let in_hole =
                hole.is_some_and(|(x0, y0, x1, y1)| x >= x0 && x <= x1 && y >= y0 && y <= y1);
            let (b, g, r) = if in_hole {
                (frame.bgra[i] as u32, frame.bgra[i + 1] as u32, frame.bgra[i + 2] as u32)
            } else {
                (
                    (frame.bgra[i] as u32) / 3,
                    (frame.bgra[i + 1] as u32) / 3,
                    (frame.bgra[i + 2] as u32) / 3,
                )
            };
            buf[(y * dw + x) as usize] = (0xFF << 24) | (r << 16) | (g << 8) | b;
        }
    }
}

fn stroke_selection_rect(buf: &mut [u32], dw: u32, dh: u32, area: RectArea) {
    let x0 = area.x.max(0.0) as u32;
    let y0 = area.y.max(0.0) as u32;
    let x1 = (area.x + area.width).min(dw as f64 - 1.0) as u32;
    let y1 = (area.y + area.height).min(dh as f64 - 1.0) as u32;
    let color = 0xFF_02_84_C7; // Vibrant crisp blue border
    for x in x0..=x1 {
        put_pixel_safe(buf, dw, dh, x as i32, y0 as i32, color);
        put_pixel_safe(buf, dw, dh, x as i32, y0.saturating_add(1) as i32, color);
        put_pixel_safe(buf, dw, dh, x as i32, y1 as i32, color);
        put_pixel_safe(buf, dw, dh, x as i32, y1.saturating_sub(1) as i32, color);
    }
    for y in y0..=y1 {
        put_pixel_safe(buf, dw, dh, x0 as i32, y as i32, color);
        put_pixel_safe(buf, dw, dh, x0.saturating_add(1) as i32, y as i32, color);
        put_pixel_safe(buf, dw, dh, x1 as i32, y as i32, color);
        put_pixel_safe(buf, dw, dh, x1.saturating_sub(1) as i32, y as i32, color);
    }
}

fn stroke_hover_rect(buf: &mut [u32], dw: u32, dh: u32, area: RectArea) {
    let x0 = area.x.max(0.0) as u32;
    let y0 = area.y.max(0.0) as u32;
    let x1 = (area.x + area.width).min(dw as f64 - 1.0) as u32;
    let y1 = (area.y + area.height).min(dh as f64 - 1.0) as u32;
    let color = 0xFF_38_BD_F8; // Light cyan hover frame
    for x in x0..=x1 {
        if (x / 6) % 2 == 0 {
            put_pixel_safe(buf, dw, dh, x as i32, y0 as i32, color);
            put_pixel_safe(buf, dw, dh, x as i32, y1 as i32, color);
        }
    }
    for y in y0..=y1 {
        if (y / 6) % 2 == 0 {
            put_pixel_safe(buf, dw, dh, x0 as i32, y as i32, color);
            put_pixel_safe(buf, dw, dh, x1 as i32, y as i32, color);
        }
    }
}

fn render_handles(buf: &mut [u32], dw: u32, dh: u32, area: RectArea) {
    let (sx, sy, sw, sh) = (area.x, area.y, area.width, area.height);
    let pts = [
        (sx, sy),
        (sx + sw / 2.0, sy),
        (sx + sw, sy),
        (sx, sy + sh / 2.0),
        (sx + sw, sy + sh / 2.0),
        (sx, sy + sh),
        (sx + sw / 2.0, sy + sh),
        (sx + sw, sy + sh),
    ];
    for (hx, hy) in pts {
        fill_solid_rect(buf, dw, dh, hx as i32 - 4, hy as i32 - 4, 8, 8, 0xFF_02_84_C7);
        fill_solid_rect(buf, dw, dh, hx as i32 - 3, hy as i32 - 3, 6, 6, 0xFF_FF_FF_FF);
    }
}

fn render_dimension_tag(buf: &mut [u32], dw: u32, dh: u32, area: RectArea) {
    let tag_y = (area.y - 22.0).max(4.0) as i32;
    let tag_x = area.x.max(4.0) as i32;
    let text = format!("{} x {}", area.width as i32, area.height as i32);
    fill_solid_rect(buf, dw, dh, tag_x, tag_y, (text.len() * 7 + 10) as i32, 18, 0xDD_0F_17_2A);
    draw_simple_text(buf, dw, dh, tag_x + 5, tag_y + 4, &text, 0xFF_F8_FA_FC);
}

fn render_floating_toolbar(
    buf: &mut [u32],
    dw: u32,
    dh: u32,
    area: RectArea,
    active_tool: ActiveTool,
) {
    let tb = toolbar_bounds(area, dh as f64);
    let x = tb.x as i32;
    let y = tb.y as i32;
    let w = tb.width as i32;

    // Background bar
    fill_solid_rect(buf, dw, dh, x, y, w, 38, 0xF5_1E_29_3B);
    stroke_solid_rect(buf, dw, dh, x, y, w, 38, 0xFF_33_41_55);

    let tools = [
        ("[]", ActiveTool::Rectangle),
        ("()", ActiveTool::Ellipse),
        ("->", ActiveTool::Arrow),
        ("~~", ActiveTool::Brush),
        ("##", ActiveTool::Mosaic),
        ("T", ActiveTool::Text),
    ];

    let mut cur_x = x + 4;
    for (label, tool) in tools {
        let is_active = active_tool == tool;
        let bg = if is_active { 0xFF_02_84_C7 } else { 0x00_00_00_00 };
        if is_active {
            fill_solid_rect(buf, dw, dh, cur_x, y + 4, 30, 30, bg);
        }
        let fg = if is_active { 0xFF_FF_FF_FF } else { 0xFF_E2_E8_F0 };
        draw_simple_text(buf, dw, dh, cur_x + 8, y + 12, label, fg);
        cur_x += 32;
    }

    // Divider
    fill_solid_rect(buf, dw, dh, cur_x + 2, y + 8, 1, 22, 0xFF_47_55_69);
    cur_x += 8;

    let actions = [
        ("SC", 0xFF_E2_E8_F0),  // Scroll
        ("UN", 0xFF_E2_E8_F0),  // Undo
        ("DL", 0xFF_E2_E8_F0),  // Download
        ("PIN", 0xFF_E2_E8_F0), // Pin
        ("X", 0xFF_F8_71_71),   // Cancel
        ("OK", 0xFF_10_B9_81),  // Done
    ];

    for (label, fg) in actions {
        draw_simple_text(buf, dw, dh, cur_x + 6, y + 12, label, fg);
        cur_x += 32;
    }
}

fn render_magnifier_hud(
    frame: &CapturedFrame,
    buf: &mut [u32],
    dw: u32,
    dh: u32,
    mx: f64,
    my: f64,
) {
    let hud_w = 136i32;
    let hud_h = 110i32;
    let mut hx = mx as i32 + 20;
    let mut hy = my as i32 + 20;
    if hx + hud_w > dw as i32 - 10 {
        hx = mx as i32 - hud_w - 20;
    }
    if hy + hud_h > dh as i32 - 10 {
        hy = my as i32 - hud_h - 20;
    }

    // Main HUD frame
    fill_solid_rect(buf, dw, dh, hx, hy, hud_w, hud_h, 0xEE_0B_11_1A);
    stroke_solid_rect(buf, dw, dh, hx, hy, hud_w, hud_h, 0xFF_38_BD_F8);

    // Zoom grid (9x9 pixels rendered at 5x zoom = 45x45 box)
    let zoom_box_x = hx + 8;
    let zoom_box_y = hy + 8;
    let sx = (mx * frame.width as f64 / dw as f64) as i32;
    let sy = (my * frame.height as f64 / dh as f64) as i32;

    let center_color = get_frame_pixel_color(frame, sx, sy);
    let (cr, cg, cb) = (
        ((center_color >> 16) & 0xFF) as u8,
        ((center_color >> 8) & 0xFF) as u8,
        (center_color & 0xFF) as u8,
    );

    for gy in -4..=4 {
        for gx in -4..=4 {
            let px = sx + gx;
            let py = sy + gy;
            let color = get_frame_pixel_color(frame, px, py);
            let zx = zoom_box_x + (gx + 4) * 5;
            let zy = zoom_box_y + (gy + 4) * 5;
            fill_solid_rect(buf, dw, dh, zx, zy, 5, 5, color);
        }
    }

    // Zoom box center reticle
    stroke_solid_rect(buf, dw, dh, zoom_box_x + 20, zoom_box_y + 20, 5, 5, 0xFF_EF_44_44);

    // Color Swatch
    fill_solid_rect(buf, dw, dh, hx + 62, hy + 8, 20, 20, center_color);
    stroke_solid_rect(buf, dw, dh, hx + 62, hy + 8, 20, 20, 0xFF_FFFFFF);

    // Coordinates & RGB text
    let coord_text = format!("X:{} Y:{}", sx, sy);
    let hex_text = format!("#{:02X}{:02X}{:02X}", cr, cg, cb);
    let rgb_text = format!("RGB({},{},{})", cr, cg, cb);

    draw_simple_text(buf, dw, dh, hx + 8, hy + 58, &coord_text, 0xFF_F8_FA_FC);
    draw_simple_text(buf, dw, dh, hx + 8, hy + 74, &hex_text, 0xFF_38_BD_F8);
    draw_simple_text(buf, dw, dh, hx + 8, hy + 90, &rgb_text, 0xFF_94_A3_B8);
}

fn get_frame_pixel_color(frame: &CapturedFrame, x: i32, y: i32) -> u32 {
    if x < 0 || y < 0 || x >= frame.width as i32 || y >= frame.height as i32 {
        return 0xFF_00_00_00;
    }
    let idx = (y as usize * frame.width as usize + x as usize) * 4;
    if idx + 2 < frame.bgra.len() {
        let b = frame.bgra[idx] as u32;
        let g = frame.bgra[idx + 1] as u32;
        let r = frame.bgra[idx + 2] as u32;
        (0xFF << 24) | (r << 16) | (g << 8) | b
    } else {
        0xFF_00_00_00
    }
}

fn render_overlay_annotation(buf: &mut [u32], dw: u32, dh: u32, ann: &Annotation) {
    match ann.kind {
        AnnotationKind::Rectangle if ann.geometry.len() >= 4 => {
            stroke_solid_rect(
                buf,
                dw,
                dh,
                ann.geometry[0] as i32,
                ann.geometry[1] as i32,
                ann.geometry[2] as i32,
                ann.geometry[3] as i32,
                0xFF_EF_44_44,
            );
        }
        AnnotationKind::Ellipse if ann.geometry.len() >= 4 => {
            let cx = ann.geometry[0] as i32 + ann.geometry[2] as i32 / 2;
            let cy = ann.geometry[1] as i32 + ann.geometry[3] as i32 / 2;
            let rx = ann.geometry[2] as i32 / 2;
            let ry = ann.geometry[3] as i32 / 2;
            let steps = 60;
            for i in 0..steps {
                let t1 = (i as f32) * std::f32::consts::TAU / (steps as f32);
                let t2 = ((i + 1) as f32) * std::f32::consts::TAU / (steps as f32);
                let x1 = cx + (rx as f32 * t1.cos()) as i32;
                let y1 = cy + (ry as f32 * t1.sin()) as i32;
                let x2 = cx + (rx as f32 * t2.cos()) as i32;
                let y2 = cy + (ry as f32 * t2.sin()) as i32;
                draw_line(buf, dw, dh, x1, y1, x2, y2, 0xFF_EF_44_44, 3);
            }
        }
        AnnotationKind::Arrow if ann.geometry.len() >= 4 => {
            let x1 = ann.geometry[0] as i32;
            let y1 = ann.geometry[1] as i32;
            let x2 = ann.geometry[2] as i32;
            let y2 = ann.geometry[3] as i32;
            draw_line(buf, dw, dh, x1, y1, x2, y2, 0xFF_EF_44_44, 3);

            let dx = x2 as f32 - x1 as f32;
            let dy = y2 as f32 - y1 as f32;
            let len = (dx * dx + dy * dy).sqrt();
            if len > 0.0 {
                let ux = dx / len;
                let uy = dy / len;
                let arrow_len = 15.0;
                let c = 0.87758256; // cos(0.5 rad)
                let s = 0.47942554; // sin(0.5 rad)

                let rx1 = x2 as f32 - arrow_len * (ux * c - uy * s);
                let ry1 = y2 as f32 - arrow_len * (ux * s + uy * c);
                let rx2 = x2 as f32 - arrow_len * (ux * c + uy * s);
                let ry2 = y2 as f32 - arrow_len * (-ux * s + uy * c);

                draw_line(buf, dw, dh, x2, y2, rx1 as i32, ry1 as i32, 0xFF_EF_44_44, 3);
                draw_line(buf, dw, dh, x2, y2, rx2 as i32, ry2 as i32, 0xFF_EF_44_44, 3);
            }
        }
        AnnotationKind::Mosaic => {
            if ann.geometry.len() >= 2 {
                for chunk in ann.geometry.chunks_exact(2) {
                    let cx = chunk[0] as i32;
                    let cy = chunk[1] as i32;
                    apply_buf_mosaic_block(buf, dw, dh, cx - 14, cy - 14, 28, 28);
                }
            }
        }
        AnnotationKind::Brush => {
            for chunk in ann.geometry.chunks_exact(2) {
                fill_solid_rect(
                    buf,
                    dw,
                    dh,
                    chunk[0] as i32 - 2,
                    chunk[1] as i32 - 2,
                    4,
                    4,
                    0xFF_EF_44_44,
                );
            }
        }
        AnnotationKind::Text if ann.geometry.len() >= 2 => {
            let text = ann.style.get("text").and_then(|v| v.as_str()).unwrap_or("TEXT");
            draw_simple_text(
                buf,
                dw,
                dh,
                ann.geometry[0] as i32,
                ann.geometry[1] as i32,
                text,
                0xFF_EF_44_44,
            );
        }
        _ => {}
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_line(
    buf: &mut [u32],
    dw: u32,
    dh: u32,
    x0: i32,
    y0: i32,
    x1: i32,
    y1: i32,
    color: u32,
    thickness: i32,
) {
    let dx = (x1 - x0).abs();
    let dy = -(y1 - y0).abs();
    let sx = if x0 < x1 { 1 } else { -1 };
    let sy = if y0 < y1 { 1 } else { -1 };
    let mut err = dx + dy;
    let mut x = x0;
    let mut y = y0;

    loop {
        for ty in -thickness / 2..=thickness / 2 {
            for tx in -thickness / 2..=thickness / 2 {
                if tx * tx + ty * ty <= (thickness * thickness) / 4 {
                    put_pixel_safe(buf, dw, dh, x + tx, y + ty, color);
                }
            }
        }
        if x == x1 && y == y1 {
            break;
        }
        let e2 = 2 * err;
        if e2 >= dy {
            err += dy;
            x += sx;
        }
        if e2 <= dx {
            err += dx;
            y += sy;
        }
    }
}

fn apply_buf_mosaic_block(buf: &mut [u32], dw: u32, dh: u32, x: i32, y: i32, w: i32, h: i32) {
    let x0 = x.clamp(0, dw as i32);
    let y0 = y.clamp(0, dh as i32);
    let x1 = (x + w).clamp(0, dw as i32);
    let y1 = (y + h).clamp(0, dh as i32);
    let b = 8;
    for by in (y0..y1).step_by(b as usize) {
        for bx in (x0..x1).step_by(b as usize) {
            let sample_idx = (by * dw as i32 + bx) as usize;
            if sample_idx < buf.len() {
                let color = buf[sample_idx];
                for yy in by..(by + b).min(y1) {
                    for xx in bx..(bx + b).min(x1) {
                        put_pixel_safe(buf, dw, dh, xx, yy, color);
                    }
                }
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn fill_solid_rect(buf: &mut [u32], dw: u32, dh: u32, x: i32, y: i32, w: i32, h: i32, color: u32) {
    let x0 = x.clamp(0, dw as i32);
    let y0 = y.clamp(0, dh as i32);
    let x1 = (x + w).clamp(0, dw as i32);
    let y1 = (y + h).clamp(0, dh as i32);
    for py in y0..y1 {
        for px in x0..x1 {
            put_pixel_safe(buf, dw, dh, px, py, color);
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn stroke_solid_rect(
    buf: &mut [u32],
    dw: u32,
    dh: u32,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    color: u32,
) {
    let x0 = x.clamp(0, dw as i32);
    let y0 = y.clamp(0, dh as i32);
    let x1 = (x + w).clamp(0, dw as i32 - 1);
    let y1 = (y + h).clamp(0, dh as i32 - 1);
    for px in x0..=x1 {
        put_pixel_safe(buf, dw, dh, px, y0, color);
        put_pixel_safe(buf, dw, dh, px, y1, color);
    }
    for py in y0..=y1 {
        put_pixel_safe(buf, dw, dh, x0, py, color);
        put_pixel_safe(buf, dw, dh, x1, py, color);
    }
}

fn draw_simple_text(buf: &mut [u32], dw: u32, dh: u32, x: i32, y: i32, text: &str, color: u32) {
    let mut cx = x;
    for ch in text.chars() {
        let glyph = simple_glyph_for(ch);
        for (row, bits) in glyph.iter().enumerate() {
            for col in 0..5 {
                if bits & (1 << (4 - col)) != 0 {
                    put_pixel_safe(buf, dw, dh, cx + col, y + row as i32, color);
                }
            }
        }
        cx += 6;
    }
}

fn simple_glyph_for(ch: char) -> [u8; 7] {
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
        'K' => [0x11, 0x12, 0x14, 0x18, 0x14, 0x12, 0x11],
        'L' => [0x10, 0x10, 0x10, 0x10, 0x10, 0x10, 0x1F],
        'M' => [0x11, 0x1B, 0x15, 0x15, 0x11, 0x11, 0x11],
        'N' => [0x11, 0x19, 0x15, 0x13, 0x11, 0x11, 0x11],
        'O' => [0x0E, 0x11, 0x11, 0x11, 0x11, 0x11, 0x0E],
        'P' => [0x1E, 0x11, 0x11, 0x1E, 0x10, 0x10, 0x10],
        'R' => [0x1E, 0x11, 0x11, 0x1E, 0x14, 0x12, 0x11],
        'S' => [0x0F, 0x10, 0x10, 0x0E, 0x01, 0x01, 0x1E],
        'T' => [0x1F, 0x04, 0x04, 0x04, 0x04, 0x04, 0x04],
        'U' => [0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x0E],
        'X' => [0x11, 0x11, 0x0A, 0x04, 0x0A, 0x11, 0x11],
        'Y' => [0x11, 0x11, 0x0A, 0x04, 0x04, 0x04, 0x04],
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
        ',' => [0x00, 0x00, 0x00, 0x00, 0x00, 0x04, 0x08],
        '#' => [0x0A, 0x0A, 0x1F, 0x0A, 0x1F, 0x0A, 0x0A],
        '(' => [0x02, 0x04, 0x08, 0x08, 0x08, 0x04, 0x02],
        ')' => [0x08, 0x04, 0x02, 0x02, 0x02, 0x04, 0x08],
        '[' => [0x0E, 0x08, 0x08, 0x08, 0x08, 0x08, 0x0E],
        ']' => [0x0E, 0x02, 0x02, 0x02, 0x02, 0x02, 0x0E],
        '-' => [0x00, 0x00, 0x00, 0x1F, 0x00, 0x00, 0x00],
        '>' => [0x10, 0x08, 0x04, 0x02, 0x04, 0x08, 0x10],
        '~' => [0x00, 0x00, 0x0D, 0x16, 0x00, 0x00, 0x00],
        _ => [0x1F, 0x11, 0x11, 0x11, 0x11, 0x11, 0x1F],
    }
}

fn put_pixel_safe(buf: &mut [u32], dw: u32, dh: u32, x: i32, y: i32, color: u32) {
    if x < 0 || y < 0 || x >= dw as i32 || y >= dh as i32 {
        return;
    }
    let i = (y as u32 * dw + x as u32) as usize;
    if i < buf.len() {
        buf[i] = color;
    }
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
            OverlayOutcome::Complete { selection, .. } => {
                assert_eq!(selection.width, 300.0);
            }
            _ => panic!("unexpected outcome"),
        }
    }
}
