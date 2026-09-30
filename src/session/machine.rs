use std::error::Error;
use std::io::BufRead;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

pub struct Machine {
    pub child: Child,
    dbus: Child,
    /// For scripts/viewer.py and other debug clients.
    pub qmp_socket: PathBuf,
    /// Kept open by the Rust bridge; QMP serves one client per socket.
    pub bridge_socket: PathBuf,
}

impl Machine {
    pub fn start(image_path: &Path, extra_disk: Option<&Path>) -> Result<Self, Box<dyn Error>> {
        let runtime_dir = std::env::var("XDG_RUNTIME_DIR").unwrap_or_else(|_| "/tmp".to_string());
        let uid = std::env::var("UID").unwrap_or_else(|_| {
            let output = std::process::Command::new("id").arg("-u").output();
            match output {
                Ok(result) if result.status.success() => {
                    String::from_utf8_lossy(&result.stdout).trim().to_string()
                }
                _ => std::process::id().to_string(),
            }
        });

        let socket_dir = PathBuf::from(runtime_dir.clone()).join(format!("qemu-dos-{}", uid));
        std::fs::create_dir_all(&socket_dir)?;
        let _ = std::fs::set_permissions(&socket_dir, std::fs::Permissions::from_mode(0o700));

        let qmp_socket = socket_dir.join("qmp.sock");
        let bridge_socket = socket_dir.join("qmp-bridge.sock");
        let dbus_address_file = socket_dir.join("dbus-address");
        for socket in [&qmp_socket, &bridge_socket] {
            if socket.exists() {
                std::fs::remove_file(socket)?;
            }
        }
        if dbus_address_file.exists() {
            std::fs::remove_file(&dbus_address_file)?;
        }

        let image = image_path.canonicalize().unwrap_or_else(|_| image_path.to_path_buf());
        let qemu_bin = std::env::var("QEMU_BIN").unwrap_or_else(|_| "qemu-system-i386".to_string());
        let extra_disk = extra_disk.map(|disk| disk.canonicalize().unwrap_or_else(|_| disk.to_path_buf()));
        let drive = drive_args(&image, extra_disk.as_deref())?;

        // --nofork keeps the daemon as our child so we can kill it when QEMU exits.
        let mut dbus = Command::new("dbus-daemon")
            .arg("--session")
            .arg("--nofork")
            .arg("--print-address")
            .stdout(Stdio::piped())
            .spawn()?;
        let mut dbus_address = String::new();
        std::io::BufReader::new(dbus.stdout.take().ok_or("dbus-daemon has no stdout")?)
            .read_line(&mut dbus_address)?;
        let dbus_address = dbus_address.trim().to_string();
        if dbus_address.is_empty() {
            let _ = dbus.kill();
            return Err("dbus-daemon did not print an address".into());
        }
        std::fs::write(&dbus_address_file, format!("{dbus_address}\n"))?;
        std::fs::set_permissions(&dbus_address_file, std::fs::Permissions::from_mode(0o600))?;

        let qemu_dbus_address = dbus_address
            .split(",guid=")
            .next()
            .unwrap_or(&dbus_address)
            .to_string();

        let child = Command::new(&qemu_bin)
            .arg("-name")
            .arg("Coaba DOS Lab")
            .arg("-machine")
            .arg("pc")
            .arg("-accel")
            .arg("tcg")
            .arg("-cpu")
            .arg("486")
            .arg("-m")
            .arg("32M")
            .args(&drive)
            .arg("-display")
            .arg(format!("dbus,addr={qemu_dbus_address}"))
            .arg("-qmp")
            .arg(format!("unix:{},server=on,wait=off", qmp_socket.display()))
            .arg("-qmp")
            .arg(format!("unix:{},server=on,wait=off", bridge_socket.display()))
            .arg("-nic")
            .arg("none")
            .arg("-rtc")
            .arg("base=localtime")
            .arg("-no-reboot")
            .env("XDG_RUNTIME_DIR", &runtime_dir)
            .env("DBUS_SESSION_BUS_ADDRESS", &dbus_address)
            .spawn();
        let child = match child {
            Ok(child) => child,
            Err(err) => {
                let _ = dbus.kill();
                let _ = dbus.wait();
                return Err(format!("failed to start {qemu_bin}: {err}").into());
            }
        };

        // Construct first so Drop cleans up both processes if the socket never appears.
        let mut machine = Self { child, dbus, qmp_socket, bridge_socket };
        machine.wait_until_socket_ready(Duration::from_secs(25))?;
        Ok(machine)
    }

