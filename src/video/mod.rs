pub mod capture;
pub mod dbus_display;
pub mod framebuffer;
pub mod jpeg;
pub mod stream;

use std::sync::atomic::AtomicU64;
use std::sync::{Arc, Mutex};
use std::time::Instant;

pub use framebuffer::Framebuffer;
pub use stream::VideoStream;

/// The live guest screen, whichever way it is captured.
pub struct Capture {
    pub framebuffer: Arc<Mutex<Framebuffer>>,
    /// Screen updates received: D-Bus scanouts/updates, or screendumps taken.
    pub updates: Arc<AtomicU64>,
    pub started: Instant,
    /// "dbus" or "screendump", for `!stats`.
    pub source: &'static str,
}

impl Capture {
    fn new(source: &'static str) -> Self {
        Self {
            framebuffer: Arc::new(Mutex::new(Framebuffer::new())),
            updates: Arc::new(AtomicU64::new(0)),
            started: Instant::now(),
            source,
        }
    }
}
