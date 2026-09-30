#!/usr/bin/env bash
set -Eeuo pipefail

INSIDE_DBUS=0

if [[ "${1:-}" == "--inside-dbus" ]]; then
    INSIDE_DBUS=1
    shift
fi

DOS_IMAGE="${1:-msdos.img}"
QEMU_BIN="${QEMU_BIN:-qemu-system-i386}"
BOOT_MODE="${BOOT_MODE:-hdd}"

RUNTIME_DIR="${XDG_RUNTIME_DIR:-/tmp}/qemu-dos-${UID}"
DBUS_ADDRESS_FILE="${RUNTIME_DIR}/dbus-address"
QMP_SOCKET="${RUNTIME_DIR}/qmp.sock"

mkdir -p "${RUNTIME_DIR}"
chmod 700 "${RUNTIME_DIR}"

if [[ ! -f "${DOS_IMAGE}" ]]; then
    echo "Error: DOS image not found: ${DOS_IMAGE}" >&2
    exit 1
fi

if ! command -v "${QEMU_BIN}" >/dev/null 2>&1; then
    echo "Error: ${QEMU_BIN} is not installed." >&2
    echo "Install with:" >&2
    echo "  sudo apt install qemu-system-x86 dbus-daemon" >&2
    exit 1
fi

if ! command -v dbus-run-session >/dev/null 2>&1; then
    echo "Error: dbus-run-session is not installed." >&2
    echo "Install with:" >&2
    echo "  sudo apt install dbus-daemon" >&2
    exit 1
fi

if ! "${QEMU_BIN}" -display help 2>&1 | grep -qw dbus; then
    echo "Error: this QEMU build has no D-Bus display backend." >&2
    echo "Available display backends:" >&2
    "${QEMU_BIN}" -display help >&2 || true
    exit 1
fi

# Start a private D-Bus session, then execute this script inside it.
if [[ "${INSIDE_DBUS}" -eq 0 ]]; then
    SCRIPT_PATH="$(readlink -f "$0")"
    DOS_IMAGE="$(readlink -f "${DOS_IMAGE}")"

    exec dbus-run-session -- \
        "${SCRIPT_PATH}" \
        --inside-dbus \
        "${DOS_IMAGE}"
fi

# Keep the complete address for external clients such as the Rust bridge.
printf '%s\n' "${DBUS_SESSION_BUS_ADDRESS}" >"${DBUS_ADDRESS_FILE}"
chmod 600 "${DBUS_ADDRESS_FILE}"

# QEMU parses commas inside -display as option separators.
# Remove the optional D-Bus GUID before passing the address to QEMU.
QEMU_DBUS_ADDRESS="${DBUS_SESSION_BUS_ADDRESS%%,guid=*}"

rm -f "${QMP_SOCKET}"

echo "DOS image:      ${DOS_IMAGE}"
echo "Boot mode:      ${BOOT_MODE}"
echo "D-Bus full:     ${DBUS_SESSION_BUS_ADDRESS}"
echo "D-Bus for QEMU: ${QEMU_DBUS_ADDRESS}"
echo "Address file:   ${DBUS_ADDRESS_FILE}"
echo "QMP socket:     ${QMP_SOCKET}"
echo
echo "Waiting for a D-Bus display listener..."

DRIVE_ARGS=()

case "${BOOT_MODE}" in
    hdd)
        DRIVE_ARGS=(
            -drive "file=${DOS_IMAGE},format=raw,if=ide,index=0,media=disk"
            -boot c
        )
        ;;

    floppy)
        DRIVE_ARGS=(
            -drive "file=${DOS_IMAGE},format=raw,if=floppy,index=0"
            -boot a
        )
        ;;

    *)
        echo "Error: BOOT_MODE must be 'hdd' or 'floppy'." >&2
        exit 1
        ;;
esac

exec "${QEMU_BIN}" \
    -name "Coaba DOS Lab" \
    -audiodev none,id=snd0 \
    -machine pc,pcspk-audiodev=snd0 \
    -accel tcg \
    -cpu 486 \
    -m 32M \
    "${DRIVE_ARGS[@]}" \
    -display "dbus,addr=${QEMU_DBUS_ADDRESS}" \
    -qmp "unix:${QMP_SOCKET},server=on,wait=off" \
    -nic none \
    -rtc base=localtime
