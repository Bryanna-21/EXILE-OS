# Architecture

Batch 7 is a `no_std` Rust crate (`kernel/`, edition 2024) plus two x86-64 assembly files. It targets the Rust
x86-64 `ExileKernel/` but is **not wired into it**: `ExileKernel/` and `Kernel/` are unmodified (see "Why not
integrated" below).

## Modules

| module | contents | executes where |
|---|---|---|
| `context.rs` + `arch/x86_64/context.S` | `omni_switch_context`, `omni_thread_trampoline`, initial stack frame, saved-rsp validation, stack canary | **Real code, run on the Linux x86-64 host** (real stack and register switches). Never run in ring 0. |
| `sched.rs` | uniprocessor cooperative scheduler: thread table, run queue, block / wake / sleep / kill / exit, space-load hook, reaping | **Real code, run on the host** on top of the real switch |
| `syscall.rs` | `SyscallFrame`, frame validation, `Kernel::syscall` dispatch, provisional syscalls, `omni_syscall_common` | **Real logic, run on the host.** Called directly by tests; the hardware stub that calls it was not run |
| `arch/x86_64/syscall_entry.S` | SYSCALL entry stub (swapgs, stack switch, frame push, call, restore, sysretq) | **Assembled and disassembled; never executed** (needs ring 0) |
| `msr.rs` | STAR / FMASK / GDT-descriptor encodings, CPUID check, `wrmsr` / `rdmsr`, `init_syscall` | encodings and CPUID are tested; `init_syscall` / `wrmsr` / `rdmsr` are **compiled, never executed** |
| `uaccess.rs` | `AddressSpace`, `Spaces`, range validation, `copy_from_user` / `copy_to_user` over a `UserBacking` | **Real logic on the host**, over a simulated backing memory |
| `sync.rs` | spin lock; interrupt masking (`pushfq` / `cli` / `sti`) only for `target_os = "none"` | host path executed; the bare-metal path is **assembled in isolation only** |
| `tests.rs` | 56 hosted tests | host |

## Classification

* **Fully implemented and exercised (host):** context-switch assembly, thread trampoline, initial frames, saved-rsp
  and canary validation; scheduler states, run queue, block / wake with no lost wake-up, sleep, kill, space
  termination, exit and reaping, generation-checked thread ids; syscall number and frame validation, caller
  identity from the scheduler, user-range validation and copies.
* **Partially implemented:** syscall entry (stub written and byte-checked, Rust dispatch executed, but the two have
  never been connected by a running CPU); user-memory isolation (the *rules* are implemented and tested, the page
  tables that enforce them in hardware do not exist); `sync.rs` interrupt masking.
* **Hosted simulations:** "user threads" (kernel-mode coroutines calling `Kernel::syscall`), `SimMem` user memory,
  the timer tick (tests call `tick()`), the second CPU in the race test (a second host OS thread).
* **Architecture-specific:** everything here is x86-64. The crate refuses to compile elsewhere
  (`compile_error!`).
* **Not supported / not implemented:** ARM64 (Batch 6 has a hosted register abstraction only; nothing here),
  preemption, interrupts / IDT / TSS, SMP, FPU/SSE state switching, real page tables and `cr3` loading, syscalls
  of the Batch 5 IPC layer, blocking IPC, user-mode entry (`iretq` / `sysretq` to ring 3), `fork` / exec / process
  objects, timers, a heap allocator.

## Why not integrated into `ExileKernel/`

1. **No toolchain to build or boot it here.** `ExileKernel/scripts/build.py` requires rustc 1.91.1 and the
   `x86_64-unknown-none` standard library; this environment has rustc 1.97.0 with only the host target, and the
   Rust download host answers 403. QEMU, GRUB and xorriso are not installed and apt has no network. Edits to
   `ExileKernel/` could not be compiled for its target or booted.
2. **Its own roadmap puts the prerequisites first.** `ExileKernel/` has no IDT, TSS, user segments, interrupt
   controller, timer, allocator or W^X mapping; its ROADMAP lists those before threads and syscalls. A scheduler
   and SYSCALL path need all of them before they can run in ring 0.
3. **Language boundary.** The IPC layer (Batches 3-5) is Zig and hosted; `ExileKernel/` is Rust. They cannot be
   linked together without an FFI decision that belongs to the project owner.

So the code is written so that it can be dropped in: `kernel/lib.rs` is already `no_std`; the integration surface
is the symbol set `omni_switch_context`, `omni_syscall_entry`, `omni_syscall_common` plus `Sched::run` and
`msr::init_syscall`. See `SYSCALL-INTEGRATION.md` and `SCHEDULER-INTEGRATION.md` for the exact steps.

## Relation to earlier batches

Batch 6 specified the thread-state machine, the eventcount block / wake protocol and the entry contract as Zig
models. This batch re-implements the protocol in Rust against a real context switch rather than importing the Zig
models. The Batch 6 transition rules are followed where they apply (a woken blocked thread becomes runnable; a
thread is never resumed once freed; kill is sticky; reason precedence none < timeout < woken < cancelled < killed).
Differences are intentional and listed in `SCHEDULER-INTEGRATION.md`.
