#!/bin/bash
# Send all of WSL's sound to rtp-audio running on Windows.
#   ./wsl-send.sh [windows-host] [port]     (Ctrl+C to stop and switch sound back)
# The host defaults to Windows as seen from WSL; with mirrored networking pass 127.0.0.1.
host=${1:-$(ip route | awk '/default/ {print $3; exit}')}
port=${2:-46000}
here=$(dirname "$(readlink -f "$0")")
bin=$here/rtp-audio
[ -x "$bin" ] || bin=$here/target/release/rtp-audio

# Remove a "windows" output left over from a run that didn't clean up.
for module in $(pactl list short modules | awk '/module-null-sink/ && /sink_name=windows/ {print $1}'); do
  pactl unload-module "$module"
done
previous=$(pactl get-default-sink) || exit 1

# A sink that plays nowhere; its monitor is what we send.
module=$(pactl load-module module-null-sink sink_name=windows sink_properties=device.description=Windows) || exit 1

# Switch back on any exit: unloading the sink moves every app on it back to the default.
cleanup() {
  trap - EXIT INT TERM
  pactl set-default-sink "$previous"
  pactl unload-module "$module"
  echo "Sound switched back to $previous"
}
trap cleanup EXIT
trap 'exit 130' INT TERM

pactl set-default-sink windows
# Apps already playing keep their old output unless moved.
for input in $(pactl list short sink-inputs | cut -f1); do pactl move-sink-input "$input" windows; done

parec -d windows.monitor --raw --format=s16be --rate=48000 --channels=2 --latency-msec=20 \
  | "$bin" send "$host:$port"
