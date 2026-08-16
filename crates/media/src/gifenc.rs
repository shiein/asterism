use gif::{Encoder, Frame, Repeat};

use crate::{MediaError, VideoFrame};

/// GIF 默认 10–15 FPS，区域录制。不追求 60 FPS。
pub struct GifSession {
    width: u16,
    height: u16,
    encoder: Encoder<Vec<u8>>,
    delay_cs: u16,
}

impl GifSession {
    pub fn new(width: u32, height: u32, fps: u16) -> Result<Self, MediaError> {
        let fps = fps.clamp(8, 15);
        let width = width.min(u16::MAX as u32) as u16;
        let height = height.min(u16::MAX as u32) as u16;
        let mut encoder = Encoder::new(Vec::new(), width, height, &[])
            .map_err(|e| MediaError::Failed(e.to_string()))?;
        encoder.set_repeat(Repeat::Infinite).map_err(|e| MediaError::Failed(e.to_string()))?;
        Ok(Self { width, height, encoder, delay_cs: (100 / fps).max(6) })
    }

    pub fn push(&mut self, frame: &VideoFrame) -> Result<(), MediaError> {
        if frame.width as u16 != self.width || frame.height as u16 != self.height {
            return Err(MediaError::Failed("frame size changed".into()));
        }
        let mut rgba = bgra_to_rgba(&frame.bgra);
        let mut frame = Frame::from_rgba_speed(self.width, self.height, &mut rgba, 10);
        frame.delay = self.delay_cs;
        self.encoder.write_frame(&frame).map_err(|e| MediaError::Failed(e.to_string()))
    }

    pub fn finish(self) -> Result<Vec<u8>, MediaError> {
        self.encoder.into_inner().map_err(|e| MediaError::Failed(e.to_string()))
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encodes_frames_incrementally() {
        let mut session = GifSession::new(2, 2, 10).unwrap();
        for value in [0, 64, 128] {
            let mut bgra = Vec::with_capacity(16);
            for _ in 0..4 {
                bgra.extend_from_slice(&[value, value, value, 255]);
            }
            session.push(&VideoFrame { timestamp_us: 0, width: 2, height: 2, bgra }).unwrap();
        }
        let bytes = session.finish().unwrap();
        assert!(bytes.starts_with(b"GIF"));
    }
}
