use std::io::Cursor;

use image::ImageFormat;

use crate::error::Result;

pub struct NormalizedPng {
    pub bytes: Vec<u8>,
    pub width: u32,
    pub height: u32,
}

/// 进入 Core 的图片统一转为 `image/png`。
pub fn normalize_png(input: &[u8]) -> Result<NormalizedPng> {
    let img = image::load_from_memory(input)?;
    let width = img.width();
    let height = img.height();
    let mut bytes = Vec::new();
    img.write_to(&mut Cursor::new(&mut bytes), ImageFormat::Png)?;
    Ok(NormalizedPng { bytes, width, height })
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{ImageBuffer, Rgb};

    #[test]
    fn jpeg_becomes_png() {
        let img: ImageBuffer<Rgb<u8>, _> = ImageBuffer::from_pixel(2, 2, Rgb([10, 20, 30]));
        let mut jpeg = Vec::new();
        img.write_to(&mut Cursor::new(&mut jpeg), ImageFormat::Jpeg).unwrap();
        let png = normalize_png(&jpeg).unwrap();
        assert_eq!((png.width, png.height), (2, 2));
        assert_eq!(&png.bytes[..8], b"\x89PNG\r\n\x1a\n");
    }
}
