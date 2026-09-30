mod input;
mod session;
mod video;
mod web;

use std::error::Error;
use std::io::BufRead;
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};

use input::{InputSender, KeyboardController};
use video::Capture;

/// Screendump polling rate, used when the D-Bus display listener can't register
/// or DOSLAB_VIDEO=screendump.
const CAPTURE_FPS: u32 = 10;
const DEFAULT_WEB_ADDR: &str = "127.0.0.1:3000";
/// Where `!disk <name>` and the web disk picker look for floppy images.
const DEFAULT_DISK_DIR: &str = "images";

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Usage: doslab [boot image] [extra hard disk] [cd.iso]. An .iso anywhere goes in the CD
    // drive; with only an .iso the VM boots from the CD.
    let (isos, disks): (Vec<PathBuf>, Vec<PathBuf>) = std::env::args()
        .skip(1)
        .map(PathBuf::from)
        .partition(|path| session::is_iso_name(&path.to_string_lossy()));
    let cdrom = isos.into_iter().next();
    let mut disks = disks.into_iter();
    let image = match (disks.next(), &cdrom) {
        (Some(image), _) => Some(image),
        (None, Some(_)) => None,
        (None, None) => Some(PathBuf::from("images/FD14FULL.img")),
    };
    // Optional raw hard disk image created by the user, e.g. `qemu-img create -f raw c.img 500M`.
    let extra_disk = disks.next();

    let boot = image.as_ref().or(cdrom.as_ref()).map(|path| path.display().to_string()).unwrap_or_default();
    println!("Starting DOS VM from: {boot}");

    let mut machine = session::Machine::start(image.as_deref(), extra_disk.as_deref(), cdrom.as_deref())?;
    println!("QMP socket ready at: {}", machine.qmp_socket.display());

    let mut qmp = session::QmpClient::connect(&machine.bridge_socket)?;
    println!("QEMU status: {}", qmp.query_status()?);
    let qmp = Arc::new(Mutex::new(qmp));
    let input = input::spawn_input_thread(KeyboardController::new(qmp.clone()));
    // DOSLAB_VIDEO=screendump forces the old polling, to compare when a picture looks wrong.
    let capture = if std::env::var("DOSLAB_VIDEO").is_ok_and(|v| v == "screendump") {
        video::capture::ScreendumpCapture::start(qmp.clone(), CAPTURE_FPS)
    } else {
        match video::dbus_display::start(&machine.dbus_address) {
            Ok(capture) => capture,
            Err(err) => {
                eprintln!("video: D-Bus display listener failed ({err}); polling screendump at {CAPTURE_FPS} fps");
                video::capture::ScreendumpCapture::start(qmp.clone(), CAPTURE_FPS)
            }
        }
    };
    println!("Video: {}", capture.source);
    let disk_dir = PathBuf::from(std::env::var("DOSLAB_DISK_DIR").unwrap_or_else(|_| DEFAULT_DISK_DIR.to_string()));

    let web_addr = std::env::var("DOSLAB_WEB_ADDR").unwrap_or_else(|_| DEFAULT_WEB_ADDR.to_string());
    web::spawn_server(web_addr, capture.framebuffer.clone(), input.clone(), qmp.clone(), disk_dir.clone());

    println!("DOS booting. Debug viewer: python3 scripts/viewer.py");
    println!("Console: a line is typed + Enter; ':esc', ':down down ret', ':ctrl+alt+delete' tap keys;");
    println!("         '!shot [file.ppm]' saves the current frame, '!stats' shows capture stats,");
    println!("         '!disk <image>' swaps the floppy in A: (name in {}/ or a path), '!eject' empties it,", disk_dir.display());
    println!("         '!cd <image.iso>' inserts a CD, '!eject cd' empties the CD drive.");

    // Debug console runs beside the VM; the process ends when QEMU exits.
    let bridge = Bridge { capture, qmp, disk_dir };
    std::thread::spawn(move || run_console(input, bridge));

    machine.wait_for_exit()?;
    Ok(())
}

