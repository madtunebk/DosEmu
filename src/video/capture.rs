use serde_json::json;
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::thread;
use std::time::{Duration, Instant};

use super::{Capture, Framebuffer};
use crate::session::SharedQmp;

/// Fallback when the D-Bus display listener can't register: polls QMP `screendump`
/// like scripts/viewer.py.
pub struct ScreendumpCapture;

impl ScreendumpCapture {
    pub fn start(qmp: SharedQmp, fps: u32) -> Capture {
        let capture = Capture::new("screendump");
        // Separate from the viewer's /dev/shm/qemu-dos-frame.ppm so the two don't race.
        let dump_path = PathBuf::from("/dev/shm/qemu-dos-bridge-frame.ppm");
        let interval = Duration::from_secs(1) / fps.max(1);

        let (fb, count) = (capture.framebuffer.clone(), capture.updates.clone());
        thread::spawn(move || {
            let args = json!({ "filename": dump_path.to_string_lossy() });
            loop {
                let tick = Instant::now();
                // Lock only for the command so keyboard input can interleave.
                let result = qmp.lock().unwrap().execute("screendump", Some(args.clone()));
                if let Err(err) = result {
                    eprintln!("video: screendump failed, capture stopped: {err}");
                    break;
                }
                match std::fs::read(&dump_path)
                    .map_err(Into::into)
                    .and_then(|data| Framebuffer::parse_ppm(&data))
                {
                    Ok((width, height, rgb)) => {
                        fb.lock().unwrap().update(width, height, rgb);
                        count.fetch_add(1, Ordering::Relaxed);
                    }
                    Err(err) => eprintln!("video: bad frame: {err}"),
                }
                thread::sleep(interval.saturating_sub(tick.elapsed()));
            }
            let _ = std::fs::remove_file(&dump_path);
        });

        capture
    }
}
