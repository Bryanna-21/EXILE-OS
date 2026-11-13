#!/usr/bin/env bash
# Build the Batch 8 bare-metal kernel image.
#
# Pipeline (a documented FALLBACK, see docs/BOOT.md): the pinned bare-metal toolchain
# (rustc 1.91.1 + x86_64-unknown-none) is not installable offline, so this builds the no_std
# crates with whatever rustc is installed, for the host target, with settings that make the
# output freestanding (panic=abort, static relocation, no red zone), then links them with the
# assembly stubs using GNU ld and a custom linker script. When a bare-metal target is
# available, pass --target x86_64-unknown-none and the same flags apply.
#
# usage: build.sh [--variant base|ring3|preempt|ring3-fault|trap-fault|magic-fault|cpu-fault]
#                 [--test-exit] [--profile release|debug] [--out DIR] [--target TRIPLE]
# Output: DIR/kernel.elf (ELF64), DIR/kernel32.elf (ELF32 copy for qemu -kernel),
#         DIR/build-info.txt
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
B7="$ROOT/../OMNI-Kernel-Batch-7"
VARIANT=base
TEST_EXIT=0
PROFILE=release
OUT=""
TARGET=""
while [ $# -gt 0 ]; do
    case "$1" in
        --variant) VARIANT="$2"; shift 2 ;;
        --test-exit) TEST_EXIT=1; shift ;;
        --profile) PROFILE="$2"; shift 2 ;;
        --out) OUT="$2"; shift 2 ;;
        --target) TARGET="$2"; shift 2 ;;
        *) echo "build.sh: unknown argument: $1" >&2; exit 2 ;;
    esac
done
[ -d "$B7/kernel" ] || { echo "build.sh: Batch 7 sources not found next to Batch 8" >&2; exit 2; }
[ -n "$OUT" ] || OUT="$(mktemp -d "${TMPDIR:-/tmp}/omni8-build.XXXXXX")"
mkdir -p "$OUT"

RUSTCFG=(); ASMDEF=()
case "$VARIANT" in
    base) ;;
    ring3) RUSTCFG+=(--cfg omni8_ring3) ;;
    preempt) RUSTCFG+=(--cfg omni8_preempt) ;;
    ring3-fault) RUSTCFG+=(--cfg omni8_ring3 --cfg omni8_user_fault) ;;
    trap-fault) RUSTCFG+=(--cfg omni8_fault_trap) ;;
    magic-fault) ASMDEF+=(-DFAULT_MAGIC) ;;
    cpu-fault) ASMDEF+=(-DFAULT_CPU) ;;
    *) echo "build.sh: unknown variant: $VARIANT" >&2; exit 2 ;;
esac
if [ "$TEST_EXIT" = 1 ]; then RUSTCFG+=(--cfg test_exit); ASMDEF+=(-DTEST_EXIT); fi
case "$PROFILE" in
    release) OPT=(-C opt-level=2) ;;
    debug) OPT=(-C opt-level=0 -C debug-assertions=on) ;;
    *) echo "build.sh: unknown profile: $PROFILE" >&2; exit 2 ;;
esac

CHECKCFG=(--check-cfg 'cfg(test_exit)' --check-cfg 'cfg(omni8_ring3)' --check-cfg 'cfg(omni8_preempt)' --check-cfg 'cfg(omni8_user_fault)' --check-cfg 'cfg(omni8_fault_trap)')
COMMON=(--edition 2024 -C panic=abort -C relocation-model=static -C no-redzone=yes -C debuginfo=0
        -C codegen-units=1 -C embed-bitcode=no "${OPT[@]}" --remap-path-prefix "$ROOT=." --remap-path-prefix "$B7=../OMNI-Kernel-Batch-7"
        -L "dependency=$OUT")
[ -z "$TARGET" ] || COMMON+=(--target "$TARGET")

: > "$OUT/build-info.txt"
log() { printf '%s\n' "$*" | tee -a "$OUT/build-info.txt" >/dev/null; }
run() { log "\$ $*"; "$@"; }

log "variant=$VARIANT test_exit=$TEST_EXIT profile=$PROFILE target=${TARGET:-host-fallback}"
log "rustc: $(rustc -V)"
log "gcc: $(gcc --version | head -n1)"
log "ld: $(ld --version | head -n1)"
log "objcopy: $(objcopy --version | head -n1)"
if [ -z "$TARGET" ]; then
    log "NOTICE: built with the host-target fallback; the pinned bare-metal toolchain was not used."
fi

run rustc "${COMMON[@]}" --crate-type rlib --crate-name omni_kernel7 "$B7/kernel/lib.rs" --out-dir "$OUT"
run rustc "${COMMON[@]}" --crate-type rlib --crate-name omni8 --extern "omni_kernel7=$OUT/libomni_kernel7.rlib" \
    "$ROOT/src/lib.rs" --out-dir "$OUT"
run rustc "${COMMON[@]}" "${CHECKCFG[@]}" "${RUSTCFG[@]}" --crate-type staticlib --crate-name omni8_kernel \
    --extern "omni_kernel7=$OUT/libomni_kernel7.rlib" --extern "omni8=$OUT/libomni8.rlib" \
    "$ROOT/kernel/main.rs" --out-dir "$OUT"

for s in boot isr probe; do
    run gcc -m64 -c -ffreestanding -fno-pie -mno-red-zone -Wa,--noexecstack "${ASMDEF[@]}" \
        "$ROOT/arch/x86_64/$s.S" -o "$OUT/$s.o"
done

run ld -nostdlib -static --gc-sections --build-id=none -z noexecstack -z max-page-size=4096 \
    -T "$ROOT/link/linker.ld" -o "$OUT/kernel.elf" \
    "$OUT/boot.o" "$OUT/isr.o" "$OUT/probe.o" "$OUT/libomni8_kernel.a"
run objcopy -O elf32-i386 "$OUT/kernel.elf" "$OUT/kernel32.elf"
log "built: $OUT/kernel.elf"
echo "$OUT"
