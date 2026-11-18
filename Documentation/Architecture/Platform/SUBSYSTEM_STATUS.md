# Kernel subsystem status matrix (WO2)

Grounded in the repository at the base of branch `validation/orchestrator` (f51661db), in
`Meta/Experiments/OMNI-Kernel-Batch-11/docs/GAP_MATRIX.md`, and in the new `Meta/Exile/platform` crate.
Nothing in this document was verified by booting anything: **no QEMU, no `x86_64-unknown-none` toolchain and no
pinned Rust 1.91.1 are available in the authoring environment** (see `Meta/Exile/build/README.md`).

Vocabulary: **IMPLEMENTED-HOSTED** (code exists, host unit tests ran and passed) · **PARTIAL** · **PLANNED** · **BLOCKED**.
None of these means "works on a machine". The column *Executable evidence* says exactly what ran.

| Subsystem | Status | Where | Executable evidence | Not proven / gap |
|---|---|---|---|---|
| Boot memory map | IMPLEMENTED-HOSTED (parsers) | `platform/src/hal/memmap.rs`; Batch 8 frame allocator | 10 host tests, fixtures | Never fed a real Multiboot/UEFI map |
| Page tables / W^X | IMPLEMENTED-HOSTED | OMNI Batch 8 `Mapper`, Batch 10 `txmap` | Batch 8/10 hosted tests (per their reports) | Never executed with a real MMU |
| IDT / exceptions | PARTIAL | Batch 8 `arch/x86_64`, `kernel/main.rs` | Static checks (`check_image.py`, `check_asm.py`) | Image never booted; QEMU fault variants NOT RUN |
| CPU init / features | PARTIAL | `platform/src/hal/cpu.rs`; Batch 8 GDT/TSS | CPUID decode run on the host's virtual CPU + fixtures | Per-core init: PLANNED. SMEP/SMAP/NX *enabling* not in this crate |
| Local APIC / timer | PARTIAL (register logic) | `platform/src/hal/lapic.rs` | Mock register-file tests (ordering, encodings) | Never touched a LAPIC; timer calibration not implemented |
| Per-core data / SMP | PLANNED | `sync.rs` has lock primitives only | — | No AP bring-up, no per-CPU storage. IPI *encoding* exists (`lapic.rs`) but is untested against hardware |
| Scheduler / context switch | PARTIAL | Batch 7 `sched.rs`/`context.rs` (cooperative, uniprocessor); `Kernel/` has preemptive SMP | Batch 7 hosted tests | Not preemptive in OMNI; context switch asm never executed |
| Ring 3 / syscalls | PARTIAL | Batch 7 `syscall.rs`/`uaccess.rs`, Batch 8 `ring3` variant, Batch 11 `sys.rs` | Hosted tests; ring3 images built only for the host target | Never ran in ring 3; MSR setup unexecuted |
| Process isolation | PARTIAL | Batch 9 `PagedBacking` (region model ∧ page-walk), Batch 11 caps/IPC | Hosted e2e tests | **No claim of isolation**: hardware enforcement never executed |
| Capabilities / IPC | IMPLEMENTED-HOSTED | Batch 11 `caps.rs`, `ipc.rs` | 18 hosted tests (per Batch 11 report) | Not wired to any boot image |
| Synchronization | PARTIAL | `platform/src/sync.rs` (TicketLock, RankStack); Batch 10 `irqlock` | Real host-thread contention test, mutation-checked | Not interrupt-safe by itself; never run in-kernel or with kernel scheduler |
| Panic / diagnostics | PARTIAL | Batch 8 exception reports; QEMU fault variants | Static only | Fault variants (trap/magic/cpu-fault) NOT RUN |
| Driver/service interfaces | PARTIAL | `platform/src/hal/registry.rs`, `devices.rs` traits | Mock-driver lifecycle tests | No real driver implements them |
| Boot handoff | IMPLEMENTED-HOSTED | `platform/src/boot/info.rs` | 5 tests incl. every single-bit flip detected | No bootloader produces or consumes it yet |
| Safe update / rollback | IMPLEMENTED-HOSTED (state machine) | `platform/src/boot/slots.rs` | Torn write simulated at every byte offset | Real ESP/flash semantics untested |

## Unsafe boundaries (WO2: every one documented)

| Location | Why unsafe | Invariant relied on |
|---|---|---|
| `platform/src/sync.rs` `unsafe impl Sync/Send for TicketLock<T>` | Interior mutability shared across threads | Ticket protocol grants at most one live `Guard`; `T: Send` |
| `sync.rs` `Guard::deref` / `deref_mut` | Raw pointer deref of `UnsafeCell` | Guard is sole holder; `&mut self` prevents aliasing through the guard |
| `platform/src/hal/cpu.rs` `__cpuid_count` | Intrinsic is `unsafe` on some toolchains | CPUID is always available on x86_64; `#[allow(unused_unsafe)]` documented |
| OMNI Batches 7/8/10 | Inline asm, MSR writes, page-table writes, port I/O | Documented per file in those batches; not re-audited here |

Everything else in `exile_platform` is safe Rust (`unsafe_op_in_unsafe_fn = deny`).
Arch-specific code (`HardwareCpuid`, the intrinsic) is isolated behind `#[cfg(target_arch = "x86_64")]`; all
parsers/decoders are portable `no_std`, no-alloc and take traits (`Mmio`, `ConfigAccess`, `CpuidSource`) so they can be mocked.

## Blocked items

1. Booting anything (no QEMU / GRUB / xorriso / pinned toolchain here).
2. SMP and per-core bring-up (needs QEMU `-smp` evidence first).
3. Isolation, scheduling and protection claims: require executed ring-3 fault tests.
