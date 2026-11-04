# Batch 8 pre-implementation audit

Written before any Batch 8 code, from the repository at the head of `experiment/omni-kernel-batch-7`.
Nothing in this audit was executed on hardware or under QEMU.

## What exists

| Component | Language | State | What it is |
|---|---|---|---|
| `Documentation/OMNI-Kernel/Batch-1` | prose | specification | OMNI-ABI 0.1 draft |
| `Meta/Experiments/OMNI-Kernel-Batch-2` | Zig | hosted, tested | wire format, handle and rights checks |
| `…-Batch-3` | Zig | hosted, tested | capability lifecycle, revocation, accounting |
| `…-Batch-4` | Zig | hosted, tested | IPC core: channels, queues, capability transfer |
| `…-Batch-5` | Zig | hosted, tested | kernel-facing IPC interface and a proposed syscall boundary |
| `…-Batch-6` | Zig | hosted, tested | architecture and scheduler boundary contracts, SIM tests |
| `…-Batch-7` | Rust, `no_std`, x86-64 asm | hosted tests execute the real context-switch asm | cooperative scheduler, SYSCALL entry stub and dispatch, user-address validation |
| `ExileKernel/` | Rust, x86-64 asm | 517 lines; boot-tested upstream with QEMU | Multiboot v1 bootstrap, serial, memory map ingestion, ISA debug exit |
| `Kernel/` | C++ (Serenity-derived) | 1214 source files; not built or run here | full kernel with `Arch/{x86_64,aarch64,riscv64}`; uses its own boot path |

## Dependency map

```
Batch 1 spec ──► Batch 2 ──► Batch 3 ──► Batch 4 ──► Batch 5 ──► Batch 6      (Zig, hosted, no hardware)
                                                         │            │
                          proposes syscall register ABI ─┘            └─ defines arch/scheduler contracts
                                                                              │ (contracts only; no code shared)
Batch 7 (Rust) ◄── implements a cooperative scheduler + SYSCALL stub that satisfy those contracts
   │
   └──► Batch 8 (this) depends on Batch 7 as a Cargo path dependency and reuses, unchanged:
          sched::Sched, syscall::Kernel / omni_syscall_entry / PerCpu, msr::init_syscall, uaccess::UserBacking
        Batch 8 adds what Batch 7 assumed existed: boot, GDT/TSS/IDT, paging, frame allocator, timer, ring-3 entry.

ExileKernel/ ── same boot contract (Multiboot v1, 1 MiB, COM1, ISA debug exit), read for reference, not modified, not linked.
Kernel/      ── read, not modified, not linked.
```

The Zig batches (2 to 6) and the Rust batches (7, 8) share no code. Batch 8's only runtime link to them is the set of
numeric conventions: syscall numbers 0x1F0 to 0x1F7 (provisional) and the Batch 5 register convention
(rax number; rdi, rsi, rdx, r10, r8, r9 arguments), which Batch 7's stub implements.

## Findings that shaped Batch 8

1. **Batch 7 assumed a GDT, IDT, TSS, paging and a kernel stack that nothing provided.** Its stub reads `gs:0` for a
   kernel stack and returns with `sysretq` to selectors derived from STAR. Batch 8 supplies a GDT whose layout matches
   `msr::star_value` (checked by a test), a TSS, a per-CPU block and user mappings.
2. **ExileKernel cannot host this code yet.** Its entry stub identity-maps and jumps to Rust but has no IDT, TSS,
   allocator, timer or user segments. Batch 8 therefore ships its own bootstrap (`arch/x86_64/boot.S`) with the same
   boot contract instead of editing ExileKernel.
3. **Syscall register conventions disagree.** `Kernel/` uses rax with rdx, rdi, rbx, rsi; Batch 5/7 use the System V-like
   set. Not decided; Batch 8 keeps the Batch 7 convention and says so.
4. **The pinned toolchain is unavailable here.** ExileKernel pins rustc 1.91.1 with `x86_64-unknown-none`. This
   environment has rustc 1.97.0 for the host target only and no network. Batch 8 builds with a documented fallback
   (see `BOOT.md`).
5. **No QEMU, GRUB, xorriso or KVM here.** No boot has been run. Everything that needs one is marked NOT RUN.
6. **Batch 7's single per-CPU syscall stack** is safe for one thread that exits, which is what the ring-3 demo does. It is
   not safe for several user threads that can block in the kernel. Recorded as a limitation, not fixed here.
7. **Red zone.** Interrupts arrive on the running kernel stack unless a gate uses an IST. Every Batch 8 gate uses one, and
   the build disables the red zone as well.
8. **FPU/SSE.** Batch 7 does not save FPU/SSE state on a context switch. Batch 8 enables SSE for compiled code, saves it
   around every interrupt, and documents that threads do not get private FPU state (`SCHEDULER.md`).

## Files modified outside Batch 8

None. `Kernel/`, `ExileKernel/` and Batches 1 to 7 are unchanged; `scripts/verify.sh` checks this.
