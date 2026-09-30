use std::sync::{Arc, Mutex};

use super::{Framebuffer, jpeg};

const JPEG_QUALITY: u8 = 85;

/// Per-client view of the framebuffer: yields each new frame once, as JPEG.
pub struct VideoStream {
    framebuffer: Arc<Mutex<Framebuffer>>,
    last_sent: u64,
}

impl VideoStream {
    pub fn new(framebuffer: Arc<Mutex<Framebuffer>>) -> Self {
        Self { framebuffer, last_sent: 0 }
    }

    /// The latest frame as JPEG if it changed since the last call, else None.
    /// Sends the whole frame; the dirty rect is not used yet.
    pub async fn next_jpeg(&mut self) -> Option<Vec<u8>> {
        let (number, width, height, rgb) = {
            let fb = self.framebuffer.lock().unwrap();
            if fb.frame_number == self.last_sent || fb.rgb.is_empty() {
                return None;
            }
            (fb.frame_number, fb.width, fb.height, fb.rgb.clone())
        };
        self.last_sent = number;
        tokio::task::spawn_blocking(move || jpeg::encode_rgb(width, height, &rgb, JPEG_QUALITY))
            .await
            .ok()?
            .inspect_err(|err| eprintln!("video: jpeg encode failed: {err}"))
            .ok()
    }
}
