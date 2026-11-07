# OMNI Kernel programme completion report (Batches 1 to 9)

This closes the nine-batch **experimental engineering programme**. It is not a claim that Exile OS is complete and not a claim that the
OMNI Kernel is production-ready. It is not production-ready. Test counts are those printed by the runners; the exact commands and
output are in `BATCH_9_VERIFICATION_REPORT.md`. Nothing was run under QEMU or on hardware.

## Per-batch summary

| Batch | Scope | Delivered | Tests (hosted) | Evidence | Integration status | Limitations |
|---|---|---|---|---|---|---|
| 1 | OMNI-ABI 0.1 specification | prose spec | none | review only | reference | draft; not machine-checked against prose |
| 2 | Wire format, handles, rights (Zig) | encode/decode, capability table, accounting, user window | Zig suite | passes, see report | constants cross-checked against Rust | model only |
| 3 | Capability lifecycle (Zig) | generation arena, revocation, accounting | Zig suite (includes Batch 2 suite when built) | passes | consumed by Batch 4/5 (Zig only) | model only |
| 4 | IPC core (Zig) | channels, bounded queues, capability transfer, cancellation | Zig suite | passes | Zig only | model only |
| 5 | Kernel IPC and proposed syscalls (Zig) | blocking waits, accounting ledger, syscall table 0x100..0x107 | Zig suite | passes | numbers cross-checked; IPC not in Rust | model only |
| 6 | Arch and scheduler contracts (Zig, SIM) | thread state machine, IPC-scheduler glue, SIM CPU | Zig suite | passes | contracts mirrored in Rust, no shared code | SIM only |
| 7 | Cooperative scheduler and syscall entry (Rust) | 16-thread scheduler, context-switch asm, SYSCALL stub, 5 syscalls, user-address model | 56 | passes (hosted, real asm on x86-64 host) | dependency of 8 and 9 | cooperative, uniprocessor, single syscall stack |
| 8 | Bare-metal image (Rust) | boot, GDT/TSS/IDT, W^X paging, frames, timer, ring-3 stage | 65 | hosted tests pass; image builds with host-target fallback; **never booted** | dependency of 9; not on `main` | QEMU NOT RUN; unpinned toolchain |
| 9 | Final integration | `PagedBacking`, 34 end-to-end and conformance tests, ABI checker, audit, architecture, security review, support matrix, this report | 34 plus checker (23 checks, 7/7 mutations) | passes in debug and release | overlay under `Meta/Experiments/OMNI-Kernel-Batch-9/` | hosted only |

## The nine questions

1. **What is genuinely implemented?** In Rust: a cooperative scheduler with real context switching (exercised on an x86-64 host), a
   SYSCALL entry stub and dispatcher for five syscalls, user-address validation, W^X page-table construction and translation, a
   frame allocator, descriptor and trap-frame encodings, and (Batch 9) page-table-backed user-memory access. All of it is verified by
   hosted tests.
2. **What has only been modelled?** Capabilities, rights, revocation, IPC channels and queues, capability transfer, resource accounting,
   and the scheduler/architecture contracts as Zig models (Batches 2 to 6). None is in the Rust kernel.
3. **Compiled but not executed?** The Batch 8 boot image, GDT/TSS/IDT loading, paging enablement, the timer, interrupt entry and the
   ring-3 stage: built for the host target with a fallback flow and statically checked, never run.
4. **Executed in QEMU?** Nothing. QEMU, GRUB, xorriso and KVM were unavailable; Batch 8's QEMU tests are written and report NOT RUN.
5. **Tested on actual hardware?** Nothing.
6. **What remains outside the experimental kernel?** The production kernel (`Kernel/`, `ExileKernel/`) is untouched and not linked.
   Devices, filesystems, networking, a user-space runtime, a loader, process management beyond threads and spaces, power management,
   and an ABI-stable syscall surface are all absent.
7. **Required before production integration?** Boot under QEMU with the pinned toolchain, then on hardware; resolve the syscall
   register convention with `Kernel/`; port or bind IPC and capabilities with tests against the models; per-thread kernel stacks;
   interrupt masking in locks; Mapper rollback; SMEP/SMAP/KPTI decisions; preemption and SMP design; independent security review;
   owner-approved placement under protected paths.
8. **What remains for ARM64?** Everything. Only constants in a Zig SIM contract mention it. No Rust, assembly, descriptors, vectors,
   timer, boot or tests exist. The architecture-neutral policy code (frame allocator, W^X checks, `PagedBacking` policy) is a starting point.
9. **Which security guarantees are not yet established?** Hardware enforcement of ring separation, W^X, NX and the null page;
   protection against speculative-execution attacks; safe concurrency under interrupts in the fallback image; safe blocking with a
   single syscall stack; capability and IPC security in the Rust kernel (it has none); behaviour under the pinned compiler. See
   `BATCH_9_SECURITY_REVIEW.md` (SEC-05, 12, 13, 14, 17 to 20).

## Not claimed

Production readiness; a complete operating system; preemption; SMP; ARM64; any QEMU or hardware result.
