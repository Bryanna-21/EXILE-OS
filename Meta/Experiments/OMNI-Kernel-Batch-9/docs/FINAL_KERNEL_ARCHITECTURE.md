# OMNI Kernel: final experimental architecture (Batches 1 to 9)

This describes what the source and tests in the repository establish. It is an **experimental foundation, not a
production kernel**, and it does not replace `Kernel/` or `ExileKernel/`. Paths below are relative to
`Meta/Experiments/`; `B2`..`B9` abbreviate `OMNI-Kernel-Batch-N`. "Test" names refer to hosted tests. Nothing here was executed
under QEMU or on hardware.

## Overview

```
        Zig, hosted models (B2..B6)                         Rust no_std, x86-64 (B7..B9)
 ┌───────────────────────────────────────┐        ┌──────────────────────────────────────────────┐
 │ B2 wire format, handles, rights       │        │ B9 PagedBacking  (integration, this batch)   │
 │ B3 capability lifecycle, accounting   │        │    │ implements B7 UserBacking                │
 │ B4 IPC core: channels, queues         │        │    │ walks B8 page tables (translate)         │
 │ B5 KernelIpc + proposed syscalls      │        │ B8 desc/idt/paging/frames/usermode/boot image│
 │ B6 scheduler + arch contracts (SIM)   │        │ B7 Sched, Kernel::syscall, Spaces, ctx switch│
 └───────────────────────────────────────┘        └──────────────────────────────────────────────┘
        no code shared with the Rust side:  only numeric conventions, checked by B9
        scripts/check_abi_conformance.py
```

## 1. Architecture boundaries

- Hosted Zig models (B2 to B6) define the capability, IPC and scheduler *semantics*. They run as test binaries only.
- Rust B7 implements the cooperative scheduler, the SYSCALL entry stub and user-address validation; B8 adds boot, descriptors,
  paging and ring-3 entry; B9 joins B7 to B8's page tables.
- There is no code path from the Rust kernel to the Zig models. The only shared artefacts are numbers (status codes, syscall
  numbers, register order, `USER_TOP`, page size), cross-checked by `B9/scripts/check_abi_conformance.py`.
- `Kernel/` (Serenity-derived) and `ExileKernel/` are untouched and not linked.

## 2. Memory management interfaces

- B8 `frames::FrameAllocator`: bitmap allocator over usable ranges; `paging::FrameSource` trait lets `Mapper` ask for frames.
- B8 `paging::Mapper`: builds 4-level tables, refuses W+X, enforces kernel/user window discipline; `translate` walks tables and
  returns phys address, page size, user bit, write bit. Tests: `B8/tests/{frames,paging}_tests.rs`.
- B7 `uaccess::UserBacking`: byte-level `read`/`write` per space; B9 `PagedBacking` is the first implementation backed by real
  page-table walks (`B9/src/lib.rs`; tests `mem_01..mem_17`).
- Known gap: `Mapper` can leak intermediate table frames when allocation fails part-way (test `mem_16` documents it).

## 3. Address-space interfaces

- B7 `Spaces`: `create`, `get_mut(id).map(start, len, Perm)`, `destroy`, `copy_from_user`; `USER_TOP = 0x0000_8000_0000_0000`.
- B9 `PagedBacking::bind(space, root)` ties a B7 space id to a page-table root (nonzero, page-aligned, below `PHYS_LIMIT`; at most 8
  bindings, errors `BadRoot`/`Exists`/`Full`). An unbound or destroyed space fails every access (tests `mem_10`, `mem_11`).
- B6 `user_memory.zig` is the SIM counterpart; not shared.

## 4. Thread and scheduler model

- B7 `sched.rs`: 16 threads, cooperative, uniprocessor. States `ThreadState`; operations `spawn`, `yield_now`, `sleep_ticks`,
  `block`/`wake`/`cancel`, `kill`, `terminate_space`, `exit_current`, `tick`, `run(idle)`. Exit codes via a 16-entry log.
- B6 `sched.zig` and `thread_state.zig` are the model; B7 follows the same state machine by design but not by shared code.
- Tests: B7 `kernel/tests.rs`, B9 `sys_04..sys_08`.

## 5. Context-switch mechanism

- B7 `kernel/arch/x86_64/context.S` saves callee-saved registers and switches stacks; `context.rs` wraps it. Hosted tests execute
  the real assembly on the host CPU (x86-64 hosts only).
- FPU/SSE state is not per-thread (B8 `docs/SCHEDULER.md`).
- B9 `sys_04` runs three spaces with real switches.

