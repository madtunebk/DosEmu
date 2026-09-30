use std::error::Error;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use zbus::export::futures_core::Stream;

use super::framebuffer::DirtyRect;
use super::{Capture, Framebuffer};
use crate::session::dbus_listener;

const CONSOLE_PATH: &str = "/org/qemu/Display1/Console_0";
const CONSOLE_INTERFACE: &str = "org.qemu.Display1.Console";
const LISTENER_PATH: &str = "/org/qemu/Display1/Listener";

/// Receives the guest screen from QEMU's `-display dbus` backend: QEMU pushes a full
/// `Scanout` on mode changes and an `Update` per changed rectangle, so nothing is polled.
pub fn start(dbus_address: &str) -> Result<Capture, Box<dyn Error>> {
    let capture = Capture::new("dbus");
    let (framebuffer, updates) = (capture.framebuffer.clone(), capture.updates.clone());
    let address = dbus_address.to_string();
    dbus_listener::run_on_own_thread("display", move || register(address, framebuffer, updates))?;
    Ok(capture)
}

async fn register(
    address: String,
    framebuffer: Arc<Mutex<Framebuffer>>,
    updates: Arc<AtomicU64>,
) -> zbus::Result<(zbus::Connection, zbus::Connection)> {
    let bus = zbus::connection::Builder::address(address.as_str())?.build().await?;
    let console = zbus::fdo::PropertiesProxy::builder(&bus)
        .destination("org.qemu")?
        .path(CONSOLE_PATH)?
        .cache_properties(zbus::proxy::CacheProperties::No)
        .build()
        .await?;
    let listener = Listener { framebuffer, updates, console };
    let peer =
        dbus_listener::register_p2p(&bus, CONSOLE_PATH, CONSOLE_INTERFACE, "RegisterListener", LISTENER_PATH, listener)
            .await?;
    tokio::spawn(watch_mode_changes(peer.clone()));
    Ok((bus, peer))
}

/// QEMU announces a new video mode by changing the console's Width/Height, not always with a
/// Scanout; a shrink would otherwise go unnoticed because its updates still fit the old size.
async fn watch_mode_changes(peer: zbus::Connection) {
    let result: zbus::Result<()> = async {
        let listener = peer.object_server().interface::<_, Listener>(LISTENER_PATH).await?;
        let changes = listener.get().await.console.receive_properties_changed().await?;
        let mut changes = std::pin::pin!(changes);
        while let Some(change) = std::future::poll_fn(|cx| changes.as_mut().poll_next(cx)).await {
            let args = change.args()?;
            if args.interface_name().as_str() == CONSOLE_INTERFACE {
                listener.get().await.sync_size().await;
            }
        }
        Ok(())
    }
    .await;
    if let Err(err) = result {
        eprintln!("video: not watching for mode changes: {err}");
    }
}

struct Listener {
    framebuffer: Arc<Mutex<Framebuffer>>,
    updates: Arc<AtomicU64>,
    /// Properties of QEMU's console on the bus, for the current screen size.
    console: zbus::fdo::PropertiesProxy<'static>,
}

impl Listener {
    /// Resize the picture to the console's current mode. True if the size changed.
    async fn sync_size(&self) -> bool {
        let interface = zbus::names::InterfaceName::from_static_str_unchecked(CONSOLE_INTERFACE);
        let size = async {
            let width = u32::try_from(self.console.get(interface.clone(), "Width").await?)?;
            let height = u32::try_from(self.console.get(interface, "Height").await?)?;
            Ok::<_, zbus::Error>((width, height))
        };
        match size.await {
            Ok((width, height)) => self.framebuffer.lock().unwrap().resize(width, height),
            Err(err) => {
                eprintln!("video: cannot read the console size: {err}");
                false
            }
        }
    }
}

/// Method names and signatures follow QEMU's org.qemu.Display1.Listener (ui/dbus-display1.xml).
#[zbus::interface(name = "org.qemu.Display1.Listener")]
impl Listener {
    async fn scanout(&self, width: u32, height: u32, stride: u32, pixman_format: u32, data: Vec<u8>) {
        match to_rgb(pixman_format, stride, width, height, &data) {
            Ok(rgb) => {
                self.framebuffer.lock().unwrap().scanout(width, height, rgb);
                self.updates.fetch_add(1, Ordering::Relaxed);
            }
            Err(err) => eprintln!("video: scanout {width}x{height}: {err}"),
        }
    }

    #[allow(clippy::too_many_arguments)]
    async fn update(&self, x: i32, y: i32, width: i32, height: i32, stride: u32, pixman_format: u32, data: Vec<u8>) {
        let (Ok(x), Ok(y), Ok(width), Ok(height)) = (u32::try_from(x), u32::try_from(y), u32::try_from(width), u32::try_from(height)) else {
            return eprintln!("video: bad update rect {x},{y} {width}x{height}");
        };
        let rgb = match to_rgb(pixman_format, stride, width, height, &data) {
            Ok(rgb) => rgb,
            Err(err) => return eprintln!("video: update {width}x{height}: {err}"),
        };
        let rect = DirtyRect { x, y, width, height };
        // An update that doesn't fit means the mode changed. QEMU can send the new mode's first
        // rows before the console reports its size, so grow to cover them until it does;
        // watch_mode_changes then sets the exact size, keeping these pixels.
        let mut fits = self.framebuffer.lock().unwrap().patch(rect, &rgb);
        if !fits {
            self.sync_size().await;
            let mut fb = self.framebuffer.lock().unwrap();
            if !fb.patch(rect, &rgb) {
                let (grown_width, grown_height) = (fb.width.max(x + width), fb.height.max(y + height));
                fb.resize(grown_width, grown_height);
            }
            fits = fb.patch(rect, &rgb);
        }
        if fits {
            self.updates.fetch_add(1, Ordering::Relaxed);
        } else {
            eprintln!("video: update {x},{y} {width}x{height} is outside the screen");
        }
    }

