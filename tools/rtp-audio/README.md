# rtp-audio

Play sound from WSL (or any Linux machine) on Windows, over the network. One small program:

- `rtp-audio` (or `rtp-audio receive`) on **Windows**: listens on UDP port 46000 and plays on the
  default sound device. A single `.exe`, no DLLs or installer.
- `rtp-audio send` on **Linux**: reads raw audio from `parec` and sends it as RTP.

It also plays standard RTP L16 streams (PulseAudio `module-rtp-send`, PipeWire `module-rtp-sink`,
`ffmpeg -f rtp -acodec pcm_s16be`) at 48 kHz stereo, so other machines can send to it too.

## Use

Windows (allow it through the firewall the first time):

```
rtp-audio.exe
```

WSL (needs `pulseaudio-utils` for `pactl`/`parec`):

```bash
./wsl-send.sh            # Ctrl+C stops and switches sound back
```

`wsl-send.sh` makes a "Windows" output, sets it as default, moves apps already playing onto it,
and sends it to Windows. With WSL mirrored networking use `./wsl-send.sh 127.0.0.1`.

Options (receiver): `--port 46000`, `--latency 60` (buffer in ms; raise it if it crackles),
`--rate 48000`, `--channels 2`.

## Build

```bash
cargo build --release                                   # this machine
cargo build --release --target x86_64-pc-windows-gnu    # Windows .exe from Linux (needs mingw-w64)
```

## How it works

Packets go into a jitter buffer (60 ms by default) that absorbs network hiccups; lost packets
become silence. The sound card pulls from it through a small resampler, which also plays up to
0.5% faster or slower to keep the buffer level, since the sender's and the sound card's clocks
never run at exactly the same speed.
