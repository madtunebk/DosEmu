use std::sync::{Arc, Mutex};

use super::{Framebuffer, jpeg, png};

const JPEG_QUALITY: u8 = 85;

/// Per-client view of the framebuffer: yields what changed since the client's last frame.
pub struct VideoStream {
    framebuffer: Arc<Mutex<Framebuffer>>,
    last_sent: u64,
}

impl VideoStream {
    pub fn new(framebuffer: Arc<Mutex<Framebuffer>>) -> Self {
        Self { framebuffer, last_sent: 0 }
    }

    /// The next message for the client, or None if nothing changed since the last one.
    ///
    /// Only the changed area is sent: an 8-byte header of little-endian u16s (screen width,
    /// screen height, x, y of the area) followed by the area as an image. A blinking cursor
    /// costs a 9x16 image instead of the whole screen; a new client or a mode change gets it all.
    /// The image is a lossless indexed PNG (DOS never shows more than 256 colours), or JPEG if
    /// the area somehow has more.
    pub async fn next_message(&mut self) -> Option<Vec<u8>> {
        let (number, screen, rect, rgb) = {
            let fb = self.framebuffer.lock().unwrap();
            if fb.frame_number == self.last_sent || fb.rgb.is_empty() {
                return None;
            }
            let rect = fb.changed_since(self.last_sent);
            (fb.frame_number, (fb.width, fb.height), rect, fb.crop(rect))
        };
        self.last_sent = number;
        let image = tokio::task::spawn_blocking(move || {
            png::encode_indexed(rect.width, rect.height, &rgb)
                .unwrap_or_else(|| jpeg::encode_rgb(rect.width, rect.height, &rgb, JPEG_QUALITY))
        })
        .await
        .ok()?
        .inspect_err(|err| eprintln!("video: frame encode failed: {err}"))
        .ok()?;

        let mut message = Vec::with_capacity(8 + image.len());
        for value in [screen.0, screen.1, rect.x, rect.y] {
            message.extend_from_slice(&u16::try_from(value).ok()?.to_le_bytes());
        }
        message.extend_from_slice(&image);
        Some(message)
    }
}
