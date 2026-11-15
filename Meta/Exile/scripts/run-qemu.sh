#!/usr/bin/env bash
# Boot an Exile OS image in QEMU through the project's own launcher (Meta/run.sh -> Meta/run.py), capture the logs, and decide
# pass/fail from what the guest actually printed.
#
# Modes
#   boot         Headless boot with the 'ci' machine: serial on stdout, kernel debug output (dbgln) on a debugcon file.
#                Succeeds only if every --expect pattern shows up and no panic marker does, before --timeout.
#   interactive  The normal windowed machine (needs a display). Nothing is captured or judged.
#   debug        Like boot, plus extra QEMU diagnostics (guest errors, CPU resets, unimplemented devices), the guest is not
#                shut down on a reboot/triple fault, and optionally (--gdb-wait) QEMU is frozen until gdb attaches.
#                Meta/run.py always opens a gdb stub on 127.0.0.1:1234 (unless EXILE_DISABLE_GDB_SOCKET=1).
#
# A boot is never reported as successful without at least one --expect pattern (or --profile scheduler), because a VM that merely
# stays alive proves nothing about the kernel.
#
# Exit codes: 0 success, 1 usage, 2 missing host tool (QEMU), 3 BLOCKED (kernel or image missing),
#             4 QEMU failed to start/exited early, 6 boot failed (panic, timeout, expected output missing).
# Last line: RESULT <PASS|FAIL|BLOCKED|USAGE> step=<name> reason=<text>
set -u -o pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"
ARCH="${EXILE_ARCH:-x86_64}"
BOOTLOADER="qemu"
MODE="boot"
OUT="${EXILE_VALIDATION_OUT:-$ROOT/Build/m1-validation}"
MEM="${EXILE_RAM_SIZE:-2G}"
CPUS="${EXILE_CPUS:-1}"
TIMEOUT=
IMAGE=""
SMP=0
SELFTEST=0
GDB_WAIT=0
EXPECTS=()
KERNEL_ARGS=""
RUN_SH="${EXILE_RUN_SH:-$ROOT/Meta/run.sh}"
PANIC_RE='KERNEL PANIC|Kernel panic|ASSERTION FAILED|VERIFICATION FAILED|Unhandled exception|Double fault|Triple fault'

usage() {
    cat <<EOF
Usage: ${0##*/} [--mode boot|interactive|debug] [--bootloader qemu|limine|grub|extlinux] [--image FILE]
                [--mem 2G] [--cpus N] [--smp] [--selftest] [--profile scheduler] [--expect REGEX]...
                [--cmdline 'kernel args'] [--timeout SECONDS (default 300, 3600 with --selftest)] [--gdb-wait] [--out DIR]

  --cpus N         Guest CPU count (QEMU -smp). Default 1. With --smp the kernel is also told smp=on; SMP is off in the kernel
                   by default and is not covered by this project's tests, so results with --smp are reported as such.
  --selftest       Boot with system_mode=self-test.
  --profile scheduler
                   Self-test boot that must print the verdict lines of TestSchedulerPriority and TestFinalizerUnderLoad.
                   A PASS from this script means 'the expected output appeared'; it does NOT judge the test results.
                   Use Meta/Exile/scripts/validate-scheduler-priority.sh for the verdicts.
  --expect REGEX   Extended regex that must appear in the serial or debug log (repeatable).
Logs: <out>/qemu-serial.log (stdout/serial), <out>/qemu-debug.log (debugcon), <out>/qemu-diag.log (debug mode only).
EOF
}
result() { echo "RESULT $1 step=$2 reason=$3"; exit "$4"; }

while [ $# -gt 0 ]; do
    case "$1" in
        --mode) MODE="${2:-}"; shift 2 || result USAGE args "--mode needs a value" 1 ;;
        --bootloader) BOOTLOADER="${2:-}"; shift 2 || result USAGE args "--bootloader needs a value" 1 ;;
        --image) IMAGE="${2:-}"; shift 2 || result USAGE args "--image needs a value" 1 ;;
        --mem) MEM="${2:-}"; shift 2 || result USAGE args "--mem needs a value" 1 ;;
        --cpus) CPUS="${2:-}"; shift 2 || result USAGE args "--cpus needs a value" 1 ;;
        --smp) SMP=1; shift ;;
        --selftest) SELFTEST=1; shift ;;
        --profile) [ "${2:-}" = scheduler ] || result USAGE args "unknown profile '${2:-}'" 1
                   SELFTEST=1; EXPECTS+=("SCHED-PRIO-TEST: .*result=(pass|fail)" "FINALIZER-TEST: .*result=(pass|fail)"); shift 2 ;;
        --expect) EXPECTS+=("${2:-}"); shift 2 || result USAGE args "--expect needs a pattern" 1 ;;
        --cmdline) KERNEL_ARGS="${2:-}"; shift 2 || result USAGE args "--cmdline needs a value" 1 ;;
        --timeout) TIMEOUT="${2:-}"; shift 2 || result USAGE args "--timeout needs a value" 1 ;;
        --gdb-wait) GDB_WAIT=1; shift ;;
        --out) OUT="${2:-}"; shift 2 || result USAGE args "--out needs a value" 1 ;;
        -h|--help) usage; exit 0 ;;
        *) usage >&2; result USAGE args "unknown argument: $1" 1 ;;
    esac
