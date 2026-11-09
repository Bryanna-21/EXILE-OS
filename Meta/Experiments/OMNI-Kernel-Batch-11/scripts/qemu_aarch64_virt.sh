#!/usr/bin/env bash
# Boot the ARM64 `virt` stub under QEMU and check its serial line. NOT RUN in the authoring environment (no QEMU, and the
# stub itself was never assembled). Exit: 0 pass, 1 fail, 3 NOT RUN (tool or image missing).
set -u
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
IMG="${1:-$HERE/../build/boot-aarch64.elf}"
QEMU="${OMNI11_QEMU_AARCH64:-qemu-system-aarch64}"
if ! command -v "$QEMU" >/dev/null 2>&1; then echo "NOT RUN: $QEMU not found"; exit 3; fi
if [ ! -f "$IMG" ]; then echo "NOT RUN: image $IMG missing (run scripts/build_aarch64.sh)"; exit 3; fi
OUT="$(timeout "${OMNI11_TIMEOUT:-20}" "$QEMU" -M virt -cpu cortex-a72 -m 128M -nographic -serial stdio -monitor none \
      -kernel "$IMG" 2>&1 | head -c 4096)"
echo "$OUT"
if printf '%s' "$OUT" | grep -qx 'EXILE-OS ARM64 virt: boot stub reached'; then echo "PASS"; exit 0; fi
echo "FAIL: expected serial line not seen"; exit 1
