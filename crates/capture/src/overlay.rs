use std::num::NonZeroU32;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use winit::application::ApplicationHandler;
use winit::event::{ElementState, MouseButton, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{Key, NamedKey};
use winit::window::{Fullscreen, Window, WindowId};

use crate::backend::{CaptureError, CapturedFrame};

/// Native Fast Overlay：冻结帧、选区、多屏、DPI。不承担复杂标注。
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Selection {
    /// 图片逻辑坐标，不是 CSS。
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

#[derive(Clone, Debug)]
pub enum OverlayEvent {
    Selected(Selection),
    Cancelled,
}

pub trait FastOverlay: Send {
    fn show_frozen(&mut self, frame: &CapturedFrame) -> Result<(), CaptureError>;
    fn take_event(&mut self) -> Option<OverlayEvent>;
    fn dismiss(&mut self);
}

/// 会话编排：热路径先冻结，再并行唤醒标注 WebView。
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

/// 独立事件循环：显示冻结帧并拖拽选区。Esc/右键取消。
pub fn select_region(frame: &CapturedFrame) -> Result<Option<Selection>, CaptureError> {
    let event_loop = EventLoop::new().map_err(|e| CaptureError::Failed(e.to_string()))?;
    event_loop.set_control_flow(ControlFlow::Wait);
    let mut app = OverlayApp {
        frame: frame.clone(),
        window: None,
        surface: None,
        dragging: false,
        start: None,
        current: None,
        result: None,
        done: false,
    };
    event_loop.run_app(&mut app).map_err(|e| CaptureError::Failed(e.to_string()))?;
    Ok(app.result)
}

struct OverlayApp {
    frame: CapturedFrame,
    window: Option<Arc<Window>>,
    surface: Option<softbuffer::Surface<Arc<Window>, Arc<Window>>>,
    dragging: bool,
    start: Option<(f64, f64)>,
    current: Option<(f64, f64)>,
    result: Option<Selection>,
    done: bool,
}

impl ApplicationHandler for OverlayApp {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        let attrs = Window::default_attributes()
            .with_title("Asterism")
            .with_fullscreen(Some(Fullscreen::Borderless(None)))
            .with_decorations(false);
        let window = Arc::new(event_loop.create_window(attrs).expect("overlay window"));
        let context = softbuffer::Context::new(window.clone()).expect("softbuffer");
        let surface = softbuffer::Surface::new(&context, window.clone()).expect("surface");
        self.window = Some(window);
        self.surface = Some(surface);
        self.redraw();
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => {
                self.result = None;
                self.done = true;
                event_loop.exit();
            }
            WindowEvent::KeyboardInput { event, .. } if event.state == ElementState::Pressed => {
                if event.logical_key == Key::Named(NamedKey::Escape) {
                    self.result = None;
                    self.done = true;
                    event_loop.exit();
                }
            }
            WindowEvent::MouseInput { state, button, .. } => {
                if button == MouseButton::Right && state == ElementState::Pressed {
                    self.result = None;
                    self.done = true;
                    event_loop.exit();
                }
                if button == MouseButton::Left {
                    match state {
                        ElementState::Pressed => {
                            self.dragging = true;
                            self.start = self.current;
                        }
                        ElementState::Released => {
                            self.dragging = false;
                            if let (Some(a), Some(b)) = (self.start, self.current) {
                                self.result = Some(norm_sel(a, b));
                                self.done = true;
                                event_loop.exit();
                            }
                        }
                    }
                }
            }
            WindowEvent::CursorMoved { position, .. } => {
                self.current = Some((position.x, position.y));
                if self.dragging {
                    self.redraw();
                }
            }
            WindowEvent::RedrawRequested => self.redraw(),
            _ => {}
        }
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
        blit_dimmed(&self.frame, &mut buf, size.width, size.height);
        if let (Some(a), Some(b), true) = (self.start, self.current, self.dragging) {
            stroke_rect(&mut buf, size.width, size.height, a, b);
        }
        let _ = buf.present();
    }
}

fn norm_sel(a: (f64, f64), b: (f64, f64)) -> Selection {
    let x = a.0.min(b.0);
    let y = a.1.min(b.1);
    Selection { x, y, width: (a.0 - b.0).abs().max(1.0), height: (a.1 - b.1).abs().max(1.0) }
}

fn blit_dimmed(frame: &CapturedFrame, buf: &mut [u32], dw: u32, dh: u32) {
    for y in 0..dh {
        let sy = (y as u64 * frame.height as u64 / dh as u64) as u32;
        for x in 0..dw {
            let sx = (x as u64 * frame.width as u64 / dw as u64) as u32;
            let i = ((sy * frame.width + sx) * 4) as usize;
            if i + 2 >= frame.bgra.len() {
                continue;
            }
            let b = (frame.bgra[i] as u16 * 3 / 5) as u32;
            let g = (frame.bgra[i + 1] as u16 * 3 / 5) as u32;
            let r = (frame.bgra[i + 2] as u16 * 3 / 5) as u32;
            buf[(y * dw + x) as usize] = (0xFF << 24) | (r << 16) | (g << 8) | b;
        }
    }
}

fn stroke_rect(buf: &mut [u32], dw: u32, dh: u32, a: (f64, f64), b: (f64, f64)) {
    let x0 = a.0.min(b.0).max(0.0) as u32;
    let y0 = a.1.min(b.1).max(0.0) as u32;
    let x1 = a.0.max(b.0).min(dw as f64 - 1.0) as u32;
    let y1 = a.1.max(b.1).min(dh as f64 - 1.0) as u32;
    let color = 0xFF_C6_E2_7A;
    for x in x0..=x1 {
        put(buf, dw, x, y0, color);
        put(buf, dw, x, y1, color);
    }
    for y in y0..=y1 {
        put(buf, dw, x0, y, color);
        put(buf, dw, x1, y, color);
    }
}

fn put(buf: &mut [u32], dw: u32, x: u32, y: u32, color: u32) {
    let i = (y * dw + x) as usize;
    if i < buf.len() {
        buf[i] = color;
    }
}
