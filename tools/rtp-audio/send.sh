#!/bin/bash
# Send this machine's sound to rtp-audio running on Windows.
#
#   ./send.sh [host] [port]               all sound: apps play to a "Windows" output that is sent
#   ./send.sh -s <source> [host] [port]   send one existing source instead (a microphone, or
#                                         "<speakers>.monitor" to send a copy and keep playing here)
#   ./send.sh -l                          list sources (use the number or the name with -s)
#
# Ctrl+C stops and switches sound back. The host defaults to Windows as seen from WSL; on a real
# machine give the Windows PC's IP (ipconfig), with WSL mirrored networking 127.0.0.1.
usage() { sed -n '2,10s/^# \{0,1\}//p' "$0"; }

source=
args=()
while [ $# -gt 0 ]; do
  case $1 in
    -l | --list)
      pactl list short sources | awk -F'\t' '{printf "%3s  %-60s %s\n", $1, $2, $5}'
      exit ;;
    -s | --source)
      [ -n "$2" ] || { echo "$1 needs a source (see -l)" >&2; exit 1; }
      source=$2; shift 2 ;;
    -h | --help) usage; exit ;;
    -*) echo "unknown option $1" >&2; usage >&2; exit 1 ;;
    *) args+=("$1"); shift ;;
  esac
done
host=${args[0]:-$(ip route | awk '/default/ {print $3; exit}')}
port=${args[1]:-46000}
here=$(dirname "$(readlink -f "$0")")
bin=$here/rtp-audio
[ -x "$bin" ] || bin=$here/target/release/rtp-audio

send() {
  parec -d "$1" --raw --format=s16be --rate=48000 --channels=2 --latency-msec=20 \
    | "$bin" send "$host:$port"
}

if [ -n "$source" ]; then
  # A number from -l, or a name.
  if [[ $source =~ ^[0-9]+$ ]]; then
    source=$(pactl list short sources | awk -F'\t' -v n="$source" '$1 == n {print $2}')
  fi
  if [ -z "$source" ] || ! pactl list short sources | cut -f2 | grep -qxF "$source"; then
    echo "no such source; these exist:" >&2
    pactl list short sources | awk -F'\t' '{printf "%3s  %s\n", $1, $2}' >&2
    exit 1
  fi
  echo "Sending source $source"
  send "$source"
  exit
fi

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

send windows.monitor
