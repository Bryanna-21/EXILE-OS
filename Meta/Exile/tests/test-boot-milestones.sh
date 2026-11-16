#!/usr/bin/env bash
# Host-side tests for check-boot-milestones.sh. No kernel build or QEMU required.
set -u

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"
CHECK="$ROOT/Meta/Exile/scripts/check-boot-milestones.sh"
PIPE="$ROOT/Meta/Exile/scripts/run-boot-pipeline.sh"
TMP="$(mktemp -d)" || exit 1
trap 'rm -rf "$TMP"' EXIT

pass=0
fail=0

ok() {
    pass=$((pass + 1))
    echo "PASS $1"
}

bad() {
    fail=$((fail + 1))
    echo "FAIL $1: $2"
}

for script in "$CHECK" "$PIPE"; do
    if bash -n "$script"; then
        ok "bash syntax: ${script##*/}"
    else
        bad "bash syntax: ${script##*/}" "syntax error"
    fi
done

cat > "$TMP/good.log" <<'LOG'
MM: Multiboot mmap: address=0x1000, length=100, type=1
MM: Physical page entries: 1000
CPU[0]: Supported features: x86_64
Scheduler[0]: idle loop running
Read system_mode: self-test
Activating 12 services in dependency order...
LOG

if bash "$CHECK" "$TMP/good.log" >"$TMP/output" 2>&1; then
    grep -q '^RESULT PASS step=milestones' "$TMP/output" &&
        ok "all six boot milestones pass" ||
        bad "all six boot milestones pass" "missing PASS result"
else
    bad "all six boot milestones pass" "$(cat "$TMP/output")"
fi

grep -q 'service activation initiated' "$TMP/output" &&
    ok "service activation is not mislabeled as readiness" ||
    bad "service activation label" "expected accurate label"

grep -vF 'Scheduler[0]: idle loop running' "$TMP/good.log" > "$TMP/missing.log" 
if bash "$CHECK" "$TMP/missing.log" >"$TMP/output" 2>&1; then
    bad "missing milestone is rejected" "unexpected success"
else
    grep -q '^RESULT FAIL step=milestones' "$TMP/output" &&
        ok "missing milestone is rejected" ||
        bad "missing milestone is rejected" "$(cat "$TMP/output")"
fi

cp "$TMP/good.log" "$TMP/panic.log"
echo "KERNEL PANIC: synthetic failure" >> "$TMP/panic.log"
if bash "$CHECK" "$TMP/panic.log" >"$TMP/output" 2>&1; then
    bad "panic marker is rejected" "unexpected success"
else
    grep -q '^RESULT FAIL step=panic' "$TMP/output" &&
        ok "panic marker is rejected" ||
        bad "panic marker is rejected" "$(cat "$TMP/output")"
fi

if bash "$CHECK" "$TMP/empty.log" >"$TMP/output" 2>&1; then
    bad "empty/missing logs are blocked" "unexpected success"
else
    grep -q '^RESULT BLOCKED step=logs' "$TMP/output" &&
        ok "empty/missing logs are blocked" ||
        bad "empty/missing logs are blocked" "$(cat "$TMP/output")"
fi

if bash "$PIPE" --toolchain Clang >"$TMP/output" 2>&1; then
    bad "Clang is rejected" "unexpected success"
else
    grep -q '^RESULT USAGE.*only-GNU' "$TMP/output" &&
        ok "unsupported Clang pipeline is rejected" ||
        bad "unsupported Clang pipeline is rejected" "$(cat "$TMP/output")"
fi

if bash "$PIPE" --help >"$TMP/output" 2>&1; then
    grep -q 'GNU/x86_64' "$TMP/output" &&
        ok "pipeline help is available" ||
        bad "pipeline help is available" "missing description"
else
    bad "pipeline help is available" "$(cat "$TMP/output")"
fi

echo
echo "RESULT $([ "$fail" -eq 0 ] && echo PASS || echo FAIL) pass=$pass fail=$fail"
[ "$fail" -eq 0 ]
