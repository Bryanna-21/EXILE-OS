#!/usr/bin/env bash
# GNU/x86_64 build, boot, scheduler and full-suite preemption validation.
set -u -o pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"
SCRIPTS="$ROOT/Meta/Exile/scripts"
OUT="${EXILE_VALIDATION_OUT:-$ROOT/Build/m1-validation/pipeline}"
SKIP_SMP=0

usage() {
    cat <<USAGE
Usage: ${0##*/} [--out DIR] [--skip-smp]

Runs the GNU/x86_64 validation pipeline:
  1. Kernel build
  2. QEMU image build and validation
  3. Normal boot milestone validation
  4. Scheduler host/guest validation (optionally without SMP)
  5. Separate full-suite boot and preemption verification

Clang is intentionally unsupported because the image builder, QEMU runner,
and scheduler validator currently use the GNU build directory.
USAGE
}

while [ "$#" -gt 0 ]; do
    case "$1" in
        --out)
            [ "$#" -ge 2 ] || {
                echo "RESULT USAGE step=args reason=--out-needs-value"
                exit 1
            }
            OUT="$2"
            shift 2
            ;;
        --skip-smp)
            SKIP_SMP=1
            shift
            ;;
        --toolchain)
            echo "RESULT USAGE step=args reason=only-GNU-is-supported"
            exit 1
            ;;
        -h|--help)
            usage
            exit 0
            ;;
        *)
            echo "RESULT USAGE step=args reason=unknown-argument-$1"
            usage >&2
            exit 1
            ;;
    esac
done

mkdir -p "$OUT" || {
    echo "RESULT FAIL step=setup reason=cannot-create-output-directory"
    exit 4
}

NAMES=()
STATES=()
DETAILS=()
PASS_COUNT=0
FAIL_COUNT=0
BLOCKED_COUNT=0
NOT_RUN_COUNT=0

record() {
    NAMES+=("$1")
    STATES+=("$2")
    DETAILS+=("$3")
}

state_from_rc() {
    case "$1" in
        0) echo PASS ;;
        2|3) echo BLOCKED ;;
        *) echo FAIL ;;
    esac
}

run_stage() {
    local name="$1" log="$2"
    shift 2

    echo
    echo "========== $name =========="
    "$@" >"$log" 2>&1
    local rc=$?
    local state
    state="$(state_from_rc "$rc")"

    # The preemption verifier uses rc=2 specifically for a detected panic.
    if [ "$name" = "preemption verification" ] && [ "$rc" -eq 2 ]; then
        state=FAIL
    fi
    local detail
    detail="$(grep '^RESULT ' "$log" | tail -n 1)"
    [ -n "$detail" ] || detail="exit=$rc; see $log"

    cat "$log"
    record "$name" "$state" "$detail"
    echo "STAGE $name: $state"
}

skip_stage() {
    record "$1" BLOCKED "$2"
}

KERNEL_LOG="$OUT/kernel-build.txt"
IMAGE_LOG="$OUT/image-build.txt"
BOOT_DIR="$OUT/normal-boot"
BOOT_LOG="$OUT/normal-boot.txt"
SCHED_LOG="$OUT/scheduler-validation.txt"
FULL_DIR="$OUT/full-suite"
FULL_LOG="$OUT/full-suite-boot.txt"

echo "EXILE-OS validation pipeline"
echo "Branch: $(git -C "$ROOT" branch --show-current)"
echo "Output: $OUT"
echo "Toolchain: GNU"
echo "Architecture: x86_64"

# Stage 1: kernel build.
run_stage "kernel build" "$KERNEL_LOG" \
    bash "$SCRIPTS/build-kernel.sh" \
    --arch x86_64 --toolchain GNU --out "$OUT/kernel"

KERNEL_STATE="${STATES[0]}"

# Stage 2: image build. Never proceed using a potentially stale kernel.
if [ "$KERNEL_STATE" = PASS ]; then
    run_stage "image build" "$IMAGE_LOG" \
        bash "$SCRIPTS/build-image.sh" \
        --bootloader qemu --arch x86_64 --out "$OUT/image"
else
    skip_stage "image build" "kernel build did not pass"
fi

IMAGE_STATE="${STATES[1]}"

