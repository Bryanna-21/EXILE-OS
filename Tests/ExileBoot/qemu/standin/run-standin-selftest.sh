#!/usr/bin/env bash
# Self-test of the QEMU runner's mechanics using stand-in UEFI applications (standin.c).
# *** This is NOT a boot test of the Exile prekernel. *** It proves only that: OVMF boots an image from the ESP,
# EFI ConOut text reaches the -serial capture, and the runner classifies VERIFIED / REJECTED / TIMEOUT (silent image).
# Needs: clang (>=15), lld-link, qemu-system-x86_64, OVMF.  Usage: run-standin-selftest.sh [ovmf-code.fd]
set -u
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
OVMF_ARG="${1:-/usr/share/OVMF/OVMF_CODE_4M.fd}"
CLANG="${CLANG:-$(command -v clang-20 || command -v clang)}"
LINK="${LLD_LINK:-$(command -v lld-link-20 || command -v lld-link)}"
[ -n "$CLANG" ] && [ -n "$LINK" ] || { echo "need clang and lld-link" >&2; exit 1; }
OUT="$(mktemp -d)"; trap 'rm -rf "$OUT"' EXIT
for m in 0 1 2; do
    "$CLANG" -target x86_64-unknown-windows -ffreestanding -fshort-wchar -mno-red-zone -fno-stack-protector \
        -DSTANDIN_MODE=$m -c "$HERE/standin.c" -o "$OUT/s$m.o" &&
    "$LINK" -subsystem:efi_application -entry:efi_main -nodefaultlib "$OUT/s$m.o" -out:"$OUT/s$m.efi" >/dev/null || { echo "build failed"; exit 1; }
done
# Source the runner for its functions. NO_OUTPUT and QEMU_EXITED are not exercised here.
set -- "$OUT/s0.efi" "$OVMF_ARG"
# shellcheck source=../run-qemu-boot-tests.sh
. "$HERE/../run-qemu-boot-tests.sh"
TIMEOUT="${QEMU_TEST_TIMEOUT:-25}"
run_case "stand-in prints verified line  -> VERIFIED" "$OUT/s0.efi" VERIFIED
run_case "stand-in prints failure line   -> REJECTED" "$OUT/s1.efi" REJECTED
# OVMF itself logs (BdsDxe ...) to serial, so a started-but-silent image is TIMEOUT, not NO_OUTPUT.
run_case "stand-in prints nothing        -> TIMEOUT" "$OUT/s2.efi" TIMEOUT
echo "stand-in self-test: $pass passed, $fail failed (mechanics only; not a prekernel boot test)"
[ "$fail" -eq 0 ]
