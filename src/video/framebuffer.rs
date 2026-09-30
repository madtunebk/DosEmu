use std::collections::VecDeque;
use std::error::Error;
use std::path::Path;

/// How many recent changes are remembered for viewers that fell a few frames behind.
const CHANGE_LOG_LEN: usize = 64;

/// Region of the screen that changed since the previous frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DirtyRect {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

impl DirtyRect {
    /// Smallest rectangle covering both.
    pub fn union(self, other: DirtyRect) -> DirtyRect {
        let (x, y) = (self.x.min(other.x), self.y.min(other.y));
        let right = (self.x + self.width).max(other.x + other.width);
        let bottom = (self.y + self.height).max(other.y + other.height);
        DirtyRect { x, y, width: right - x, height: bottom - y }
    }
}

/// Latest guest screen as packed RGB24.
#[derive(Clone, Default)]
pub struct Framebuffer {
    pub width: u32,
    pub height: u32,
    pub rgb: Vec<u8>,
    /// Increments every time the picture changes.
    pub frame_number: u64,
    /// Changed area of the latest frame; the whole screen on a resize.
    pub dirty: Option<DirtyRect>,
    /// (frame_number, changed area) of the latest changes, oldest first.
    recent: VecDeque<(u64, DirtyRect)>,
    /// frame_number of the last size change; viewers from before it need a full frame.
    resized_at: u64,
}

impl Framebuffer {
    pub fn new() -> Self {
        Self::default()
    }

    /// Replace the picture. Returns the changed area, or None if nothing changed.
    pub fn update(&mut self, width: u32, height: u32, rgb: Vec<u8>) -> Option<DirtyRect> {
        let dirty = if width != self.width || height != self.height {
            Some(DirtyRect { x: 0, y: 0, width, height })
        } else {
            diff_bounds(width, &self.rgb, &rgb)
        };
        if let Some(rect) = dirty {
            let resized = (width, height) != (self.width, self.height);
            self.width = width;
            self.height = height;
            self.rgb = rgb;
            self.changed(rect, resized);
        }
        dirty
    }

    /// Take a whole new picture, as sent on a video mode change.
    pub fn scanout(&mut self, width: u32, height: u32, rgb: Vec<u8>) {
        let resized = (width, height) != (self.width, self.height);
        self.width = width;
        self.height = height;
        self.rgb = rgb;
        self.changed(DirtyRect { x: 0, y: 0, width, height }, resized);
    }

    fn changed(&mut self, rect: DirtyRect, resized: bool) {
        self.frame_number += 1;
        self.dirty = Some(rect);
        if resized {
            self.resized_at = self.frame_number;
        }
        if self.recent.len() == CHANGE_LOG_LEN {
            self.recent.pop_front();
        }
        self.recent.push_back((self.frame_number, rect));
    }

    /// What a viewer that last saw frame `seen` must redraw: everything changed since then,
    /// or the whole screen if the size changed or the log no longer reaches back that far.
    pub fn changed_since(&self, seen: u64) -> DirtyRect {
        let full = DirtyRect { x: 0, y: 0, width: self.width, height: self.height };
        let log_covers = self.recent.front().is_some_and(|&(first, _)| first <= seen + 1);
        if seen < self.resized_at || !log_covers {
            return full;
        }
        self.recent
            .iter()
            .filter(|&&(number, _)| number > seen)
            .map(|&(_, rect)| rect)
            .reduce(DirtyRect::union)
            .unwrap_or(full)
    }

    /// Packed RGB24 copy of one area of the picture.
    pub fn crop(&self, rect: DirtyRect) -> Vec<u8> {
        let (stride, row_len) = (self.width as usize * 3, rect.width as usize * 3);
        let mut out = Vec::with_capacity(row_len * rect.height as usize);
        for row in rect.y as usize..(rect.y + rect.height) as usize {
            let start = row * stride + rect.x as usize * 3;
            out.extend_from_slice(&self.rgb[start..start + row_len]);
        }
        out
    }

    /// Change the size, keeping the pixels that still fit and filling the rest with black.
    /// False if the size is unchanged.
    pub fn resize(&mut self, width: u32, height: u32) -> bool {
        if (width, height) == (self.width, self.height) {
            return false;
        }
        let mut rgb = vec![0; width as usize * height as usize * 3];
        let keep = self.width.min(width) as usize * 3;
        if keep > 0 {
            let (old_stride, new_stride) = (self.width as usize * 3, width as usize * 3);
            for (old_row, new_row) in self.rgb.chunks_exact(old_stride).zip(rgb.chunks_exact_mut(new_stride)) {
                new_row[..keep].copy_from_slice(&old_row[..keep]);
            }
        }
        self.scanout(width, height, rgb);
        true
    }

    /// Copy a packed RGB24 rectangle into the picture. False if it doesn't fit.
    pub fn patch(&mut self, rect: DirtyRect, rgb: &[u8]) -> bool {
        let fits = rect.x.checked_add(rect.width).is_some_and(|right| right <= self.width)
            && rect.y.checked_add(rect.height).is_some_and(|bottom| bottom <= self.height)
            && rgb.len() == rect.width as usize * rect.height as usize * 3;
        if !fits {
            return false;
        }
        let (row_len, stride) = (rect.width as usize * 3, self.width as usize * 3);
        for (row, src) in rgb.chunks_exact(row_len).enumerate() {
            let start = (rect.y as usize + row) * stride + rect.x as usize * 3;
            self.rgb[start..start + row_len].copy_from_slice(src);
        }
        self.changed(rect, false);
        true
    }

