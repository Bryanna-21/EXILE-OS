#!/usr/bin/env bash
# QEMU tamper tests for the EFI prekernel.   *** STATUS: NOT YET EXECUTED ***
#
# This script has never been run: it needs a Kernel.efi built with ENABLE_BOOT_VERIFICATION=ON, and the OS build
# could not be performed in the environment where it was written (see PHASE_ONE_COMPLETION_REPORT.md, blockers).
# Treat every result it produces as the first real run and review it before relying on it.
#
# What it does, for the Kernel.efi you give it:
#   1. boots the unmodified image under QEMU + OVMF (valid manifest: expect the "verified" message),
#   2. flips one byte of the embedded manifest (expect a verification panic, no further boot output),
#   3. flips one byte of the embedded kernel payload (expect a verification panic).
# "Mismatched seed" and "invalid trust anchor" are build-time scenarios (check-keys refuses the build); they are
# covered by test-boot-integrity.sh and are not repeated here.
#
# QEMU is not a Secure Boot environment for this test (OVMF runs with its default, non-enforcing variables), so a pass proves
# only that the prekernel's own verification logic refuses tampered images. It says nothing about UEFI Secure Boot or
# real hardware.
#
# Usage: run-qemu-boot-tests.sh <Kernel.efi> [ovmf-code.fd]
#
# Expected serial output (from Kernel/EFIPrekernel/BootVerification.cpp):
#   success: "Kernel image verified against the signed boot manifest"
#   failure: "Kernel image verification failed: <reason>"  (PANIC; boot must not continue past it)
set -u
EFI="${1:?usage: $0 <Kernel.efi> [ovmf-code.fd]}"
OVMF="${2:-/usr/share/OVMF/OVMF_CODE_4M.fd}"
for f in "$EFI" "$OVMF"; do [ -f "$f" ] || { echo "missing file: $f" >&2; exit 1; }; done
command -v qemu-system-x86_64 >/dev/null || { echo "qemu-system-x86_64 not found" >&2; exit 1; }
command -v timeout >/dev/null || { echo "timeout not found" >&2; exit 1; }

WORK="$(mktemp -d)"; trap 'rm -rf "$WORK"' EXIT
pass=0; fail=0

# run_case <name> <efi-file> <must-contain> <must-not-contain>
run_case() {
    name="$1"; efi="$2"; want="$3"; unwanted="$4"
    mkdir -p "$WORK/esp/EFI/BOOT"; cp "$efi" "$WORK/esp/EFI/BOOT/BOOTX64.EFI"
    : >"$WORK/serial.txt"
    timeout 120 qemu-system-x86_64 -machine q35 -m 512 -display none -no-reboot \
        -drive if=pflash,format=raw,readonly=on,file="$OVMF" \
        -drive format=raw,file=fat:rw:"$WORK/esp" \
        -serial file:"$WORK/serial.txt" >/dev/null 2>&1
    if grep -q "$want" "$WORK/serial.txt" && { [ -z "$unwanted" ] || ! grep -q "$unwanted" "$WORK/serial.txt"; }; then
        echo "PASS $name"; pass=$((pass+1))
    else
        echo "FAIL $name (wanted '$want'${unwanted:+, forbidden '$unwanted'}); serial tail:"; tail -5 "$WORK/serial.txt" | sed 's/^/    /'; fail=$((fail+1))
    fi
}

# flip_after_marker <in> <out> <hex-marker> <offset-from-marker>: flips one byte at marker+offset; the marker must occur exactly once.
flip_after_marker() {
    python3 -I - "$1" "$2" "$3" "$4" <<'PY'
import sys
data = bytearray(open(sys.argv[1], "rb").read())
marker = bytes.fromhex(sys.argv[3])
i = data.find(marker)
if i < 0 or data.find(marker, i + 1) >= 0:
    sys.exit("marker %s not found exactly once" % sys.argv[3])
data[i + int(sys.argv[4])] ^= 0xFF
open(sys.argv[2], "wb").write(data)
PY
}

run_case "valid image verifies" "$EFI" "Kernel image verified against the signed boot manifest" "Kernel image verification failed"
# 45584c42 0200 0000 = manifest magic 'EXLB' + version 2 + flags 0 (the magic alone also appears in the verifier's code);
# offset 40 is inside the SHA-256 digest field.
flip_after_marker "$EFI" "$WORK/tampered-manifest.efi" "45584c4202000000" 40 || { echo "FAIL could not build tampered-manifest image"; exit 1; }
run_case "modified manifest is rejected" "$WORK/tampered-manifest.efi" "Kernel image verification failed" "Kernel image verified against"
# The embedded kernel starts with the ELF magic; byte 100 is inside the ELF header/program headers region.
flip_after_marker "$EFI" "$WORK/tampered-kernel.efi" "7f454c46" 100 || { echo "FAIL could not build tampered-kernel image"; exit 1; }
run_case "modified kernel payload is rejected" "$WORK/tampered-kernel.efi" "Kernel image verification failed" "Kernel image verified against"

echo "qemu boot tests: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
