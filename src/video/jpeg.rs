use jpeg_encoder::{ColorType, Encoder};
use std::error::Error;

/// Encode an RGB24 frame as JPEG in memory.
pub fn encode_rgb(width: u32, height: u32, rgb: &[u8], quality: u8) -> Result<Vec<u8>, Box<dyn Error + Send + Sync>> {
    let width = u16::try_from(width)?;
    let height = u16::try_from(height)?;
    let mut out = Vec::with_capacity(rgb.len() / 8);
    Encoder::new(&mut out, quality).encode(rgb, width, height, ColorType::Rgb)?;
    Ok(out)
}