if [ "$IMAGE_STATE" = PASS ]; then
    mkdir -p "$BOOT_DIR"

    # Stage 3: normal boot. The runner watches both serial and kernel debug output.
    run_stage "normal boot" "$BOOT_LOG" \
        bash "$SCRIPTS/run-qemu.sh" \
        --mode boot --timeout 300 --out "$BOOT_DIR" \
        --expect 'MM: (Multiboot mmap|Multiboot module|EFI memory map)' \
        --expect 'MM: Physical page entries:' \
        --expect 'CPU\[0\]: Supported features:' \
        --expect 'Scheduler\[[0-9]+\]: idle loop running' \
        --expect 'Read system_mode:' \
        --expect 'Activating [0-9]+ services in dependency order'

    if [ "${STATES[2]}" = PASS ]; then
        run_stage "boot milestone verification" "$OUT/boot-milestones.txt" \
            bash "$SCRIPTS/check-boot-milestones.sh" \
            "$BOOT_DIR/qemu-serial.log" "$BOOT_DIR/qemu-debug.log"
    else
        skip_stage "boot milestone verification" "normal boot did not pass"
    fi

    # Stage 4: scheduler validation is its own boot and its own report.
    SCHED_ARGS=(--out "$OUT/scheduler")
    [ "$SKIP_SMP" = 1 ] && SCHED_ARGS+=(--skip-smp)

    run_stage "scheduler validation" "$SCHED_LOG" \
        bash "$SCRIPTS/validate-scheduler-priority.sh" "${SCHED_ARGS[@]}"

    # Stage 5: separate full installed test suite boot.
    mkdir -p "$FULL_DIR"
    run_stage "full-suite self-test boot" "$FULL_LOG" \
        bash "$SCRIPTS/run-qemu.sh" \
        --selftest --timeout 3600 --out "$FULL_DIR" \
        --expect 'TEST-SUITE-RESULT: failed=[0-9]+'

    if [ "${STATES[5]}" = PASS ]; then
        SERIAL="$FULL_DIR/qemu-serial.log"
        DEBUG="$FULL_DIR/qemu-debug.log"
        EVIDENCE="$FULL_DIR/preemption-evidence.log"

        if [ ! -s "$SERIAL" ] || [ ! -s "$DEBUG" ]; then
            record "preemption verification" FAIL \
                "serial or debug log missing/empty"
        else
            cat "$SERIAL" "$DEBUG" > "$EVIDENCE"

            VERIFY="$ROOT/Meta/Exile/verify-preemption-serial.py"
            VERIFY_ARG="$VERIFY"
            EVIDENCE_ARG="$EVIDENCE"
            if command -v cygpath >/dev/null 2>&1; then
                VERIFY_ARG="$(cygpath -w "$VERIFY")"
                EVIDENCE_ARG="$(cygpath -w "$EVIDENCE")"
            fi

            run_stage "preemption verification" "$OUT/preemption-verification.txt" \
                python3 -I "$VERIFY_ARG" "$EVIDENCE_ARG"
        fi
    else
        skip_stage "preemption verification" \
            "full-suite self-test boot did not pass"
    fi
else
    skip_stage "normal boot" "image build did not pass"
    skip_stage "boot milestone verification" "image build did not pass"
    skip_stage "scheduler validation" "image build did not pass"
    skip_stage "full-suite self-test boot" "image build did not pass"
    skip_stage "preemption verification" "image build did not pass"
fi

echo
echo "========== PIPELINE SUMMARY =========="
printf '%-34s %-9s %s\n' "STAGE" "STATE" "DETAIL"

for i in "${!NAMES[@]}"; do
    printf '%-34s %-9s %s\n' \
        "${NAMES[$i]}" "${STATES[$i]}" "${DETAILS[$i]}"
    case "${STATES[$i]}" in
        PASS) PASS_COUNT=$((PASS_COUNT + 1)) ;;
        FAIL) FAIL_COUNT=$((FAIL_COUNT + 1)) ;;
        BLOCKED) BLOCKED_COUNT=$((BLOCKED_COUNT + 1)) ;;
        *) NOT_RUN_COUNT=$((NOT_RUN_COUNT + 1)) ;;
    esac
done

SUMMARY="PASS=$PASS_COUNT FAIL=$FAIL_COUNT BLOCKED=$BLOCKED_COUNT NOT_RUN=$NOT_RUN_COUNT"
echo "$SUMMARY"
echo "Report directory: $OUT"

if [ "$FAIL_COUNT" -gt 0 ]; then
    echo "RESULT FAIL step=pipeline reason=$SUMMARY"
    exit 4
fi
if [ "$BLOCKED_COUNT" -gt 0 ]; then
    echo "RESULT BLOCKED step=pipeline reason=$SUMMARY"
    exit 3
fi

echo "RESULT PASS step=pipeline reason=$SUMMARY"
exit 0
