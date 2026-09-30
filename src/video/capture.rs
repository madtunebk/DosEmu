use serde_json::json;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use super::Framebuffer;
use crate::session::SharedQmp;

/// MVP stopgap: polls QMP `screendump` like scripts/viewer.py.
/// To be replaced by QEMU Display1 `ScanoutMap`/`UpdateMap` over D-Bus.
pub struct ScreendumpCapture {
    pub framebuffer: Arc<Mutex<Framebuffer>>,
    /// Screendumps taken, changed or not.
    pub polls: Arc<AtomicU64>,
    pub started: Instant,
}

impl ScreendumpCapture {
    pub fn start(qmp: SharedQmp, fps: u32) -> Self {
        let framebuffer = Arc::new(Mutex::new(Framebuffer::new()));
        let polls = Arc::new(AtomicU64::new(0));
        // Separate from the viewer's /dev/shm/qemu-dos-frame.ppm so the two don't race.
        let dump_path = PathBuf::from("/dev/shm/qemu-dos-bridge-frame.ppm");
        let interval = Duration::from_secs(1) / fps.max(1);

        let (fb, count) = (framebuffer.clone(), polls.clone());
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

        Self { framebuffer, polls, started: Instant::now() }
    }
}
