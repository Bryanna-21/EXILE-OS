# Kernel completion programme: status by phase

Honest scope statement. The brief asks for a complete, QEMU-validated kernel across 16 phases. This session had no QEMU, GRUB, xorriso,
network, apt, or pinned Rust 1.91.1 / `x86_64-unknown-none`, and could not build `Kernel/` at all. So: one audit, one architecture proposal,
one baseline checker, and three small hosted-tested components. Everything else is listed as not done. "Source present" means files exist in
`Kernel/`; it is not a claim that they build or work.

| Phase | In `Kernel/` (source only) | In EUK / OMNI before this batch | Done in Batch 10 | Remaining |
|---|---|---|---|---|
| 0 Audit and decision | n/a | n/a | audit + `ARCHITECTURE_DECISION.md` (needs owner approval) | owner decision |
| 1 Boot and init | prekernel, Multiboot, per-arch boot | EUK boot milestone (QEMU-verified elsewhere); OMNI B8 image never booted | nothing | QEMU boot tests in a provisioned environment |
| 2 Memory management | MemoryManager, AddressSpace, regions, physical regions | OMNI B8 frame allocator, W^X Mapper | `txmap`: all-or-nothing mapping, table teardown (hosted) | guard pages, kernel heap, TLB, fragmentation monitoring, real-hardware tests |
| 3 CPU, interrupts | Processor, IDT, APIC/PIC, FPU/SIMD state | OMNI B8 GDT/IDT/TSS (never run) | `irqlock`: interrupt-masking lock protocol (hosted model) | execute under QEMU; per-CPU state; APIC |
| 4 Processes, threads, scheduler | Process, Thread, preemptive Scheduler with timer tick | OMNI B7 cooperative scheduler | nothing | preemption proof under QEMU; OMNI remains cooperative |
| 5 Syscalls, user mode | SyscallEntry, ~73 handlers | OMNI B7 five syscalls | `abi`: version query, register adaptor (hosted) | user-mode test program under QEMU; ABI adoption in `Kernel/` |
| 6 Capabilities and security | pledge and unveil syscalls (process-based restrictions), no OMNI-style capabilities | Zig models only | SEC status table | port capabilities, SMEP/SMAP/KPTI verification |
| 7 IPC | local sockets (`LocalSocket.cpp`), mmap-based sharing (source) | Zig models only | nothing | Rust or C++ implementation + tests |
| 8 Synchronisation | mutexes, spinlocks, wait queues (source) | OMNI SpinLock | `irqlock` only | audit for races, deadlock detection, lock-order doc |
| 9 Drivers | extensive (PCI, USB, VirtIO, storage, input, GPU, audio) | none | nothing | per-driver QEMU validation |
| 10 Storage and filesystem | block devices, Ext2, FAT, ISO9660 | none | nothing | hosted/QEMU tests of the interfaces |
| 11 Networking | adapters, IP, sockets | none | nothing | validation |
| 12 SMP | AP startup, TLB broadcast (source) | none (uniprocessor) | nothing | multi-core QEMU validation |
| 13 ARM64 | `Arch/aarch64`, `riscv64` trees | Zig SIM constants only | nothing | build and QEMU boot validation |
| 14 Diagnostics | logging, panic, coredump (source) | OMNI report module | nothing | structured logging, watchdog |
| 15 Build and test | CMake flow | `verify.sh` per batch | `check_toolchain.py` + pin file; B10 `verify.sh` | CI with pinned toolchain; QEMU test runner |
| 16 Integration | the system itself | none | decision document | stable interface definition once ABI is approved |

## Implemented in this batch

| Feature | Files | How | Tests | Result (hosted) |
|---|---|---|---|---|
| Transactional mapping, table teardown | `src/txmap.rs` | journal of page-table writes and a record of allocated frames, replayed in reverse on failure; capacity checked first | `tests/txmap.rs` (10) | 10/10 pass |
| Interrupt-masking lock | `src/irqlock.rs` | lock takes an `IrqControl`; saves state and disables, restores the saved state on unlock | `tests/irqlock.rs` (7) | 7/7 pass |
| ABI version and register adaptor | `src/abi.rs` | `AbiKernel` wraps the Batch 7 `Kernel`; `AbiMode::Exile` maps rdx, rdi, rbx, rsi to the OMNI argument registers | `tests/abi.rs` (7) | 7/7 pass |
| Toolchain pin check | `scripts/check_toolchain.py`, `toolchain.pin` | compares versions and tools; mismatch exits 3 | run by `verify.sh` | reports 7 problems here (see report) |

## Blocking dependencies

1. A provisioned environment: Rust 1.91.1 with `x86_64-unknown-none`, QEMU (x86-64 and aarch64), GRUB 2 and xorriso, a C++ cross toolchain for `Kernel/`.
2. Owner approval of the architecture decision and the syscall ABI.
3. For `Kernel/` changes: approval to touch protected paths.

## Security risks outstanding

See `SECURITY_STATUS.md`. Nothing here establishes hardware isolation, preemption, SMP safety, or production readiness.

## Next milestone

In a provisioned environment: (1) build `Kernel/` and run `Tests/Kernel` under QEMU for a real baseline; (2) re-run EUK's boot tests and the Batch 8 image under QEMU; (3) decide the ABI; (4) only then choose which OMNI components to port.

## Git safety

| Item | Value |
|---|---|
| Starting commit | `686eecaf6598f34c84b06f783f508ab35f764398` (`origin/main`), with the Batch 8 and Batch 9 overlays present but untracked |
| Working branch | none: detached worktree, nothing committed |
| Files changed | none tracked; one new untracked directory `Meta/Experiments/OMNI-Kernel-Batch-10/` (see `manifest.json`) |
| Protected paths | `Kernel/`, `ExileKernel/`, `Documentation/`, Batches 2 to 9 untouched (checked by `verify.sh`) |
| Pushed / merged | nothing |