done
case "$MODE" in boot|interactive|debug) ;; *) result USAGE args "unknown --mode '$MODE'" 1 ;; esac
case "$ARCH" in x86_64) ;; *) result USAGE args "only x86_64 is supported by this script" 1 ;; esac
[[ "$CPUS" =~ ^[1-9][0-9]*$ ]] || result USAGE args "--cpus must be a positive integer" 1
if [ -z "$TIMEOUT" ]; then TIMEOUT=300; [ "$SELFTEST" = 1 ] && TIMEOUT=3600; fi   # self-test runs the whole installed suite
[[ "$TIMEOUT" =~ ^[1-9][0-9]*$ ]] || result USAGE args "--timeout must be a positive integer" 1
[[ "$MEM" =~ ^[1-9][0-9]*[MG]$ ]] || result USAGE args "--mem must look like 512M or 2G" 1
if [ "$SMP" = 1 ] && [ "$CPUS" -lt 2 ]; then result USAGE args "--smp needs --cpus 2 or more" 1; fi
if [ "$MODE" != interactive ] && [ ${#EXPECTS[@]} -eq 0 ]; then
    result USAGE args "boot/debug mode needs --expect REGEX or --profile scheduler: staying alive is not proof of a boot" 1
fi

case "$BOOTLOADER" in
    qemu) IMAGE_NAME=_disk_image; MACHINE="" ;;
    limine) IMAGE_NAME=limine_disk_image; MACHINE=limine ;;
    grub) IMAGE_NAME=grub_disk_image; MACHINE=qgrub ;;
    extlinux) IMAGE_NAME=extlinux_disk_image; MACHINE=qextlinux ;;
    *) result USAGE args "unknown --bootloader '$BOOTLOADER'" 1 ;;
esac
# Headless boot/debug use the 'ci' machine, which boots the kernel straight from the image's build directory (-kernel).
# Only the plain qemu layout is supported there; bootloader images are run with their own machine type.
[ "$MODE" != interactive ] && [ "$BOOTLOADER" = qemu ] && MACHINE=ci
BUILD_DIR="$ROOT/Build/$ARCH"
[ -n "$IMAGE" ] || IMAGE="$BUILD_DIR/$IMAGE_NAME"

# ------------------------------------------------------------------------------ prerequisites
export PATH="$ROOT/Toolchain/Local/qemu/bin:$PATH"
QEMU="qemu-system-x86_64"
[ -n "${EXILE_QEMU_BIN:-}" ] && QEMU="$EXILE_QEMU_BIN"
if ! command -v "$QEMU" >/dev/null 2>&1; then
    echo "QEMU not found ($QEMU). Install QEMU 6.2 or newer (apt install qemu-system-x86) or run Toolchain/BuildQemu.sh." >&2
    result BLOCKED qemu "missing $QEMU" 2
