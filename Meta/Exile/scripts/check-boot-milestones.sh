#!/usr/bin/env bash
# Check boot milestones against one or more QEMU serial/debug logs.
set -u

if [ "$#" -lt 1 ]; then
    echo "Usage: ${0##*/} LOG [LOG ...]" >&2
    echo "RESULT USAGE step=args reason=no-log-files"
    exit 1
fi

TMP="$(mktemp)" || {
    echo "RESULT FAIL step=setup reason=mktemp-failed"
    exit 1
}
trap 'rm -f "$TMP"' EXIT

valid=0
for log in "$@"; do
    if [ -s "$log" ]; then
        cat "$log" >> "$TMP"
        valid=$((valid + 1))
    fi
done

if [ "$valid" -eq 0 ]; then
    echo "RESULT BLOCKED step=logs reason=no-readable-nonempty-log"
    exit 3
fi

failures=0
check() {
    local label="$1" pattern="$2"
    if grep -Eq -- "$pattern" "$TMP"; then
        printf 'PASS  %s\n' "$label"
    else
        printf 'FAIL  %s\n' "$label"
        failures=$((failures + 1))
    fi
}

echo "Boot milestone validation"

if grep -Eq 'KERNEL PANIC|Kernel panic|ASSERTION FAILED|VERIFICATION FAILED|Unhandled exception|Double fault|Triple fault' "$TMP"; then
    echo "FAIL  panic/assertion marker detected"
    echo "RESULT FAIL step=panic reason=kernel-failure-marker"
    exit 1
fi

check "bootloader memory handoff" 'MM: (Multiboot mmap|Multiboot module|EFI memory map)'
check "kernel memory initialization" 'MM: Physical page entries:'
check "CPU feature initialization" 'CPU\[0\]: Supported features:'
check "scheduler idle-loop startup" 'Scheduler\[[0-9]+\]: idle loop running'
check "system mode initialization" 'Read system_mode:'
check "service activation initiated" 'Activating [0-9]+ services in dependency order'

if [ "$failures" -gt 0 ]; then
    echo "RESULT FAIL step=milestones reason=$failures-milestone(s)-missing"
    exit 1
fi

echo "RESULT PASS step=milestones reason=all-6-milestones-observed"
exit 0
