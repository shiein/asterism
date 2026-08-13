use gif::{Encoder, Frame, Repeat};

use crate::{MediaError, VideoFrame};

/// GIF 默认 10–15 FPS，区域录制。不追求 60 FPS。
pub struct GifSession {
    width: u16,
    height: u16,
    frames: Vec<Vec<u8>>,
    delay_cs: u16,
}

impl GifSession {
    pub fn new(width: u32, height: u32, fps: u16) -> Self {
        let fps = fps.clamp(8, 15);
        Self {
            width: width.min(u16::MAX as u32) as u16,
            height: height.min(u16::MAX as u32) as u16,
            frames: Vec::new(),
            delay_cs: (100 / fps).max(6),
        }
    }

    pub fn push(&mut self, frame: &VideoFrame) -> Result<(), MediaError> {
        if frame.width as u16 != self.width || frame.height as u16 != self.height {
            return Err(MediaError::Failed("frame size changed".into()));
        }
        self.frames.push(bgra_to_rgba(&frame.bgra));
        Ok(())
    }

    pub fn finish(self) -> Result<Vec<u8>, MediaError> {
        let mut out = Vec::new();
        {
            let mut enc = Encoder::new(&mut out, self.width, self.height, &[])
                .map_err(|e| MediaError::Failed(e.to_string()))?;
            enc.set_repeat(Repeat::Infinite).map_err(|e| MediaError::Failed(e.to_string()))?;
            for rgba in &self.frames {
                let mut frame =
                    Frame::from_rgba_speed(self.width, self.height, &mut rgba.clone(), 10);
                frame.delay = self.delay_cs;
                enc.write_frame(&frame).map_err(|e| MediaError::Failed(e.to_string()))?;
            }
        }
        Ok(out)
    }
}

pub fn bgra_to_rgba_pub(bgra: &[u8]) -> Vec<u8> {
    bgra_to_rgba(bgra)
}

fn bgra_to_rgba(bgra: &[u8]) -> Vec<u8> {
    let mut out = bgra.to_vec();
    for px in out.chunks_exact_mut(4) {
        px.swap(0, 2);
    }
    out
}