fi
qemu_version="$("$QEMU" --version 2>/dev/null | sed -n '1s/.*version \([0-9][0-9]*\.[0-9][0-9]*\).*/\1/p')"
[ -n "$qemu_version" ] || result BLOCKED qemu "cannot read the version of $QEMU" 2
if [ "$(printf '6.2\n%s\n' "$qemu_version" | sort -V | head -n1)" != "6.2" ]; then
    result BLOCKED qemu "QEMU $qemu_version is older than the 6.2 that Meta/run.py requires" 2
fi
[ -x "$RUN_SH" ] || result BLOCKED launcher "$RUN_SH is not executable" 3
[ -s "$BUILD_DIR/Kernel/Kernel" ] || result BLOCKED prerequisites "kernel not built: $BUILD_DIR/Kernel/Kernel (run Meta/Exile/scripts/build-kernel.sh)" 3
[ -s "$IMAGE" ] || result BLOCKED prerequisites "image not found: $IMAGE (run Meta/Exile/scripts/build-image.sh --bootloader $BOOTLOADER)" 3

# ------------------------------------------------------------------------------ environment for Meta/run.py
CMDLINE="graphics_subsystem_mode=off serial_debug"
[ "$MODE" = interactive ] && CMDLINE=""
[ "$SELFTEST" = 1 ] && CMDLINE="$CMDLINE system_mode=self-test"
[ "$SMP" = 1 ] && CMDLINE="$CMDLINE smp=on"
[ -n "$KERNEL_ARGS" ] && CMDLINE="$CMDLINE $KERNEL_ARGS"
export EXILE_SOURCE_DIR="$ROOT" EXILE_ARCH="$ARCH" EXILE_BUILD_DIR="$BUILD_DIR" EXILE_RAM_SIZE="$MEM" EXILE_CPUS="$CPUS"
export EXILE_DISK_IMAGE="$IMAGE"
[ -n "$MACHINE" ] && export EXILE_RUN="$MACHINE"
[ -n "${CMDLINE# }" ] && export EXILE_KERNEL_CMDLINE="${CMDLINE# }"

mkdir -p "$OUT" || result FAIL setup "cannot create $OUT" 4
SERIAL="$OUT/qemu-serial.log"; DEBUGLOG="$OUT/qemu-debug.log"; DIAG="$OUT/qemu-diag.log"
: > "$SERIAL"; : > "$DEBUGLOG"
echo "mode=$MODE machine=${MACHINE:-default} image=$IMAGE cpus=$CPUS mem=$MEM smp_flag=$SMP qemu=$qemu_version" | tee "$OUT/qemu-run-config.txt"
echo "kernel cmdline: ${EXILE_KERNEL_CMDLINE:-<none>}" | tee -a "$OUT/qemu-run-config.txt"

if [ "$MODE" = interactive ]; then
    exec "$RUN_SH"
fi

# ------------------------------------------------------------------------------ debug mode: a QEMU shim that adds diagnostics
SHIM_DIR=""
cleanup() { [ -n "$SHIM_DIR" ] && rm -rf "$SHIM_DIR"; return 0; }
trap cleanup EXIT
if [ "$MODE" = debug ]; then
    SHIM_DIR="$(mktemp -d)"
    REAL_QEMU="$(command -v "$QEMU")"
    extra="-d guest_errors,cpu_reset,unimp -D $DIAG -no-shutdown"
    [ "$GDB_WAIT" = 1 ] && extra="$extra -S"
    cat > "$SHIM_DIR/qemu-system-x86_64-exile-diag" <<SHIM
