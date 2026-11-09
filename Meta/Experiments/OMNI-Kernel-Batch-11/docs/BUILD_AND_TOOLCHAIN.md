# Build and toolchain

Pinned versions are in `../OMNI-Kernel-Batch-10/toolchain.pin`: rustc 1.91.1, target `x86_64-unknown-none`, Zig 0.15.2; QEMU (x86-64 and aarch64),
`grub-mkrescue` and `xorriso` for boot tests. This batch was verified with rustc 1.97.0 on the host target and Zig 0.15.2 (built without LLVM), so results are
**not** evidence for the pinned compiler.

## Linux or WSL2 (Ubuntu 24.04)

```
sudo apt-get install -y build-essential python3 qemu-system-x86 qemu-system-arm grub-pc-bin grub-common xorriso mtools
rustup toolchain install 1.91.1 -t x86_64-unknown-none -c clippy,rustfmt
# Zig 0.15.2 from ziglang.org, put on PATH or set ZIG=/path/to/zig
cd Meta/Experiments/OMNI-Kernel-Batch-11 && scripts/verify.sh
```

The hosted tests execute real x86-64 context-switch assembly, so they need an x86-64 Linux host (WSL2 on x86-64 qualifies; a native Windows shell does not;
ARM64 hosts cannot run them). Use external build directories (`CARGO_TARGET_DIR`, `OMNI11_ZIG_CACHE`) so nothing is written inside the repository.
These instructions were written from the scripts' requirements; the Windows/WSL path was not tried.

## Static check of `Kernel/`

`scripts/kernel_syntax_check.sh [REPO_ROOT] [files]` (`KARCH=x86_64|aarch64`) runs `clang++ -fsyntax-only` over `Kernel/` translation units with generated configuration
headers (all optional debug switches off, `SANITIZE_PTRS` on). It needs clang 18 or newer and libstdc++ headers, uses the host (not the project) toolchain, skips the two
kernel-coverage files and the files that need LibVT's generated state machine, and says so. Exit 0 all pass, 1 a failure, 3 NOT RUN. `verify.sh` runs it only when
`OMNI11_KERNEL_SYNTAX="x86_64 aarch64"` is set.

## Verification environment variables

`OMNI_RUSTC_EXPECTED`, `ZIG`, `OMNI11_KERNEL_SYNTAX`, `CARGO_TARGET_DIR`, `OMNI11_ZIG_CACHE`, `OMNI11_SKIP_B8_VERIFY`, `OMNI11_REQUIRE_PINNED`, `OMNI_BASE_REF`.
Exit status of `verify.sh`: 0 all executed checks passed, 1 a check failed, 2 a required tool is missing. SKIPPED is never counted as passed.
