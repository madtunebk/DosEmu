# Coaba DOS Lab sanity viewer

Headless QEMU + live Tk viewer + DOS keyboard/game input. This is a proof of
concept: video uses QMP `screendump` polling; the future Rust bridge should use
QEMU Display1 `ScanoutMap`/`UpdateMap` directly.

## Requirements

```bash
sudo apt install qemu-system-x86 dbus-daemon python3-tk
```

## Run

```bash
chmod +x start.sh viewer.py
./start.sh images/FD14FULL.img
```

In another terminal:

```bash
python3 viewer.py
```

Type DOS commands in the field, or click the image for direct WASD, arrows,
Space, Enter, Escape, modifiers and F1-F12 input.

Do not run QEMU as root. The provided configuration disables networking and
enables QEMU's sandbox.