## 6. Syscall entry and dispatch

```
 user (ring 3) ─SYSCALL→ syscall_entry.S (swapgs, kernel stack from gs:0, build SyscallFrame 128 B)
                         → Kernel::syscall(&mut frame) → validate_frame → dispatch(nr)
                         → status in rax → sysretq   (never iretq on this path; test cf_04)
```

- Numbers: 0x1F0 YIELD, 0x1F1 EXIT, 0x1F2 SLEEP, 0x1F3 THREAD_SELF, 0x1F4 SPACE_SELF; 0x1F5..0x1F7 debug (off by default).
  Anything else returns `NO_SYS` (-33), including B5's IPC numbers 0x100..0x107 (tests `sys_01`, `cf_05`).
- Calling outside thread context returns `INVALID_STATE` (`sys_03`).
- A frame that would return into the kernel half kills the thread (`sys_07`, B7 `validate_frame`).

## 7. Capability model

- Specified in B1; modelled in B2/B3: 32-bit `Handle` = (generation << 16) | index, generation 0 invalid; `Rights` is a packed
  u32 with 9 defined bits and reserved bits that must be zero; `GenArena` retires a slot instead of wrapping its generation.
- Not implemented in the Rust kernel. See Support matrix.

## 8. IPC transport and message format

- B2 `core/ipc.zig`: 32-byte header (`abi_version` 0x0001, op, flags=0, kind send/call/reply, request id, handle count);
  limits 4096 bytes, 8 handles; decode never allocates and never indexes outside the buffer.
- B4: channels with two ends, bounded queues, hold spaces so in-flight authority is owned and revocable; B5 adds blocking waits and
  a proposed syscall surface (`Nr` 0x100..0x107).
- Not implemented in the Rust kernel.

## 9. Object lifetime and ownership

- Zig side: exact-once destruction through the B3 derivation tree; in-flight capabilities are owned by the receiving end's hold space
  (B4 header comment). Stale references are rejected by generation checks (B3 `gen_arena.zig`).
- Rust side: `Spaces::destroy` and `Sched::terminate_space` clean threads; B9 `sys_05` and `mem_10` verify no access survives. Frame
  ownership belongs to the caller of `Mapper`/`FrameAllocator` (no automatic reclamation; `mem_16`).

## 10. Error handling

- One status set: B2 `Status` 0, -1..-17; B5 adds `FAULT = -32` and `NO_SYS = -33`. B7 `status.rs` repeats them. `sys_10` asserts every
  Rust return is inside the set; the checker asserts the numeric tables agree.
- Failed user-memory writes are all-or-nothing and output buffers are not left partially filled (`mem_13`, `mem_14`).

## 11. Resource accounting

- Zig: B2 `Account`/`Counters` (pages, handles, threads, channels, queued bytes), B3 per-space limits, B5 `accounting.zig`.
- Rust: only fixed table sizes (16 threads, 8 bindings, 16-entry exit log); exhaustion returns errors (`sys_08`, `mem_11`, `mem_17`).

## 12. Security boundaries

- Intended: ring 3 / ring 0 split by page tables and GDT/TSS; W^X; null page unmapped; supervisor bit on kernel pages.
- B9 adds defence in depth: a user access must satisfy both B7's region model and the page tables (present, user, writable for writes);
  overflow and `USER_TOP` are checked before any walk (`mem_03`, `mem_05..07`).
- None of the hardware-enforced parts has been executed. See `BATCH_9_SECURITY_REVIEW.md`.

## 13. Supported architectures

x86-64 only, hosted tests plus a never-booted image. ARM64 is not implemented. See `SUPPORT_MATRIX.md`.

## 14. Known limitations

Cooperative only; uniprocessor; no IPC/capabilities in Rust; never booted; unpinned toolchain; single syscall stack; no ABI version
syscall; no SMEP/SMAP/KPTI; syscall register convention vs `Kernel/` undecided; `Mapper` partial-failure leak.

## 15. Future integration requirements

1. Boot B8 under QEMU with the pinned toolchain; then on hardware.
2. Decide the syscall register convention against `Kernel/` (owner decision).
3. Port B4/B5 IPC and B3 capabilities to Rust, or bind them through a stable ABI, with tests against the Zig models.
4. Per-thread syscall stacks and interrupt masking in `SpinLock` for the host-target fallback.
5. `Mapper` rollback on failure; larger exit log.
6. Owner-approved placement under protected paths (`Documentation/`, `ExileKernel/`).
7. ARM64 port (new descriptor formats, vectors, timer).
