use crate::backend::{CaptureError, CapturedFrame};

#[derive(Clone, Debug)]
pub struct StitchFrame {
    pub width: u32,
    pub height: u32,
    pub bgra: Vec<u8>,
}

/// 滚动截图：downscale → grayscale → overlap search → confidence。
/// 不引入 OpenCV。连续低置信度必须停自动滚动并保留当前结果。
pub struct ScrollCaptureEngine {
    pub tiles: Vec<StitchFrame>,
    pub low_confidence_streak: u32,
    pub threshold: f32,
}

impl Default for ScrollCaptureEngine {
    fn default() -> Self {
        Self { tiles: Vec::new(), low_confidence_streak: 0, threshold: 0.35 }
    }
}

impl ScrollCaptureEngine {
    pub fn push(&mut self, frame: &CapturedFrame) -> Result<f32, CaptureError> {
        let tile =
            StitchFrame { width: frame.width, height: frame.height, bgra: frame.bgra.clone() };
        let confidence =
            self.tiles.last().map(|prev| overlap_confidence(prev, &tile)).unwrap_or(1.0);
        if confidence < self.threshold {
            self.low_confidence_streak += 1;
        } else {
            self.low_confidence_streak = 0;
            self.tiles.push(tile);
        }
        Ok(confidence)
    }

    pub fn should_stop_auto(&self) -> bool {
        self.low_confidence_streak >= 3
    }

    pub fn flatten(&self) -> Option<StitchFrame> {
        if self.tiles.is_empty() {
            return None;
        }
        let width = self.tiles[0].width;
        let mut height = 0u32;
        for t in &self.tiles {
            height = height.saturating_add(t.height / 2);
        }
        height = height.saturating_add(self.tiles.last()?.height / 2);
        let mut bgra = vec![0u8; (width * height * 4) as usize];
        let mut y = 0u32;
        for tile in &self.tiles {
            let h = if std::ptr::eq(tile, self.tiles.last().unwrap()) {
                tile.height
            } else {
                tile.height / 2
            };
            for row in 0..h.min(tile.height) {
                let src = ((row * tile.width) * 4) as usize;
                let dst = ((y + row) * width * 4) as usize;
                let n = (tile.width * 4) as usize;
                if dst + n <= bgra.len() && src + n <= tile.bgra.len() {
                    bgra[dst..dst + n].copy_from_slice(&tile.bgra[src..src + n]);
                }
            }
            y += h;
        }
        Some(StitchFrame { width, height: y.max(1), bgra })
    }
}

fn overlap_confidence(a: &StitchFrame, b: &StitchFrame) -> f32 {
    if a.width == 0 || b.width == 0 {
        return 0.0;
    }
    let band = a.height.clamp(8, 48);
    let mut err = 0u64;
    let mut n = 0u64;
    for y in 0..band {
        let ay = a.height.saturating_sub(band) + y;
        for x in (0..a.width.min(b.width)).step_by(8) {
            let ai = ((ay * a.width + x) * 4) as usize;
            let bi = ((y * b.width + x) * 4) as usize;
            if ai + 2 < a.bgra.len() && bi + 2 < b.bgra.len() {
                let da = a.bgra[ai] as i32 - b.bgra[bi] as i32;
                err += da.unsigned_abs() as u64;
                n += 1;
            }
        }
    }
    if n == 0 {
        return 0.0;
    }
    1.0 - (err as f32 / (n as f32 * 255.0)).clamp(0.0, 1.0)
}
