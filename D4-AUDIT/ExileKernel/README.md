# Exile Universal Kernel — isolated x86-64 boot milestone

Place this directory at `ExileKernel/` alongside the existing Exile OS, never over `Kernel/`. This package does not change the OS image or build system. It is not an Exile userspace-capable kernel.

Requirements: checksum-verified Rust 1.91.1 with x86_64-unknown-none libraries, GNU gcc/binutils, Python 3; host Rust std for portable tests; GRUB 2 BIOS tools, xorriso and QEMU for emulator tests. No crates or OS headers are needed. See `docs/TOOLCHAIN.md` for exact archive checksums.

```
export EUK_RUSTC=/path/to/rust-1.91.1/bin/rustc
export EUK_SYSROOT=/path/to/rust-1.91.1
python3 scripts/test.py
python3 scripts/build.py
python3 scripts/boot_test.py
```

Optional tool overrides: EUK_HOST_RUSTC, EUK_CC, EUK_LD, EUK_QEMU, EUK_GRUB. GRUB needs xorriso in PATH and installed i386-pc modules. `build/` is generated and excluded from the source package. Normal images halt after serial diagnostics. Emulator test images deliberately use QEMU isa-debug-exit; its 33 success and 35 failure statuses are asserted by the harness, not shell success codes.

For reproducibility, run build.py with two different --output directories and compare the SHA-256 of euk.elf. Do not infer boot success from a build alone. See docs/VERIFICATION.md for actual results and limitations. The original EUK milestone source is released under Apache License 2.0. See LICENSE, NOTICE.md, and THIRD_PARTY.md; separate obligations may apply to third-party components and future binary distributions.