    async fn disable(&self) {}

    async fn mouse_set(&self, _x: i32, _y: i32, _on: i32) {}

    async fn cursor_define(&self, _width: i32, _height: i32, _hot_x: i32, _hot_y: i32, _data: Vec<u8>) {}

    /// Optional extra interfaces (shared-memory Unix.Map, ...); none yet, so QEMU copies pixels.
    #[zbus(property)]
    async fn interfaces(&self) -> Vec<String> {
        Vec::new()
    }
}

/// Convert pixels in a pixman format (`bpp << 24 | type << 16 | a << 12 | r << 8 | g << 4 | b`)
/// to packed RGB24. Handles the ARGB/ABGR packed formats QEMU's VGA surfaces use.
fn to_rgb(format: u32, stride: u32, width: u32, height: u32, data: &[u8]) -> Result<Vec<u8>, String> {
    const X8R8G8B8: u32 = 0x2002_0888;
    const A8R8G8B8: u32 = 0x2002_8888;
    const TYPE_ARGB: u32 = 2;
    const TYPE_ABGR: u32 = 3;
    let bpp = format >> 24;
    let kind = (format >> 16) & 0xff;
    let (r_bits, g_bits, b_bits) = ((format >> 8) & 0xf, (format >> 4) & 0xf, format & 0xf);
    let bytes = (bpp / 8) as usize;
    if !(kind == TYPE_ARGB || kind == TYPE_ABGR) || ![2, 3, 4].contains(&bytes) || r_bits == 0 {
        return Err(format!("unsupported pixman format {format:#010x}"));
    }
    let (stride, width, height) = (stride as usize, width as usize, height as usize);
    if stride < width * bytes || data.len() < stride * height.saturating_sub(1) + width * bytes {
        return Err(format!("{} bytes is too short for stride {stride}", data.len()));
    }

    // Channel shifts from the least significant bit up.
    let (r_shift, b_shift) = if kind == TYPE_ARGB { (b_bits + g_bits, 0) } else { (0, r_bits + g_bits) };
    let g_shift = if kind == TYPE_ARGB { b_bits } else { r_bits };
    let channel = |pixel: u32, shift: u32, bits: u32| {
        let max = (1u32 << bits) - 1;
        (((pixel >> shift) & max) * 255 / max) as u8
    };

    let mut rgb = Vec::with_capacity(width * height * 3);
    if format == X8R8G8B8 || format == A8R8G8B8 {
        // The common case (32-bit VGA surfaces): little-endian B, G, R, X bytes. A full-screen
        // game frame arrives ~30 times a second, so this must stay well under 33 ms.
        for row in data.chunks(stride).take(height) {
            for px in row[..width * 4].chunks_exact(4) {
                rgb.extend_from_slice(&[px[2], px[1], px[0]]);
            }
        }
        return Ok(rgb);
    }
    for row in data.chunks(stride).take(height) {
        for px in row[..width * bytes].chunks_exact(bytes) {
            let pixel = px.iter().rev().fold(0u32, |acc, &byte| acc << 8 | byte as u32);
            rgb.extend_from_slice(&[
                channel(pixel, r_shift, r_bits),
                channel(pixel, g_shift, g_bits),
                channel(pixel, b_shift, b_bits),
            ]);
        }
    }
    Ok(rgb)
}

#[cfg(test)]
mod tests {
    use super::to_rgb;

    #[test]
    fn converts_x8r8g8b8() {
        // Little-endian BGRX bytes, one row of two pixels plus stride padding.
        let data = [0x10, 0x20, 0x30, 0, 0xff, 0x00, 0x80, 0, 9, 9, 9, 9];
        assert_eq!(to_rgb(0x2002_0888, 12, 2, 1, &data).unwrap(), [0x30, 0x20, 0x10, 0x80, 0x00, 0xff]);
    }

    #[test]
    fn converts_a8r8g8b8() {
        let data = [0x10, 0x20, 0x30, 0xff];
        assert_eq!(to_rgb(0x2002_8888, 4, 1, 1, &data).unwrap(), [0x30, 0x20, 0x10]);
    }

    #[test]
    fn converts_r5g6b5() {
        // Pure red and pure blue in 16-bit 565.
        let data = [0x00, 0xf8, 0x1f, 0x00];
        assert_eq!(to_rgb(0x1002_0565, 4, 2, 1, &data).unwrap(), [255, 0, 0, 0, 0, 255]);
    }

    #[test]
    fn rejects_short_data() {
        assert!(to_rgb(0x2002_0888, 8, 2, 2, &[0; 8]).is_err());
    }
}
