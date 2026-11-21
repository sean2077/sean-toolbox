//! PNG ↔ Win32 DIB conversion, kept platform-independent so it is unit-tested
//! on every host.

use image::ImageFormat;
use std::io::Cursor;

pub(crate) fn png_to_dib(png: &[u8]) -> Option<Vec<u8>> {
    let image = image::load_from_memory_with_format(png, ImageFormat::Png)
        .ok()?
        .to_rgba8();
    let width = image.width() as i32;
    let height = image.height() as i32;
    if width == 0 || height == 0 {
        return None;
    }
    let mut dib = Vec::with_capacity(40 + (width as usize * height as usize * 4));
    dib.extend_from_slice(&40u32.to_le_bytes());
    dib.extend_from_slice(&width.to_le_bytes());
    dib.extend_from_slice(&height.to_le_bytes());
    dib.extend_from_slice(&1u16.to_le_bytes());
    dib.extend_from_slice(&32u16.to_le_bytes());
    dib.extend_from_slice(&0u32.to_le_bytes());
    dib.extend_from_slice(&((width * height * 4) as u32).to_le_bytes());
    dib.extend_from_slice(&0i32.to_le_bytes());
    dib.extend_from_slice(&0i32.to_le_bytes());
    dib.extend_from_slice(&0u32.to_le_bytes());
    dib.extend_from_slice(&0u32.to_le_bytes());
    for y in (0..height as u32).rev() {
        for x in 0..width as u32 {
            let pixel = image.get_pixel(x, y);
            dib.extend_from_slice(&[pixel[2], pixel[1], pixel[0], pixel[3]]);
        }
    }
    Some(dib)
}

pub(crate) fn dib_to_png(dib: &[u8]) -> Option<Vec<u8>> {
    if dib.len() < 40 {
        return None;
    }
    let header_size = u32::from_le_bytes(dib[0..4].try_into().ok()?) as usize;
    if header_size < 40 || header_size > dib.len() {
        return None;
    }
    let bit_count = u16::from_le_bytes(dib[14..16].try_into().ok()?);
    let colors_used = u32::from_le_bytes(dib[32..36].try_into().ok()?) as usize;
    let palette = if bit_count <= 8 {
        if colors_used > 0 {
            colors_used
        } else {
            1usize << bit_count
        }
    } else {
        0
    };
    let offset = 14 + header_size + palette * 4;
    let mut bmp = Vec::with_capacity(14 + dib.len());
    bmp.extend_from_slice(&0x4D42u16.to_le_bytes());
    bmp.extend_from_slice(&((14 + dib.len()) as u32).to_le_bytes());
    bmp.extend_from_slice(&[0, 0, 0, 0]);
    bmp.extend_from_slice(&(offset as u32).to_le_bytes());
    bmp.extend_from_slice(dib);
    let image = image::load_from_memory_with_format(&bmp, ImageFormat::Bmp).ok()?;
    let mut output = Cursor::new(Vec::new());
    image.write_to(&mut output, ImageFormat::Png).ok()?;
    Some(output.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{ImageBuffer, Rgba};

    fn png() -> Vec<u8> {
        let image: ImageBuffer<Rgba<u8>, Vec<u8>> =
            ImageBuffer::from_pixel(2, 2, Rgba([10, 20, 30, 255]));
        let mut output = Cursor::new(Vec::new());
        image.write_to(&mut output, ImageFormat::Png).unwrap();
        output.into_inner()
    }

    #[test]
    fn png_dib_round_trip_keeps_dimensions_and_colors() {
        let png = png();
        let dib = png_to_dib(&png).unwrap();
        assert_eq!(u32::from_le_bytes(dib[0..4].try_into().unwrap()), 40);
        let decoded = dib_to_png(&dib).unwrap();
        let image = image::load_from_memory_with_format(&decoded, ImageFormat::Png).unwrap();
        assert_eq!(image.width(), 2);
        assert_eq!(image.height(), 2);
        assert_eq!(image.to_rgba8().get_pixel(0, 0).0, [10, 20, 30, 255]);
    }

    #[test]
    fn malformed_dib_is_rejected() {
        assert!(dib_to_png(b"short").is_none());
    }
}