#!/bin/sh
# Adds diagnostics only to the real VM start (recognised by -kernel); queries like -version pass through unchanged.
case " \$* " in
    *" -kernel "*) exec "$REAL_QEMU" "\$@" $extra ;;
    *) exec "$REAL_QEMU" "\$@" ;;
esac
SHIM
    chmod +x "$SHIM_DIR/qemu-system-x86_64-exile-diag"
    export PATH="$SHIM_DIR:$PATH" EXILE_QEMU_BIN=qemu-system-x86_64-exile-diag
    echo "debug: gdb stub on 127.0.0.1:1234 (attach with: gdb Build/$ARCH/Kernel/Kernel -ex 'target remote :1234'); diagnostics -> $DIAG"
    [ "$GDB_WAIT" = 1 ] && echo "debug: QEMU starts frozen (-S); continue from gdb"
fi

# ------------------------------------------------------------------------------ run, watch the logs, decide
rm -f "$BUILD_DIR/debug.log"           # the 'ci' machine writes kernel debug output here
setsid env \
    PATH="$PATH" \
    EXILE_SOURCE_DIR="$ROOT" \
    EXILE_ARCH="$ARCH" \
    EXILE_BUILD_DIR="$BUILD_DIR" \
    EXILE_RAM_SIZE="$MEM" \
    EXILE_CPUS="$CPUS" \
    EXILE_DISK_IMAGE="$IMAGE" \
    EXILE_RUN="$MACHINE" \
    EXILE_KERNEL_CMDLINE="${EXILE_KERNEL_CMDLINE:-}" \
    EXILE_QEMU_BIN="${EXILE_QEMU_BIN:-}" \
    STUB_SCENARIO="${STUB_SCENARIO:-}" \
    "$RUN_SH" >"$SERIAL" 2>&1 &
PID=$!
start=$SECONDS
verdict=""; reason=""
logs_text() { cat "$SERIAL" "$BUILD_DIR/debug.log" 2>/dev/null; }
while :; do
    text="$(logs_text)"
    if echo "$text" | grep -Eq "$PANIC_RE"; then verdict=PANIC; reason="panic marker: $(echo "$text" | grep -Eo "$PANIC_RE" | head -n1)"; break; fi
    all=1
    for pat in "${EXPECTS[@]}"; do echo "$text" | grep -Eq -- "$pat" || { all=0; break; }; done
    if [ "$all" = 1 ]; then verdict=PASS; reason="all ${#EXPECTS[@]} expected pattern(s) seen after $((SECONDS - start))s"; break; fi
    if ! kill -0 "$PID" 2>/dev/null; then verdict=EXIT; reason="QEMU exited before the expected output appeared"; break; fi
    if [ $((SECONDS - start)) -ge "$TIMEOUT" ]; then verdict=TIMEOUT; reason="timeout after ${TIMEOUT}s; missing: $(for p in "${EXPECTS[@]}"; do echo "$text" | grep -Eq -- "$p" || printf '%s; ' "$p"; done)"; break; fi
    sleep 1
done
# Stop the VM (the whole process group started by setsid).
kill -TERM -- "-$PID" 2>/dev/null; sleep 1; kill -KILL -- "-$PID" 2>/dev/null
wait "$PID" 2>/dev/null
[ -f "$BUILD_DIR/debug.log" ] && cp "$BUILD_DIR/debug.log" "$DEBUGLOG"
echo "logs: $SERIAL ($(wc -l <"$SERIAL") lines), $DEBUGLOG ($(wc -l <"$DEBUGLOG") lines)"

case "$verdict" in
    PASS) [ "$SMP" = 1 ] && echo "NOTE: this boot used smp=on with $CPUS CPUs; it proves only what the expected patterns show."
          result PASS boot "$reason" 0 ;;
    PANIC) tail -n 15 "$SERIAL" >&2; result FAIL boot "$reason" 6 ;;
    EXIT) tail -n 15 "$SERIAL" >&2; result FAIL qemu-start "$reason" 4 ;;
    *) result FAIL boot "$reason" 6 ;;
esac