/// What the `!` console commands act on.
struct Bridge {
    capture: Capture,
    qmp: session::SharedQmp,
    disk_dir: PathBuf,
}

fn run_console(input: InputSender, bridge: Bridge) {
    for line in std::io::stdin().lock().lines() {
        let Ok(line) = line else { break };
        if let Some(command) = line.strip_prefix('!') {
            if let Err(err) = run_bridge_command(&bridge, command) {
                eprintln!("error: {err}");
            }
            continue;
        }
        // Queued behind any web key events so the two sources never interleave mid-line.
        let _ = input.send(Box::new(move |keyboard| {
            if let Err(err) = type_console_line(keyboard, &line) {
                eprintln!("input error: {err}");
            }
        }));
    }
}

fn type_console_line(keyboard: &mut KeyboardController, line: &str) -> Result<(), Box<dyn Error>> {
    if let Some(keys) = line.strip_prefix(':') {
        for combo in keys.split_whitespace() {
            let mut parts: Vec<&str> = combo.split('+').collect();
            let key = parts.pop().unwrap_or_default();
            keyboard.tap_with(&parts, key)?;
        }
        return Ok(());
    }

    let skipped = keyboard.type_text(line)?;
    if !skipped.is_empty() {
        eprintln!("skipped unmapped characters: {skipped:?}");
    }
    keyboard.tap("ret")
}

fn run_bridge_command(bridge: &Bridge, command: &str) -> Result<(), Box<dyn Error>> {
    let capture = &bridge.capture;
    let mut args = command.split_whitespace();
    match args.next() {
        Some("shot") => {
            let path = args.next().unwrap_or("shot.ppm");
            let fb = capture.framebuffer.lock().unwrap().clone();
            if fb.rgb.is_empty() {
                return Err("no frame captured yet".into());
            }
            fb.save_ppm(Path::new(path))?;
            println!("saved {}x{} frame #{} to {path}", fb.width, fb.height, fb.frame_number);
        }
        Some("stats") => {
            let fb = capture.framebuffer.lock().unwrap();
            let updates = capture.updates.load(Ordering::Relaxed);
            let secs = capture.started.elapsed().as_secs_f64();
            println!(
                "{}x{} via {}, {} changed frames, {updates} updates ({:.1}/s), last dirty: {:?}",
                fb.width, fb.height, capture.source, fb.frame_number, updates as f64 / secs, fb.dirty
            );
        }
        Some(kind @ ("disk" | "cd")) => {
            let (drive, list) = if kind == "cd" {
                (session::Drive::Cdrom, session::list_isos(&bridge.disk_dir))
            } else {
                (session::Drive::Floppy, session::list_floppies(&bridge.disk_dir))
            };
            let Some(name) = args.next() else {
                println!("{} images in {}: {}", drive.label(), bridge.disk_dir.display(), list.join(" "));
                return Ok(());
            };
            let in_dir = bridge.disk_dir.join(name);
            let path = if in_dir.is_file() { in_dir } else { PathBuf::from(name) };
            let path = path.canonicalize().map_err(|err| format!("{name}: {err}"))?;
            bridge.qmp.lock().unwrap().change_medium(drive, &path)?;
            println!("{} now holds {}", drive.label(), path.display());
        }
        Some("eject") => {
            let drive = match args.next() {
                None | Some("a" | "a:" | "A:") => session::Drive::Floppy,
                Some("cd") => session::Drive::Cdrom,
                Some(other) => return Err(format!("eject what? '{other}' (try !eject or !eject cd)").into()),
            };
            bridge.qmp.lock().unwrap().eject(drive)?;
            println!("{} is empty", drive.label());
        }
        _ => return Err(format!("unknown command '!{command}' (try !shot, !stats, !disk, !cd, !eject)").into()),
    }
    Ok(())
}