    fn wait_until_socket_ready(&mut self, timeout: Duration) -> Result<(), Box<dyn Error>> {
        let socket_path = self.qmp_socket.as_path();
        let start = Instant::now();
        loop {
            if let Some(status) = self.child.try_wait()? {
                return Err(format!("QEMU exited during startup: {status}").into());
            }
            match std::os::unix::net::UnixStream::connect(socket_path) {
                Ok(stream) => {
                    stream.set_read_timeout(Some(Duration::from_secs(2)))?;
                    let mut reader = std::io::BufReader::new(stream.try_clone()?);
                    let mut line = String::new();
                    match reader.read_line(&mut line) {
                        Ok(_) if !line.trim().is_empty() => return Ok(()),
                        Ok(_) => {}
                        Err(_) => {}
                    }
                }
                Err(_) => {}
            }

            if start.elapsed() >= timeout {
                return Err(format!("QMP socket did not become ready at {}", socket_path.display()).into());
            }
            thread::sleep(Duration::from_millis(150));
        }
    }

    pub fn wait_for_exit(&mut self) -> Result<(), Box<dyn Error>> {
        let status = self.child.wait()?;
        println!("QEMU exited with status: {status}");
        Ok(())
    }
}

/// Standard PC floppy image sizes: 360K, 720K, 1.2M, 1.44M, 2.88M.
const FLOPPY_SIZES: [u64; 5] = [368_640, 737_280, 1_228_800, 1_474_560, 2_949_120];

pub fn is_floppy_image(path: &Path) -> bool {
    std::fs::metadata(path).is_ok_and(|meta| meta.is_file() && FLOPPY_SIZES.contains(&meta.len()))
}

/// File names of the floppy images in `dir`, sorted, for the disk picker.
pub fn list_floppies(dir: &Path) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut names: Vec<String> = entries
        .flatten()
        .filter(|entry| is_floppy_image(&entry.path()))
        .filter_map(|entry| entry.file_name().into_string().ok())
        .collect();
    names.sort();
    names
}

/// Floppy boot sectors print "Error!" when attached as a hard disk, so pick the
/// bus from the image size. BOOT_MODE=hdd|floppy overrides, as in start.sh.
/// `extra_disk` is a blank or existing raw hard disk (e.g. to install DOS onto):
/// C: when booting a floppy, D: otherwise.
fn drive_args(image: &Path, extra_disk: Option<&Path>) -> Result<Vec<String>, Box<dyn Error>> {
    let mode = match std::env::var("BOOT_MODE") {
        Ok(mode) => mode,
        Err(_) => if is_floppy_image(image) { "floppy" } else { "hdd" }.to_string(),
    };
    let file = image.display();
    let (drive, boot, disk_index) = match mode.as_str() {
        "hdd" => (format!("file={file},format=raw,if=ide,index=0,media=disk"), "c", 1),
        "floppy" => (format!("file={file},format=raw,if=floppy,index=0"), "a", 0),
        other => return Err(format!("BOOT_MODE must be 'hdd' or 'floppy', got '{other}'").into()),
    };
    println!("Boot mode: {mode}");
    let mut args = vec!["-drive".into(), drive, "-boot".into(), boot.into()];
    if mode == "hdd" {
        // Empty drive A: so floppies can be inserted later (`!disk`, web disk picker).
        args.push("-drive".into());
        args.push("if=floppy,index=0".into());
    }
    if let Some(disk) = extra_disk {
        if !disk.is_file() {
            return Err(format!("extra disk image not found: {}", disk.display()).into());
        }
        println!("Extra disk: {} (IDE index {disk_index})", disk.display());
        args.push("-drive".into());
        args.push(format!("file={},format=raw,if=ide,index={disk_index},media=disk", disk.display()));
    }
    Ok(args)
}

impl Drop for Machine {
    fn drop(&mut self) {
        if self.child.try_wait().ok().flatten().is_none() {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
        let _ = self.dbus.kill();
        let _ = self.dbus.wait();
    }
}
