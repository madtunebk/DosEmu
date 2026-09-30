mod audio;
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
use video::ScreendumpCapture;

const CAPTURE_FPS: u32 = 10;
const DEFAULT_WEB_ADDR: &str = "127.0.0.1:3000";

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1).map(PathBuf::from);
    let image = args.next().unwrap_or_else(|| PathBuf::from("images/FD14FULL.img"));
    // Optional raw hard disk image created by the user, e.g. `qemu-img create -f raw c.img 500M`.
    let extra_disk = args.next();

    println!("Starting DOS VM with image: {}", image.display());

    let mut machine = session::Machine::start(&image, extra_disk.as_deref())?;
    println!("QMP socket ready at: {}", machine.qmp_socket.display());

    let mut qmp = session::QmpClient::connect(&machine.bridge_socket)?;
    println!("QEMU status: {}", qmp.query_status()?);
    let qmp = Arc::new(Mutex::new(qmp));
    let input = input::spawn_input_thread(KeyboardController::new(qmp.clone()));
    let capture = ScreendumpCapture::start(qmp, CAPTURE_FPS);

    let web_addr = std::env::var("DOSLAB_WEB_ADDR").unwrap_or_else(|_| DEFAULT_WEB_ADDR.to_string());
    web::spawn_server(web_addr, capture.framebuffer.clone(), input.clone());

    println!("DOS booting. Debug viewer: python3 scripts/viewer.py");
    println!("Console: a line is typed + Enter; ':esc', ':down down ret', ':ctrl+alt+delete' tap keys;");
    println!("         '!shot [file.ppm]' saves the current frame, '!stats' shows capture stats.");

    // Debug console runs beside the VM; the process ends when QEMU exits.
    std::thread::spawn(move || run_console(input, capture));

    machine.wait_for_exit()?;
    Ok(())
}

fn run_console(input: InputSender, capture: ScreendumpCapture) {
    for line in std::io::stdin().lock().lines() {
        let Ok(line) = line else { break };
        if let Some(command) = line.strip_prefix('!') {
            if let Err(err) = run_bridge_command(&capture, command) {
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

fn run_bridge_command(capture: &ScreendumpCapture, command: &str) -> Result<(), Box<dyn Error>> {
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
            let polls = capture.polls.load(Ordering::Relaxed);
            let secs = capture.started.elapsed().as_secs_f64();
            println!(
                "{}x{}, {} changed frames, {polls} polls ({:.1}/s), last dirty: {:?}",
                fb.width, fb.height, fb.frame_number, polls as f64 / secs, fb.dirty
            );
        }
        _ => return Err(format!("unknown command '!{command}' (try !shot, !stats)").into()),
    }
    Ok(())
}
