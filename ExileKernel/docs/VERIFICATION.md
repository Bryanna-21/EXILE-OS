# Verification report — EUK first x86-64 milestone

Final regression run: 2026-10-20 UTC. Original reconstruction began 2026-10-19. Earlier temporary EUK sources were unavailable; the newly preserved sources and all tests were rebuilt. This package completes the isolated boot milestone, NOT the full operating system or an Exile-userspace-compatible kernel.

## Outcomes
- 18 portable memory-map/range tests: PASS, test harness exit 0.
- Static ELF64 x86-64 link, Multiboot header placement/checksum, entry range, no dynamic interpreter and no undefined symbols: PASS, scripts exit 0.
- Two non-test builds in different output paths: byte-identical, cmp exit 0.
- Kernel ELF SHA256 (both builds): 4bbba47cf88d4cbdbe64a53f9b770a4146e37af9162df77677ef3fd2e60d7d35.
- Real GRUB boot under QEMU TCG: normal pc and q35, plus dirty-table initialization regression: PASS.
- Six injected q35 rejection paths (magic, CPU features, missing/truncated map, invalid handoff bounds, oversized map): PASS with expected failure diagnostics, no BOOT_OK.
- rustfmt 1.91.1 --check: exit 0.
- Dependency/documentation/license-status gate: exit 0. This only checks review gates, NOT legal clearance.
- Existing web app-routing test: 1 passed, exit 0; unrelated to kernel correctness.
- Dedicated GitHub CI workflow provided, but not run on GitHub; no commit/push performed.

QEMU success exits 33 and failure exits 35 are intentional isa-debug-exit conventions. The Python harness checks them and exits 0 only for expected outcomes. These are not fabricated shell successes. Untested faults and platforms remain unverified.

## Exact final commands/environment
Run from the Exile OS parent folder containing ExileKernel:

```sh
export EUK_RUSTC=/tmp/euk-toolchain/sysroot/bin/rustc
export EUK_SYSROOT=/tmp/euk-toolchain/sysroot
export EUK_QEMU=/nix/store/z74wg6i612b95lsw06dsm9cy4f6s5jp2-qemu-10.1.2/bin/qemu-system-x86_64
export EUK_GRUB=/nix/store/94r96r9fmdzqyd3r58pslwrcm0agkf9a-grub-2.12/bin/grub-mkrescue
export PATH=/nix/store/bp7pz93i7s6fqg973gix2yiyr4xqglh3-libisoburn-1.5.6/bin:$PATH
/nix/store/7kq4df60y9c7pb473x7sgv8iyraf86l1-rustfmt-1.91.1/bin/rustfmt --edition 2024 --check ExileKernel/src/kernel.rs ExileKernel/src/memory.rs
python3 ExileKernel/scripts/test.py
python3 ExileKernel/scripts/audit.py
python3 ExileKernel/scripts/build.py --output /tmp/euk-repro/a
python3 ExileKernel/scripts/build.py --output /tmp/euk-repro/b
cmp /tmp/euk-repro/a/euk.elf /tmp/euk-repro/b/euk.elf
sha256sum /tmp/euk-repro/a/euk.elf /tmp/euk-repro/b/euk.elf
python3 ExileKernel/scripts/boot_test.py
```

All final commands above exited 0. Compiler/target archives were independently SHA256 checked against pinned values in TOOLCHAIN.md, then installed using install.sh --prefix=/tmp/euk-toolchain/sysroot --disable-ldconfig. GNU GCC 14.3.0 / binutils 2.44; QEMU 10.1.2 / GRUB 2.12 / xorriso 1.5.6.

The first reconstruction attempt used Rust --emit=obj with direct ld and failed on unresolved core panic routines. Fixed by producing a staticlib containing required Rust core/runtime support and linking with --gc-sections. Final link passes without suppressing errors. Review prompted explicit page-table zeroing, a dirty-table regression, bounded early serial polling, more failure boot tests and narrower map diagnostics. Overlap rejection remains deliberate fail-closed restricted-platform policy; legitimate overlapping firmware maps are unsupported until reserved-precedence normalization is implemented.

## Material final output

### Portable tests
```

running 18 tests
test tests::alignment_overflow ... ok
test tests::ceiling ... ok
test tests::empty_map ... ok
test tests::bounded_capacity ... ok
test tests::huge_record ... ok
test tests::adjacent ... ok
test tests::overflowing_range ... ok
test tests::overlap ... ok
test tests::page_alignment ... ok
test tests::short_record ... ok
test tests::extended_record ... ok
test tests::small_record ... ok
test tests::unordered ... ok
test tests::reserved_kind ... ok
test tests::valid ... ok
test tests::reserved_prefix ... ok
test tests::zero_range ... ok
test tests::short_header ... ok

test result: ok. 18 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

```

### Boot test serial logs

#### cpu/q35
```text
EUK FAIL: CPU features
```

#### handoff/q35
```text
EUK: x86-64 long mode
EUK FAIL: handoff bounds
```

#### magic/q35
```text
EUK FAIL: boot magic
```

#### map_length/q35
```text
EUK: x86-64 long mode
EUK FAIL: memory map bounds
```

#### map_missing/q35
```text
EUK: x86-64 long mode
EUK FAIL: memory map missing
```

#### map_truncated/q35
```text
EUK: x86-64 long mode
EUK FAIL: memory map invalid
```

#### normal/pc
```text
EUK: x86-64 long mode
EUK: bounded memory map parsed
EUK BOOT_OK
```

#### normal/q35
```text
EUK: x86-64 long mode
EUK: bounded memory map parsed
EUK BOOT_OK
```

#### table_dirty/q35
```text
EUK: x86-64 long mode
EUK: bounded memory map parsed
EUK BOOT_OK
```

## Files delivered and installation

All new files are under ExileKernel/ except .github/workflows/euk.yml. Extract source into the existing Exile OS repository top folder after review. Do not overwrite existing work; if either path already exists, merge manually. There are no changes to the reference OS/kernel/Store. ZIP contains source, documentation, a source hash manifest and CI only; no ELF, ISO, runtime binaries, downloaded dependencies, Git metadata or generated build folders. Local web metadata and workspace AGENTS/roadmap edits are not OS deliverables and are excluded.

## Limits and blockers

No full existing Exile OS compile/boot was performed. Exile userland/Store does not run on EUK; no existing kernel or default boot path was replaced. No AArch64 boot/compile, real hardware, scheduler, live page allocator, exceptions/interrupt/timer foundation, syscalls, IPC, drivers, storage/network/GUI, SMP, W^X, guard page, secure boot or Android support is implemented or verified. ISO reproducibility and cross-host byte identity are not claimed. See ARCHITECTURE, THREAT_MODEL, INTEGRATION and ROADMAP for the next gates.

The project owner has authorized Apache-2.0 licensing for the original EUK contribution. This does not certify third-party rights or clear every dependency for binary distribution. EUK remains experimental and is not represented as production-ready.
