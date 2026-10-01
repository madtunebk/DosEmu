#!/bin/bash
# Send all of WSL's sound to rtp-audio running on Windows.
#   ./wsl-send.sh [windows-host] [port]     (Ctrl+C to stop and switch sound back)
# The host defaults to Windows as seen from WSL; with mirrored networking pass 127.0.0.1.
set -e
host=${1:-$(ip route | awk '/default/ {print $3; exit}')}
port=${2:-46000}
here=$(dirname "$(readlink -f "$0")")
bin=$here/rtp-audio
[ -x "$bin" ] || bin=$here/target/release/rtp-audio

# A sink that plays nowhere; its monitor is what we send.
if ! pactl list short sinks | grep -q $'\twindows\t'; then
  pactl load-module module-null-sink sink_name=windows sink_properties=device.description=Windows >/dev/null
fi
previous=$(pactl get-default-sink)
pactl set-default-sink windows
# Apps already playing keep their old output unless moved.
for input in $(pactl list short sink-inputs | cut -f1); do pactl move-sink-input "$input" windows || true; done
trap 'pactl set-default-sink "$previous"' EXIT

parec -d windows.monitor --raw --format=s16be --rate=48000 --channels=2 --latency-msec=20 \
  | "$bin" send "$host:$port"
