#!/usr/bin/env python3
"""Build the isolated EUK AArch64 QEMU virt image; no implicit downloads."""
import os
import pathlib
import shutil
import subprocess
import sys

ROOT = pathlib.Path(__file__).resolve().parents[1]
OUT = pathlib.Path(sys.argv[1]).resolve() if len(sys.argv) > 1 else ROOT / "build-aarch64"
OUT.mkdir(parents=True, exist_ok=True)

def tool(env, default):
    value = os.environ.get(env) or shutil.which(default)
    if not value:
        raise SystemExit(f"missing required tool: set {env} or install {default}")
    return value

clang = tool("EUK_AARCH64_CC", "clang")
rustc = tool("EUK_AARCH64_RUSTC", "rustc")
linker = tool("EUK_AARCH64_LD", "ld.lld")
readelf = tool("EUK_AARCH64_READELF", "llvm-readelf" if shutil.which("llvm-readelf") else "readelf")

def run(args):
    print("+", " ".join(map(str, args)), flush=True)
    subprocess.run(list(map(str, args)), check=True, cwd=ROOT)

target = "aarch64-unknown-none"
asm_flags = ["--target=aarch64-none-elf", "-ffreestanding", "-fno-pic", "-c"]
run([clang, *asm_flags, "src/arch/aarch64/boot.S", "-o", OUT / "boot.o"])
run([clang, *asm_flags, "src/arch/aarch64/vectors.S", "-o", OUT / "vectors.o"])
run([rustc, "--version"])
run([rustc, "--edition=2024", "--target", target, "--crate-type", "staticlib",
     "-C", "panic=abort", "-C", "opt-level=2", "-C", "relocation-model=static",
     "-C", "no-redzone=yes", "src/arch/aarch64/main.rs", "-o", OUT / "main.a"])
run([linker, "-nostdlib", "--gc-sections", "--build-id=none",
     "-T", "src/arch/aarch64/linker.ld", OUT / "boot.o", OUT / "vectors.o",
     OUT / "main.a", "-o", OUT / "euk-aarch64.elf"])
run([readelf, "-h", "-l", OUT / "euk-aarch64.elf"])
print("AArch64 ELF linked. This is not a QEMU boot result.")
