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
    /// Private session bus QEMU's `-display dbus` backend is on.
    pub dbus_address: String,
}

impl Machine {
    pub fn start(image_path: Option<&Path>, extra_disk: Option<&Path>, cdrom: Option<&Path>) -> Result<Self, Box<dyn Error>> {
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

        let qemu_bin = std::env::var("QEMU_BIN").unwrap_or_else(|_| "qemu-system-i386".to_string());
        let absolute = |path: &Path| path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
        let drive = drive_args(
            image_path.map(absolute).as_deref(),
            extra_disk.map(absolute).as_deref(),
            cdrom.map(absolute).as_deref(),
        )?;

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

        let sound_cards = sound_card_args(&qemu_bin);
        let qemu = |accel: &str| {
            let mut command = Command::new(&qemu_bin);
            command
                .args(["-name", "Coaba DOS Lab", "-accel", accel, "-cpu", "486", "-m", "32M"])
                // No host audio: left to itself QEMU probes the host's sound system (PulseAudio,
                // PipeWire, ALSA...) from inside our private D-Bus session, which crashed QEMU on
                // the first beep. Sound is meant to reach the browser through the bridge instead.
                .args(["-audiodev", "none,id=snd0", "-machine", "pc,pcspk-audiodev=snd0"])
                .args(&sound_cards)
                .args(&drive)
                .arg("-display")
                .arg(format!("dbus,addr={qemu_dbus_address}"))
                .arg("-qmp")
                .arg(format!("unix:{},server=on,wait=off", qmp_socket.display()))
                .arg("-qmp")
                .arg(format!("unix:{},server=on,wait=off", bridge_socket.display()))
                .args(["-nic", "none", "-rtc", "base=localtime", "-no-reboot"])
                .env("XDG_RUNTIME_DIR", &runtime_dir)
                .env("DBUS_SESSION_BUS_ADDRESS", &dbus_address);
            command
        };

        let accels = accelerators();
        let child = match qemu(accels[0]).spawn() {
            Ok(child) => child,
            Err(err) => {
                let _ = dbus.kill();
                let _ = dbus.wait();
                return Err(format!("failed to start {qemu_bin}: {err}").into());
            }
        };

        // Construct first so Drop cleans up both processes if the socket never appears.
        let mut machine = Self {
            child,
            dbus,
            qmp_socket: qmp_socket.clone(),
            bridge_socket: bridge_socket.clone(),
            dbus_address: dbus_address.clone(),
        };
        for (attempt, accel) in accels.iter().enumerate() {
            if attempt > 0 {
                let _ = machine.child.kill();
                let _ = machine.child.wait();
                for socket in [&qmp_socket, &bridge_socket] {
                    let _ = std::fs::remove_file(socket);
                }
                machine.child = qemu(accel).spawn()?;
            }
            match machine.wait_until_socket_ready(Duration::from_secs(25)) {
                Ok(()) => {
                    println!("CPU accelerator: {accel}");
                    return Ok(machine);
                }
                Err(err) if attempt + 1 < accels.len() => {
                    eprintln!("QEMU did not start with {accel} ({err}); retrying with {}", accels[attempt + 1]);
                }
                Err(err) => return Err(err),
            }
        }
        unreachable!("accelerators() is never empty")
    }

