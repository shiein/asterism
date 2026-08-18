use std::io::Write;

use crate::{MediaError, VideoFrame};

/// 最小可播放 Motion-JPEG AVI。不依赖 ffmpeg。
pub struct AviMjpeg {
    width: u32,
    height: u32,
    fps: u32,
    frames: Vec<Vec<u8>>,
}

impl AviMjpeg {
    pub fn new(width: u32, height: u32, fps: u32) -> Self {
        Self { width, height, fps: fps.max(1), frames: Vec::new() }
    }

    pub fn push(&mut self, frame: &VideoFrame) -> Result<(), MediaError> {
        let img = image::RgbaImage::from_raw(
            frame.width,
            frame.height,
            crate::gifenc::bgra_to_rgba_pub(&frame.bgra),
        )
        .ok_or_else(|| MediaError::Failed("bad frame".into()))?;
        let mut jpeg = Vec::new();
        image::DynamicImage::ImageRgba8(img)
            .write_to(&mut std::io::Cursor::new(&mut jpeg), image::ImageFormat::Jpeg)
            .map_err(|e| MediaError::Failed(e.to_string()))?;
        self.frames.push(jpeg);
        Ok(())
    }

    pub fn finish(self) -> Result<Vec<u8>, MediaError> {
        let mut movi = Vec::new();
        let mut idx = Vec::new();
        for jpeg in &self.frames {
            let off = movi.len() as u32;
            movi.extend_from_slice(b"00dc");
            movi.extend_from_slice(&(jpeg.len() as u32).to_le_bytes());
            movi.extend_from_slice(jpeg);
            if jpeg.len() % 2 == 1 {
                movi.push(0);
            }
            idx.extend_from_slice(b"00dc");
            idx.extend_from_slice(&16u32.to_le_bytes());
            idx.extend_from_slice(&off.to_le_bytes());
            idx.extend_from_slice(&(jpeg.len() as u32).to_le_bytes());
        }
        let max_frame = self.frames.iter().map(Vec::len).max().unwrap_or(0) as u32;
        let mut hdrl = Vec::new();
        write_avih(&mut hdrl, self.width, self.height, self.fps, self.frames.len() as u32);
        let mut strl = Vec::new();
        write_strh(
            &mut strl,
            self.width,
            self.height,
            self.fps,
            self.frames.len() as u32,
            max_frame,
        );
        write_strf(&mut strl, self.width, self.height);
        write_list(&mut hdrl, b"strl", &strl);
        // idx1 offsets are relative to the 'movi' FourCC (offset 4 inside the LIST payload).
        for chunk in idx.chunks_mut(16) {
            if chunk.len() == 16 {
                let off = u32::from_le_bytes(chunk[8..12].try_into().unwrap_or([0; 4]));
                chunk[8..12].copy_from_slice(&(off + 4).to_le_bytes());
            }
        }
        let mut out = Vec::new();
        out.extend_from_slice(b"RIFF");
        let riff_size_at = out.len();
        out.extend_from_slice(&0u32.to_le_bytes());
        out.extend_from_slice(b"AVI ");
        write_list(&mut out, b"hdrl", &hdrl);
        write_list(&mut out, b"movi", &movi);
        out.extend_from_slice(b"idx1");
        out.extend_from_slice(&(idx.len() as u32).to_le_bytes());
        out.extend_from_slice(&idx);
        let size = (out.len() - 8) as u32;
        out[riff_size_at..riff_size_at + 4].copy_from_slice(&size.to_le_bytes());
        Ok(out)
    }
}

fn write_list(out: &mut Vec<u8>, fourcc: &[u8; 4], body: &[u8]) {
    out.extend_from_slice(b"LIST");
    out.extend_from_slice(&((body.len() + 4) as u32).to_le_bytes());
    out.extend_from_slice(fourcc);
    out.extend_from_slice(body);
}

fn write_avih(out: &mut Vec<u8>, w: u32, h: u32, fps: u32, frames: u32) {
    let us = 1_000_000 / fps.max(1);
    out.extend_from_slice(b"avih");
    out.extend_from_slice(&56u32.to_le_bytes());
    let mut b = vec![0u8; 56];
    b[0..4].copy_from_slice(&us.to_le_bytes());
    b[16..20].copy_from_slice(&16u32.to_le_bytes());
    b[24..28].copy_from_slice(&frames.to_le_bytes());
    b[32..36].copy_from_slice(&1u32.to_le_bytes());
    b[40..44].copy_from_slice(&w.to_le_bytes());
    b[44..48].copy_from_slice(&h.to_le_bytes());
    out.extend_from_slice(&b);
    let _ = Write::flush(&mut std::io::sink());
}

fn write_strh(out: &mut Vec<u8>, w: u32, h: u32, fps: u32, frames: u32, buf: u32) {
    out.extend_from_slice(b"strh");
    out.extend_from_slice(&56u32.to_le_bytes());
    let mut b = vec![0u8; 56];
    b[0..4].copy_from_slice(b"vids");
    b[4..8].copy_from_slice(b"MJPG");
    b[20..24].copy_from_slice(&1u32.to_le_bytes());
    b[24..28].copy_from_slice(&fps.max(1).to_le_bytes());
    b[32..36].copy_from_slice(&frames.to_le_bytes());
    b[36..40].copy_from_slice(&buf.to_le_bytes());
    b[40..44].copy_from_slice(&0xFFFF_FFFFu32.to_le_bytes());
    b[48..50].copy_from_slice(&0u16.to_le_bytes());
    b[50..52].copy_from_slice(&0u16.to_le_bytes());
    b[52..54].copy_from_slice(&(w as u16).to_le_bytes());
    b[54..56].copy_from_slice(&(h as u16).to_le_bytes());
    out.extend_from_slice(&b);
}

fn write_strf(out: &mut Vec<u8>, w: u32, h: u32) {
    out.extend_from_slice(b"strf");
    out.extend_from_slice(&40u32.to_le_bytes());
    let mut b = vec![0u8; 40];
    b[0..4].copy_from_slice(&40u32.to_le_bytes());
    b[4..8].copy_from_slice(&w.to_le_bytes());
    b[8..12].copy_from_slice(&h.to_le_bytes());
    b[12..14].copy_from_slice(&1u16.to_le_bytes());
    b[14..16].copy_from_slice(&24u16.to_le_bytes());
    b[16..20].copy_from_slice(b"MJPG");
    let bi_size_image = w.saturating_mul(h).saturating_mul(3);
    b[20..24].copy_from_slice(&bi_size_image.to_le_bytes());
    out.extend_from_slice(&b);
}