    /// Parse a binary PPM (P6, maxval 255) as written by QEMU's `screendump`.
    pub fn parse_ppm(data: &[u8]) -> Result<(u32, u32, Vec<u8>), Box<dyn Error>> {
        let mut pos = 0;
        let mut fields = [0u32; 3];
        if data.get(..2) != Some(b"P6") {
            return Err("not a binary PPM (P6)".into());
        }
        pos += 2;
        for field in &mut fields {
            // Skip whitespace and '#' comments between header fields.
            loop {
                match data.get(pos) {
                    Some(b) if b.is_ascii_whitespace() => pos += 1,
                    Some(b'#') => {
                        while data.get(pos).is_some_and(|&b| b != b'\n') {
                            pos += 1;
                        }
                    }
                    _ => break,
                }
            }
            let start = pos;
            while data.get(pos).is_some_and(u8::is_ascii_digit) {
                pos += 1;
            }
            *field = std::str::from_utf8(&data[start..pos])?.parse()?;
        }
        pos += 1; // single whitespace byte before the pixels
        let [width, height, maxval] = fields;
        if maxval != 255 {
            return Err(format!("unsupported PPM maxval {maxval}").into());
        }
        let len = width as usize * height as usize * 3;
        let pixels = data.get(pos..pos + len).ok_or("PPM pixel data is truncated")?;
        Ok((width, height, pixels.to_vec()))
    }

    pub fn save_ppm(&self, path: &Path) -> Result<(), Box<dyn Error>> {
        let mut out = format!("P6\n{} {}\n255\n", self.width, self.height).into_bytes();
        out.extend_from_slice(&self.rgb);
        std::fs::write(path, out)?;
        Ok(())
    }
}

/// Bounding box of the pixels that differ between two same-sized RGB24 frames.
fn diff_bounds(width: u32, old: &[u8], new: &[u8]) -> Option<DirtyRect> {
    let row_len = width as usize * 3;
    if row_len == 0 {
        return None;
    }
    let (mut x0, mut y0, mut x1, mut y1) = (u32::MAX, u32::MAX, 0, 0);
    for (y, (old_row, new_row)) in old.chunks(row_len).zip(new.chunks(row_len)).enumerate() {
        if old_row == new_row {
            continue;
        }
        let first = old_row.iter().zip(new_row).position(|(a, b)| a != b).unwrap_or(0);
        let last = old_row.iter().zip(new_row).rposition(|(a, b)| a != b).unwrap_or(0);
        x0 = x0.min((first / 3) as u32);
        x1 = x1.max((last / 3) as u32);
        y0 = y0.min(y as u32);
        y1 = y as u32;
    }
    (y0 != u32::MAX).then(|| DirtyRect { x: x0, y: y0, width: x1 - x0 + 1, height: y1 - y0 + 1 })
}

#[cfg(test)]
mod tests {
    use super::{DirtyRect, Framebuffer};

    #[test]
    fn resize_keeps_overlap_and_patch_checks_bounds() {
        let mut fb = Framebuffer::new();
        fb.scanout(2, 1, vec![1, 1, 1, 2, 2, 2]);
        assert!(fb.resize(3, 2));
        assert_eq!(fb.rgb, [1, 1, 1, 2, 2, 2, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
        assert!(!fb.patch(DirtyRect { x: 2, y: 1, width: 2, height: 1 }, &[9; 6]));
        assert!(fb.patch(DirtyRect { x: 2, y: 1, width: 1, height: 1 }, &[9; 3]));
        assert_eq!(&fb.rgb[15..], [9, 9, 9]);
    }

    #[test]
    fn changed_since_merges_recent_changes() {
        let mut fb = Framebuffer::new();
        fb.scanout(4, 4, vec![0; 48]);
        let seen = fb.frame_number;
        assert!(fb.patch(DirtyRect { x: 0, y: 0, width: 1, height: 1 }, &[1; 3]));
        assert!(fb.patch(DirtyRect { x: 2, y: 3, width: 1, height: 1 }, &[2; 3]));
        assert_eq!(fb.changed_since(seen), DirtyRect { x: 0, y: 0, width: 3, height: 4 });
        assert_eq!(fb.changed_since(fb.frame_number - 1), DirtyRect { x: 2, y: 3, width: 1, height: 1 });
        assert_eq!(fb.crop(DirtyRect { x: 2, y: 3, width: 2, height: 1 }), [2, 2, 2, 0, 0, 0]);
        // A new viewer (or one from before a resize) gets the whole screen.
        assert_eq!(fb.changed_since(0), DirtyRect { x: 0, y: 0, width: 4, height: 4 });
        fb.resize(5, 4);
        assert_eq!(fb.changed_since(seen + 2), DirtyRect { x: 0, y: 0, width: 5, height: 4 });
    }
}