    fn wait_until_socket_ready(&mut self, timeout: Duration) -> Result<(), Box<dyn Error>> {
        let socket_path = self.qmp_socket.as_path();
        let start = Instant::now();
        loop {
            if let Some(status) = self.child.try_wait()? {
                return Err(format!("QEMU exited during startup: {status}").into());
            }
            // Ready once QEMU greets a connection with its QMP banner.
            if let Ok(stream) = std::os::unix::net::UnixStream::connect(socket_path) {
                stream.set_read_timeout(Some(Duration::from_secs(2)))?;
                let mut line = String::new();
                if std::io::BufReader::new(stream).read_line(&mut line).is_ok() && !line.trim().is_empty() {
                    return Ok(());
                }
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

/// Sound Blaster 16 (220h, IRQ 5, DMA 1/5) and AdLib (OPL2 FM at 388h), the cards DOS games
/// expect. Without one, a game set up for Sound Blaster calls a driver that isn't there and
/// jumps into the interrupt table (seen as a JemmEx "exception 06" at 0000:00xx). They play
/// into the silent snd0 audiodev for now. Only devices this QEMU build has are added.
fn sound_card_args(qemu_bin: &str) -> Vec<String> {
    let available = Command::new(qemu_bin)
        .args(["-device", "help"])
        .output()
        .map(|out| String::from_utf8_lossy(&out.stdout).into_owned())
        .unwrap_or_default();
    let mut args = Vec::new();
    for card in ["sb16", "adlib"] {
        if available.contains(&format!("name \"{card}\"")) {
            args.extend(["-device".to_string(), format!("{card},audiodev=snd0")]);
        } else {
            eprintln!("sound: this QEMU has no {card} device; DOS programs won't find one");
        }
    }
    args
}

/// KVM runs DOS directly on the host CPU; TCG emulates every instruction and costs far more.
/// Auto mode tries KVM when /dev/kvm is usable and falls back to TCG. DOSLAB_ACCEL=kvm|tcg
/// forces one, e.g. tcg for old games that run too fast at full hardware speed.
fn accelerators() -> Vec<&'static str> {
    match std::env::var("DOSLAB_ACCEL").as_deref() {
        Ok("kvm") => vec!["kvm"],
        Ok("tcg") => vec!["tcg"],
        _ => {
            let kvm_usable = std::fs::OpenOptions::new().read(true).write(true).open("/dev/kvm").is_ok();
            if kvm_usable { vec!["kvm", "tcg"] } else { vec!["tcg"] }
        }
    }
}

/// Standard PC floppy image sizes: 360K, 720K, 1.2M, 1.44M, 2.88M.
const FLOPPY_SIZES: [u64; 5] = [368_640, 737_280, 1_228_800, 1_474_560, 2_949_120];

pub fn is_floppy_size(len: u64) -> bool {
    FLOPPY_SIZES.contains(&len)
}

pub fn is_floppy_image(path: &Path) -> bool {
    std::fs::metadata(path).is_ok_and(|meta| meta.is_file() && is_floppy_size(meta.len()))
}

/// Uploads in progress are hidden files; never offer them.
fn listed_names(dir: &Path, keep: impl Fn(&Path, &str) -> bool) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut names: Vec<String> = entries
        .flatten()
        .filter_map(|entry| Some((entry.path(), entry.file_name().into_string().ok()?)))
        .filter(|(path, name)| !name.starts_with('.') && keep(path, name))
        .map(|(_, name)| name)
        .collect();
    names.sort();
    names
}

/// File names of the floppy images in `dir`, sorted, for the disk picker.
pub fn list_floppies(dir: &Path) -> Vec<String> {
    listed_names(dir, |path, _| is_floppy_image(path))
}

pub fn is_iso_name(name: &str) -> bool {
    name.to_ascii_lowercase().ends_with(".iso")
}

/// File names of the CD images (.iso) in `dir`, sorted, for the CD picker.
pub fn list_isos(dir: &Path) -> Vec<String> {
    listed_names(dir, |path, name| is_iso_name(name) && path.is_file())
}

/// Drives for the VM. The boot image is a floppy or hard disk (by size; BOOT_MODE=hdd|floppy
/// overrides, as in start.sh); without one the VM boots from the CD. `extra_disk` is a raw hard
/// disk (e.g. to install DOS onto): C: unless the boot image is itself a hard disk, then D:.
/// Drive A: and the CD drive always exist, empty if nothing is given, so media can be
/// inserted later (`!disk`, `!cd`, the web pickers).
fn drive_args(image: Option<&Path>, extra_disk: Option<&Path>, cdrom: Option<&Path>) -> Result<Vec<String>, Box<dyn Error>> {
    for (what, path) in [("image", image), ("extra disk", extra_disk), ("CD image", cdrom)] {
        if let Some(path) = path.filter(|path| !path.is_file()) {
            return Err(format!("{what} not found: {}", path.display()).into());
        }
    }
    let mode = match (image, std::env::var("BOOT_MODE")) {
        (None, _) => "cdrom".to_string(),
        (Some(_), Ok(mode)) => mode,
        (Some(image), Err(_)) => if is_floppy_image(image) { "floppy" } else { "hdd" }.to_string(),
    };
    let drive = |spec: String| ["-drive".to_string(), spec];
    let mut args = Vec::new();
    let mut floppy = "if=floppy,index=0".to_string();
    let mut next_disk_index = 0;
    match (mode.as_str(), image) {
        ("floppy", Some(image)) => floppy = format!("file={},format=raw,if=floppy,index=0", image.display()),
        ("hdd", Some(image)) => {
            args.extend(drive(format!("file={},format=raw,if=ide,index=0,media=disk", image.display())));
            next_disk_index = 1;
        }
        ("cdrom", _) => {}
        (other, _) => return Err(format!("BOOT_MODE must be 'hdd' or 'floppy', got '{other}'").into()),
    }
    args.extend(drive(floppy));
    if let Some(disk) = extra_disk {
        println!("Extra disk: {} (IDE index {next_disk_index})", disk.display());
        args.extend(drive(format!("file={},format=raw,if=ide,index={next_disk_index},media=disk", disk.display())));
    }
    let cd = match cdrom {
        Some(iso) => {
            println!("CD: {}", iso.display());
            format!("file={},format=raw,if=ide,index=2,media=cdrom", iso.display())
        }
        None => "if=ide,index=2,media=cdrom".to_string(),
    };
    args.extend(drive(cd));
    let boot = match mode.as_str() {
        "floppy" => "a",
        "hdd" => "c",
        _ => "d",
    };
    args.extend(["-boot".to_string(), boot.to_string()]);
    println!("Boot mode: {mode}");
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
