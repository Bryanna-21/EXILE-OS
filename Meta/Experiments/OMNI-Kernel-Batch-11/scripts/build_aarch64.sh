#!/usr/bin/env bash
# Build the ARM64 `virt` stub with Zig. Requires a Zig with LLVM extensions (the authoring Zig had none and failed to
# assemble the stub). Output: build/boot-aarch64.elf. Exit: 0 built, 1 failed, 3 NOT RUN (zig missing).
set -u
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ZIG="${ZIG:-zig}"
command -v "$ZIG" >/dev/null 2>&1 || { echo "NOT RUN: zig not found"; exit 3; }
mkdir -p "$HERE/../build"
"$ZIG" build-exe "$HERE/../arch/aarch64-virt/boot.zig" -target aarch64-freestanding -mcpu=cortex_a72 -O ReleaseSmall \
  -T "$HERE/../arch/aarch64-virt/link.ld" -femit-bin="$HERE/../build/boot-aarch64.elf" || exit 1
echo "built $HERE/../build/boot-aarch64.elf"
