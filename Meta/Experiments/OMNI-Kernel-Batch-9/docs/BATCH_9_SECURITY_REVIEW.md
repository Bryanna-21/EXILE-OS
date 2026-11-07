# Batch 9 security review

Scope: the integrated experimental foundation (Batches 2 to 9). This is a focused review of source and hosted tests by the
author of the batch, not an independent audit. **No guarantee of isolation is established**: nothing here ran under QEMU or on hardware,
and hosted tests are not proof of hardware isolation.

Classification meanings: **FIXED** a defect or gap existed and this batch closed it, with a test. **MITIGATED** a control exists and
is tested at the stated level (the level is always named). **UNRESOLVED** a real weakness remains. **NOT APPLICABLE** the code does not
exist in the Rust kernel. A model-only control is never described as protecting the Rust kernel.

Test ids: `mem_NN`, `sys_NN`, `cf_NN`, `conc_NN` are in `tests/` of this package; `B7 sch_/ctx_/ua_` are in `OMNI-Kernel-Batch-7/kernel/tests.rs`.

| Id | Area | Class | Evidence and limits |
|---|---|---|---|
| SEC-01 | Capability forgery and stale handles | NOT APPLICABLE to the Rust kernel; MITIGATED in the Zig model | The Rust kernel has no capabilities. In the Zig model handles carry a generation, generation 0 is invalid, and slots are retired rather than reused after generation exhaustion (`B3/src/gen_arena.zig`, B2/B3 tests). Hosted model only. |
| SEC-02 | Unauthorized object access | NOT APPLICABLE (Rust); MITIGATED in model | Rights checks live in B2/B3 Zig. Not reachable from Rust syscalls: unknown numbers return `NO_SYS` (`sys_01`). |
| SEC-03 | Incorrect ownership transfer | MITIGATED in model only | B4 hold spaces make in-flight capabilities owned and revocable (`B4/src/ipc.zig` header, B4 tests). No Rust equivalent. |
| SEC-04 | Use-after-free and stale references | MITIGATED for thread ids; FIXED for user spaces | Stale thread ids rejected (`sys_08`, B7 `ctx_08`). Before B9, nothing tied a destroyed space to page-table state; now a destroyed or unbound space refuses all access (`mem_10`). Physical frames are not reclaimed by `PagedBacking`; ownership stays with the caller (see SEC-14). |
| SEC-05 | Interrupt masking in `SpinLock` (host-target fallback image) | **UNRESOLVED** | B7 `irq_save`/`irq_restore` are compiled only when `target_os = "none"`. The B8 fallback image is built for the host target, so a lock taken in thread context is not protected from an interrupt handler on the same CPU. Belongs to B7/B8, which Batch 9 must not modify. Proposal in `PROPOSALS.md` P-01. |
| SEC-06 | Double destruction | MITIGATED (threads, spaces); model only for objects | `terminate_space(0)` refused (`sys_05`); kill is sticky and idempotent (B7 `sch_10`); a destroyed space cannot be reused for access (`mem_10`). Exactly-once object destruction exists only in the Zig model (B3). |
| SEC-07 | Memory-boundary violations (user copies) | FIXED (gap) and MITIGATED | B7 validated only its own region model. `PagedBacking` requires the page tables to agree, for every page of the range, before copying anything (`mem_01..mem_07`, `mem_12`). Writes are all-or-nothing (`mem_13`). Hosted: the "page tables" are a software memory (`SimPhys`), not the MMU. |
| SEC-08 | Integer overflow | MITIGATED | `checked_add` on `addr + len`, rejection above `USER_TOP` before any walk (`mem_03`, `mem_04`); sleep of `u64::MAX` rejected (`sys_06`). Zig side uses checked arithmetic and `Overflow` (B2). Not fuzzed. |
| SEC-09 | IPC resource exhaustion | NOT APPLICABLE (Rust); MITIGATED in model | Bounded queues, byte accounting and quotas in B2/B4/B5 Zig tests. The Rust kernel has no IPC. |
| SEC-10 | Unbounded allocation | MITIGATED | The Rust crates are `no_std` with fixed tables: 16 threads, 8 bindings, 16-entry exit log; exhaustion returns errors (`sys_08`, `mem_11`, `mem_17`). Hosted tests allocate freely, which is test code only. |
| SEC-11 | Invalid syscall arguments | MITIGATED | Unknown numbers, debug calls when disabled, bad addresses and length limits all return defined statuses (`sys_01`, `sys_02`, `mem_03`, `mem_04`, `sys_10`). |
| SEC-12 | User/kernel boundary | MITIGATED in hosted logic; **UNRESOLVED** at hardware level | Frames returning to the kernel half kill the thread (`sys_07`); supervisor pages never user-accessible through `PagedBacking` (`mem_08`); entry stub uses `sysretq` and not `iretq` (`cf_04`). Whether the CPU actually enforces rings, W^X, NX and the null page is **not executed**. No SMEP, SMAP, UMIP or KPTI. |
| SEC-13 | Scheduler state corruption | MITIGATED (hosted); **UNRESOLVED** (single syscall stack) | State machine, kill, terminate and exhaustion are tested (`sys_04..sys_08`, B7 `sch_*`). B7 has one per-CPU syscall stack, unsafe if several user threads block in the kernel. Cooperative and uniprocessor only; not safe under preemption or SMP. |
| SEC-14 | Incorrect cleanup after failure | **UNRESOLVED** (Mapper) / MITIGATED (backing) | `Mapper` can leave allocated intermediate table frames after a failed mapping; no visible mapping remains but frames leak (`mem_16` documents this). A refused user copy zeroes the output buffer (`mem_14`). Proposal P-02. |
| SEC-15 | Locking and concurrency assumptions | MITIGATED (hosted); assumption stated | `SpinLock` and `PagedBacking` exercised from real OS threads (`conc_01`, `conc_02`). The kernel assumes one CPU; see SEC-05 for interrupts. Hosted OS-thread concurrency is not SMP kernel concurrency. |
| SEC-16 | Information leakage via errors and logs | MITIGATED (partly) | Statuses are coarse codes (`sys_10`); debug syscalls (checksum, fill, args) are off by default (`sys_02`); unmapped memory reads as an error, and refused copies zero the destination (`mem_14`). The B8 boot image prints register state and diagnostics to the serial port by design, which would be an information channel in any non-test build. |
| SEC-17 | Hardware mitigations absent | **UNRESOLVED** | No SMEP/SMAP/UMIP, KPTI, ASLR, stack protector, guard pages (B8 `docs/SECURITY.md`). |
| SEC-18 | No ABI version query in the Rust syscalls | **UNRESOLVED** | The Zig wire format carries `abi_version`; the Rust syscalls have no equivalent. Proposal P-03. |
| SEC-19 | Pinned toolchain not used | **UNRESOLVED** (environment) | Rust 1.91.1 and `x86_64-unknown-none` unavailable; results are from 1.97.0 on the host target. Behaviour of the pinned compiler is unverified. |
| SEC-20 | Syscall register convention differs from `Kernel/` | **UNRESOLVED** (owner decision) | B5/B7 use rax, rdi, rsi, rdx, r10, r8, r9; `Kernel/` uses rax, rdx, rdi, rbx, rsi. Checked identical across Zig and Rust (`check_abi_conformance.py`); not reconciled with `Kernel/`. |

## Summary

- FIXED: 1 gap (SEC-07, plus the destroyed-space part of SEC-04).
- UNRESOLVED: SEC-05, SEC-12 (hardware), SEC-13 (syscall stack), SEC-14, SEC-17, SEC-18, SEC-19, SEC-20.
- Model-only controls (SEC-01, 02, 03, 09) say nothing about the Rust kernel until it is ported or bound.

## Not reviewed

DMA, devices, firmware, timing and speculative side channels, hardware errata, anything needing a booted system.
