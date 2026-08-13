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
    overlaps: Vec<u32>,
    pub low_confidence_streak: u32,
    pub threshold: f32,
}

impl Default for ScrollCaptureEngine {
    fn default() -> Self {
        Self { tiles: Vec::new(), overlaps: Vec::new(), low_confidence_streak: 0, threshold: 0.82 }
    }
}

impl ScrollCaptureEngine {
    pub fn push(&mut self, frame: &CapturedFrame) -> Result<f32, CaptureError> {
        let tile =
            StitchFrame { width: frame.width, height: frame.height, bgra: frame.bgra.clone() };
        let Some(previous) = self.tiles.last() else {
            self.tiles.push(tile);
            return Ok(1.0);
        };
        let matched = best_overlap(previous, &tile);
        let no_progress = matched.overlap_rows + 2 >= previous.height.min(tile.height);
        if matched.confidence < self.threshold || no_progress {
            self.low_confidence_streak += 1;
        } else {
            self.low_confidence_streak = 0;
            self.overlaps.push(matched.overlap_rows);
            self.tiles.push(tile);
        }
        Ok(matched.confidence)
    }

    pub fn should_stop_auto(&self) -> bool {
        self.low_confidence_streak >= 3
    }

    /// macOS 自动滚动：注入 CGEvent 滚轮。Windows 由 SendInput 在平台模块补。
    pub fn inject_scroll(delta: i32) {
        #[cfg(target_os = "macos")]
        unsafe {
            macos_scroll(delta);
        }
        let _ = delta;
    }

    pub fn flatten(&self) -> Option<StitchFrame> {
        if self.tiles.is_empty() {
            return None;
        }
        let width = self.tiles[0].width;
        if self.tiles.iter().any(|tile| tile.width != width) {
            return None;
        }
        let mut height = self.tiles[0].height;
        for (tile, overlap) in self.tiles.iter().skip(1).zip(&self.overlaps) {
            height = height.saturating_add(tile.height.saturating_sub(*overlap));
        }
        let mut bgra = vec![0u8; (width * height * 4) as usize];
        let mut y = 0u32;
        for (index, tile) in self.tiles.iter().enumerate() {
            let first_row = if index == 0 { 0 } else { self.overlaps[index - 1] };
            for row in first_row..tile.height {
                let src = ((row * tile.width) * 4) as usize;
                let dst = ((y + row - first_row) * width * 4) as usize;
                let n = (tile.width * 4) as usize;
                if dst + n <= bgra.len() && src + n <= tile.bgra.len() {
                    bgra[dst..dst + n].copy_from_slice(&tile.bgra[src..src + n]);
                }
            }
            y += tile.height.saturating_sub(first_row);
        }
        Some(StitchFrame { width, height: y.max(1), bgra })
    }
}

#[cfg(target_os = "macos")]
unsafe fn macos_scroll(delta: i32) {
    #[link(name = "CoreGraphics", kind = "framework")]
    unsafe extern "C" {
        fn CGEventCreateScrollWheelEvent(
            source: *mut std::ffi::c_void,
            units: u32,
            wheel_count: u32,
            wheel1: i32,
        ) -> *mut std::ffi::c_void;
        fn CGEventPost(tap: u32, event: *mut std::ffi::c_void);
        fn CFRelease(cf: *mut std::ffi::c_void);
    }
    unsafe {
        let ev = CGEventCreateScrollWheelEvent(std::ptr::null_mut(), 0, 1, delta);
        if !ev.is_null() {
            CGEventPost(0, ev);
            CFRelease(ev);
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct OverlapMatch {
    overlap_rows: u32,
    confidence: f32,
}

fn best_overlap(a: &StitchFrame, b: &StitchFrame) -> OverlapMatch {
    if a.width == 0 || a.width != b.width || a.height < 8 || b.height < 8 {
        return OverlapMatch { overlap_rows: 0, confidence: 0.0 };
    }
    let max_overlap = a.height.min(b.height);
    let mut best = OverlapMatch { overlap_rows: 0, confidence: 0.0 };
    for overlap in 8..=max_overlap {
        let confidence = overlap_score(a, b, overlap);
        if confidence >= best.confidence {
            best = OverlapMatch { overlap_rows: overlap, confidence };
        }
    }
    best
}

fn overlap_score(a: &StitchFrame, b: &StitchFrame, overlap: u32) -> f32 {
    let x_samples = a.width.min(32);
    let y_samples = overlap.min(24);
    let mut error = 0u64;
    let mut samples = 0u64;
    for yi in 0..y_samples {
        let offset_y = sample_position(yi, y_samples, overlap);
        let ay = a.height - overlap + offset_y;
        let by = offset_y;
        for xi in 0..x_samples {
            let x = sample_position(xi, x_samples, a.width);
            let ai = ((ay * a.width + x) * 4) as usize;
            let bi = ((by * b.width + x) * 4) as usize;
            if ai + 2 < a.bgra.len() && bi + 2 < b.bgra.len() {
                for channel in 0..3 {
                    error += (a.bgra[ai + channel] as i32 - b.bgra[bi + channel] as i32)
                        .unsigned_abs() as u64;
                    samples += 1;
                }
            }
        }
    }
    if samples == 0 {
        return 0.0;
    }
    1.0 - (error as f32 / (samples as f32 * 255.0)).clamp(0.0, 1.0)
}

fn sample_position(index: u32, samples: u32, extent: u32) -> u32 {
    if samples <= 1 || extent <= 1 { 0 } else { index * (extent - 1) / (samples - 1) }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(start_row: u32, height: u32) -> CapturedFrame {
        let width = 16;
        let mut bgra = Vec::with_capacity((width * height * 4) as usize);
        for row in start_row..start_row + height {
            for x in 0..width {
                bgra.extend_from_slice(&[
                    row as u8,
                    (row.wrapping_mul(3) + x) as u8,
                    (row.wrapping_mul(7) + x.wrapping_mul(5)) as u8,
                    255,
                ]);
            }
        }
        CapturedFrame {
            width,
            height,
            bgra,
            monitor: crate::backend::MonitorInfo {
                id: 1,
                name: "test".into(),
                origin_physical: (0, 0),
                origin_logical: (0.0, 0.0),
                scale_factor: 1.0,
                capture_size: (width, height),
            },
        }
    }

    #[test]
    fn stitches_using_detected_overlap() {
        let mut engine = ScrollCaptureEngine::default();
        engine.push(&frame(0, 100)).unwrap();
        let confidence = engine.push(&frame(60, 100)).unwrap();

        assert!(confidence > 0.99);
        assert_eq!(engine.overlaps, vec![40]);
        let stitched = engine.flatten().unwrap();
        assert_eq!(stitched.height, 160);
        assert_eq!(stitched.bgra[((120 * stitched.width) * 4) as usize], 120);
    }

    #[test]
    fn repeated_static_frames_stop_auto_without_duplication() {
        let mut engine = ScrollCaptureEngine::default();
        let static_frame = frame(0, 100);
        engine.push(&static_frame).unwrap();
        for _ in 0..3 {
            engine.push(&static_frame).unwrap();
        }

        assert!(engine.should_stop_auto());
        assert_eq!(engine.tiles.len(), 1);
        assert_eq!(engine.flatten().unwrap().height, 100);
    }
}
