use std::collections::HashMap;
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
    let mut lookup: HashMap<[u8; 3], u8> = HashMap::new();
    let mut indices = Vec::with_capacity(rgb.len() / 3);
    // Neighbouring pixels are usually the same colour; skip the map lookup for runs.
    let mut last: Option<([u8; 3], u8)> = None;
    for px in rgb.chunks_exact(3) {
        let color = [px[0], px[1], px[2]];
        let index = match last {
            Some((last_color, index)) if last_color == color => index,
            _ => match lookup.get(&color) {
                Some(&index) => index,
                None => {
                    let index = u8::try_from(lookup.len()).ok()?;
                    lookup.insert(color, index);
                    palette.extend_from_slice(&color);
                    index
                }
            },
        };
        last = Some((color, index));
        indices.push(index);
    }
    Some((palette, indices))
}

fn write_png(width: u32, height: u32, palette: &[u8], indices: &[u8]) -> Result<Vec<u8>, Box<dyn Error + Send + Sync>> {
    let mut out = Vec::with_capacity(indices.len() / 4);
    let mut encoder = png::Encoder::new(&mut out, width, height);
    encoder.set_color(png::ColorType::Indexed);
    encoder.set_depth(png::BitDepth::Eight);
    encoder.set_palette(palette.to_vec());
    // Speed over size: this runs up to ~30 times a second per client.
    encoder.set_compression(png::Compression::Fast);
    encoder.set_filter(png::FilterType::NoFilter);
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
