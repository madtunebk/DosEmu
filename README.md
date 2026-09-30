# Coaba DOS Lab

Run DOS in QEMU and play it in your browser: picture, keyboard, sound, floppy and CD swapping,
all through one small Rust bridge.

```
QEMU (FreeDOS / MS-DOS)  ──D-Bus display + audio──▶  doslab bridge  ──WebSocket──▶  browser
          ▲                                               │
          └──────────── QMP (keys, disk swaps) ◀──────────┘
```

## Requirements

```bash
sudo apt install qemu-system-x86 dbus-daemon   # QEMU 8.2+ with the D-Bus display and audio
```

and a Rust toolchain (`rustup`, edition 2024).

## Getting DOS disk images

Disk images aren't kept in this repository (`images/` is in `.gitignore`); bring your own:

- **FreeDOS** (free): download the FreeDOS 1.4 *Floppy Edition* (1.44 MB disks) from
  <https://www.freedos.org/download/>, unzip it and copy the boot disk and install disks
  (`x86BOOT.img`, `x86DSK01.img` … `x86DSK06.img`) into `images/`.
- **MS-DOS** or other DOS versions: use your own disk images the same way.

Anything in `images/` (or `DOSLAB_DISK_DIR`) shows up in the page's A: and CD pickers.

## Run

```bash
cargo build --release
target/release/doslab images/x86BOOT.img c.img
```

Open <http://127.0.0.1:3000>, click the screen and type.

Arguments, in any combination:

| Argument | Meaning |
|---|---|
| first image | what to boot: a floppy image (by size: 360K–2.88M) boots as A:, anything else as a hard disk |
| second image | an extra raw hard disk: C: when booting a floppy, D: when booting a hard disk |
| `*.iso` | inserted in the CD drive; with only an `.iso`, the VM boots from the CD |

### Installing FreeDOS onto a hard disk

```bash
qemu-img create -f raw c.img 500M
target/release/doslab images/x86BOOT.img c.img     # install; swap disks 01-06 in the A: picker
target/release/doslab c.img                        # afterwards, boot from C:
```

## The web page

- **Screen:** click it to send keys to DOS (the frame glows amber while it has the keyboard).
- **🖱 Mouse:** clicking the screen also captures the mouse for DOS (needs a DOS mouse driver
  such as CTMOUSE, which FreeDOS loads). **Esc** gives the pointer back; in fullscreen Esc goes
  to DOS and you hold Esc to leave. Turn the button off for programs without mouse support.
- **🔊 Sound:** plays the Sound Blaster 16, AdLib and PC speaker. Browsers only allow audio
  after a click; once turned on, a click on the screen restarts it. The header shows
  `gaps`/`drops` counters if playback struggles.
- **📷 Screenshot:** saves the exact screen as a PNG. **▤ CRT:** scanlines. **⛶ Fullscreen.**
- **⟲ Ctrl+Alt+Del:** reboots DOS.
- **Drive bay:** pick a floppy (A:) or CD image and press Insert/Eject, or drop a `.img` or
  `.iso` file anywhere on the page to upload and insert it. A dropped file never overwrites
  a different one with the same name; it becomes `name-2.img`.

## Settings

| Variable | Default | Meaning |
|---|---|---|
| `DOSLAB_WEB_ADDR` | `127.0.0.1:3000` | where the web page is served |
| `DOSLAB_DISK_DIR` | `images` | folder the floppy/CD pickers list and uploads go to |
| `DOSLAB_ACCEL` | `tcg` | `kvm` for hardware virtualization (faster, but some DOS games and memory managers crash under it) |
| `DOSLAB_SOUND` | on | `off`: keep the sound cards but send no sound |
| `DOSLAB_VIDEO` | D-Bus | `screendump`: old polling capture, for comparison |
| `BOOT_MODE` | by image size | `floppy` or `hdd` to force how the boot image is attached |
| `QEMU_BIN` | `qemu-system-i386` | QEMU binary |

## Terminal commands

While `doslab` runs, its terminal accepts:

| Command | Does |
|---|---|
| `dir` + Enter | any line is typed into DOS, followed by Enter |
| `:esc`, `:down down ret`, `:ctrl+alt+delete` | tap keys (QEMU key names) |
| `!disk`, `!disk <image>` | list floppies / insert one in A: |
| `!cd`, `!cd <image.iso>` | list CD images / insert one |
| `!eject`, `!eject cd` | empty A: / the CD drive |
| `!shot [file.ppm]` | save the current frame |
| `!stats` | video source, frame count and update rate |

## DOS tips

- **Sound in games:** the VM has a Sound Blaster 16 at port 220, IRQ 5, DMA 1/5 and an AdLib.
  Choose those in the game's setup, and add to `C:\FDAUTO.BAT` (FreeDOS's `AUTOEXEC.BAT`):
  ```
  SET BLASTER=A220 I5 D1 H5 T6
  ```
- **Save CPU:** DOS busy-waits for keys, so QEMU uses a full CPU core even at the prompt.
  Add this line to `C:\FDAUTO.BAT` and idle CPU drops from ~100% to ~1%:
  ```
  FDAPM APMDOS
  ```
- **Editing files:** `EDIT C:\FDAUTO.BAT` in DOS; or with the VM off, from Linux:
  `mcopy -i c.img@@1M ::FDAUTO.BAT .` (and `mcopy -o -i c.img@@1M FDAUTO.BAT ::FDAUTO.BAT` back).
  `@@1M` is where the C: partition starts; if mtools reports no FAT filesystem there, the disk
  was partitioned the older way and it's `@@32256`.
- **A game crashes:** try the FreeDOS boot menu option without JemmEx/EMM386; some early-90s
  games don't cope with memory managers.

## How it works

- **Video:** the bridge registers as QEMU's D-Bus display listener; QEMU pushes changed
  rectangles (no polling). Each browser gets only the area that changed since the frame it
  last drew, as a lossless palette PNG (DOS never shows more than 256 colours), one frame in
  flight at a time so a slow browser never builds up lag.
- **Sound:** the sound cards play into QEMU's D-Bus audio backend; the bridge forwards 16-bit
  PCM to the page, which plays it ~100 ms ahead with Web Audio and never overlaps chunks.
- **Keyboard, mouse and drives:** QMP `input-send-event` (keys, relative PS/2 mouse motion
  and buttons, in one ordered queue) and `blockdev-change-medium`.
- **Robustness:** a program that crashes the virtual CPU reboots DOS instead of ending the
  emulator.

`start.sh` and `scripts/viewer.py` are the original Python prototype (screendump polling + Tk).
