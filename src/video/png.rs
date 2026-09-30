use std::error::Error;

/// Encode an RGB24 area as an indexed (palette) PNG in memory: lossless and small for DOS
/// screens, which never show more than 256 colours at once (16 in text mode).
/// None if the area has more than 256 colours.
pub fn encode_indexed(width: u32, height: u32, rgb: &[u8]) -> Option<Result<Vec<u8>, Box<dyn Error + Send + Sync>>> {
    let (palette, indices) = index_colors(rgb)?;
    Some(write_png(width, height, &palette, &indices))
}

/// Split packed RGB24 into a palette (flat RGB, at most 256 entries) and one index per pixel.
fn index_colors(rgb: &[u8]) -> Option<(Vec<u8>, Vec<u8>)> {
    let mut palette = Vec::with_capacity(16 * 3);
    let mut table = ColorTable::new();
    let mut indices = Vec::with_capacity(rgb.len() / 3);
    // Neighbouring pixels are usually the same colour; skip the lookup for runs.
    let mut last: Option<(u32, u8)> = None;
    for px in rgb.chunks_exact(3) {
        let color = u32::from(px[0]) << 16 | u32::from(px[1]) << 8 | u32::from(px[2]);
        let index = match last {
            Some((last_color, index)) if last_color == color => index,
            _ => table.index_of(color, || {
                let index = u8::try_from(palette.len() / 3).ok()?;
                palette.extend_from_slice(&px[..3]);
                Some(index)
            })?,
        };
        last = Some((color, index));
        indices.push(index);
    }
    Some((palette, indices))
}

/// Colour -> palette index for at most 256 colours: open addressing in a fixed table four times
/// that size with a multiplicative hash. Replaces a HashMap whose SipHash was ~40% of streaming.
struct ColorTable {
    /// color | OCCUPIED, 0 = empty.
    keys: [u32; ColorTable::SIZE],
    values: [u8; ColorTable::SIZE],
}

impl ColorTable {
    const SIZE: usize = 1024;
    const OCCUPIED: u32 = 1 << 24;

    fn new() -> Self {
        Self { keys: [0; Self::SIZE], values: [0; Self::SIZE] }
    }

    /// The colour's index, adding it with `add` if new. None if `add` refuses (palette full).
    fn index_of(&mut self, color: u32, add: impl FnOnce() -> Option<u8>) -> Option<u8> {
        let key = color | Self::OCCUPIED;
        let mut slot = (color.wrapping_mul(0x9E37_79B1) >> 22) as usize;
        loop {
            match self.keys[slot] {
                k if k == key => return Some(self.values[slot]),
                0 => {
                    let index = add()?;
                    self.keys[slot] = key;
                    self.values[slot] = index;
                    return Some(index);
                }
                _ => slot = (slot + 1) % Self::SIZE,
            }
        }
    }
}

fn write_png(width: u32, height: u32, palette: &[u8], indices: &[u8]) -> Result<Vec<u8>, Box<dyn Error + Send + Sync>> {
    let mut out = Vec::with_capacity(indices.len() / 4);
    let mut encoder = png::Encoder::new(&mut out, width, height);
    encoder.set_color(png::ColorType::Indexed);
    encoder.set_depth(png::BitDepth::Eight);
    encoder.set_palette(palette.to_vec());
    // Measured on captured text and mode 13h frames: Up + Default makes a full screen 4-5 KB
    // (JPEG: 42-73 KB) in ~2-3 ms; Compression::Fast is quicker but 10x bigger. The Up filter
    // turns QEMU's doubled rows (320x200 is shown as 640x400) into zeros.
    encoder.set_compression(png::Compression::Default);
    encoder.set_filter(png::FilterType::Up);
    let mut writer = encoder.write_header()?;
    writer.write_image_data(indices)?;
    writer.finish()?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::{encode_indexed, index_colors};

    #[test]
    fn indexes_colors_in_order_of_appearance() {
        let rgb = [9, 9, 9, 1, 2, 3, 9, 9, 9, 1, 2, 3];
        let (palette, indices) = index_colors(&rgb).unwrap();
        assert_eq!(palette, [9, 9, 9, 1, 2, 3]);
        assert_eq!(indices, [0, 1, 0, 1]);
    }

    #[test]
    fn more_than_256_colors_is_refused() {
        let rgb: Vec<u8> = (0..257u32).flat_map(|i| [(i % 256) as u8, (i / 256) as u8, 0]).collect();
        assert!(index_colors(&rgb).is_none());
        assert!(encode_indexed(257, 1, &rgb).is_none());
    }

    #[test]
    fn writes_a_png() {
        let png = encode_indexed(2, 1, &[255, 0, 0, 0, 0, 255]).unwrap().unwrap();
        assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n");
    }
}

