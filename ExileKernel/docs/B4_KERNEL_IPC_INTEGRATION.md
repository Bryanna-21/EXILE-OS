# EUK kernel IPC integration (Mission B4)

B4 connects the host-tested capability/IPC code of Mission B3 to the kernel's scheduler, syscall
path, address spaces, and process/thread lifecycle. This document describes what is
**implemented and tested**, and in the last section what is **not**.

## Status at a glance

| Level | State |
|---|---|
| Host unit + integration tests (`python3 scripts/test_b4.py`) | done: 72 B4 tests, 83 B3 tests, 99 D5 tests re-run |
| Real context switches in tests (blocked syscalls genuinely yield, host stacks) | done, 8 tests, 5 stable repetitions |
| `#![no_std]` `-D warnings` compile of D5 + `b3` | done |
| Kernel image declares `b3` and links (host-target build, `scripts/check.py`) | done |
| Booted under QEMU / real hardware | **no** (no QEMU, GRUB or `x86_64-unknown-none` target in the development environment) |
| A user process actually issuing these syscalls on a booted kernel | **no** (see "Limitations") |

## Files

New (12): `src/b3/mod.rs`, `kernel_domain.rs`, `syscall_dispatch.rs`, `scheduler_bridge.rs`,
`resume.rs`, `user_memory.rs`, `ipc_service.rs`, `process_lifecycle.rs`, `thread_lifecycle.rs`,
`initialization.rs`, `scripts/test_b4.py`, and this document.

Existing files changed (all backed up before editing):

| File | Change |
|---|---|
| `src/kernel.rs` | declares the D5 modules (`address_space`, `elf`, `preempt`, `process`, `sync`, `tls`, `usermode`, via `#[path]`) and `mod b3`, so the image compiles and links them. Nothing in the boot path calls them. |
| `src/d5/usermode.rs` | `handle_syscall`'s tail extracted into `pub fn apply_outcome(frame, outcome)`; behaviour identical (all D5 tests re-run and pass). |
| `src/b3/{cap_space,ipc,ipc_abi,ipc_endpoint,ipc_message,kobject}.rs` | `crate::<b3 module>` paths became `super::<module>` so the files work both under `mod b3` and flat at a crate root (`test_b3.py` still passes unchanged). `cap_space::remove_all`, `ipc::close_all` and `ipc::is_waiting` were added (used by shutdown and cleanup). B3 semantics are unchanged; the B3 fixes (cap cleanup after a failed copy-out, direct-hand-off statistics) are preserved and exercised. |

No change to the scheduler (`sched.rs`, `dispatch.rs`), the thread layer, the syscall ABI or the
address-space code.

## Architecture

```
user mode ──syscall──▶ [entry glue: not in the image yet]
                         │ usermode::handle_syscall   (D5, unchanged)  or
                         ▼ b3::syscall_dispatch::handle_frame (IPC-aware drop-in)
            nr outside 0x40..=0x48 ──▶ usermode::dispatch (D5, unchanged, ENOSYS if unknown)
            nr in 0x40..=0x48     ──▶ ipc_abi::handle  ──▶ KernelIpcService ──▶ IpcDomain
                                         │                    (b3::kernel_domain::KernelDomain)
                                         ├─ Done(v)    → value returned
                                         └─ Blocked(p) → PendingTable armed → scheduler_bridge::park
                                                          … another thread runs …
                                                        resume::finish_blocked → ipc_abi::complete
```

* `KernelDomain` – the single authoritative IPC state: `IpcDomain` plus the `PendingTable`
  (blocked syscalls) behind one `SpinLock`. Phases `Uninit → Active → Down`. `const fn new()`
  makes it a `static`; there is no unprotected global.
* `IpcKernel { domain, port }` – the only way to take both locks: **domain lock first, then
  scheduler lock** (`locked`). Neither is ever held across a context switch.
* `SchedPort` – the scheduler as seen by B4 (`with_sched`, `is_ready`, `switch_away`).
  `DispatchPort` (x86-64) implements it over `dispatch::KernelSlot` / `dispatch::reschedule`;
  `testkit::ModelPort` models it for tests. `DispatchPort::new` is `unsafe` with exactly
  `dispatch::reschedule`'s contract; these are the only two `unsafe` sites in `b3` production code.

## Dispatch flow

`syscall_dispatch::dispatch(host, mem, args, ipc, owner_of)`:

1. `ipc_abi::owns(nr)` false → `usermode::dispatch` **unchanged** (every D5 syscall and D5's
   `ENOSYS` for unknown numbers).
2. IPC number, domain not active → `ENOSYS` before the scheduler is even consulted.
3. The caller is the scheduler's current, non-idle thread (`ESRCH` otherwise); its owner is the
   process the thread belongs to, from the process table (`owner_of`, a closure so the table is
   not borrowed across a context switch). `ESRCH` if there is no process or it is exiting. The
   owner id is never taken from user space.
4. Under both locks: `PendingTable::begin` (a ticket proving the slot is free), `ipc_abi::handle`
   through `KernelIpcService`, and, if the thread was blocked, `PendingTable::arm` in the **same
   critical section**. `ipc_abi::handle` validates every argument and user pointer before any
   state changes, so a bad output pointer cannot consume a message.
5. `handle_frame` stores RAX and returns by D5's rules (`apply_outcome`: frame re-validation,
   `iretq` instead of `sysret` when TF is set).

## Blocking and wake-up flow

1. `Abi::Blocked` means the domain already marked the thread blocked (`block_current`, event id
   `0x4950_0000 + object`).
2. `scheduler_bridge::park` calls `SchedPort::switch_away` (the real `dispatch::reschedule`): the
   CPU goes to the next thread. `park` returns only when the scheduler selected this thread again,
   and verifies that it is the current, running thread (`NotResumed` otherwise).
3. Waker side (`IpcDomain::send/signal/close/...`) wakes the **best** waiter: highest effective
   priority, FIFO within a priority. A wake between "blocked" and the switch is harmless: the thread
   becomes Ready, so the scheduler either re-selects it at once or runs it right after the
   threads ahead of it (tested at the model level; the real switch returns when it is scheduled).
4. `scheduler_bridge::interrupt_blocked` (used by `thread_lifecycle::interrupt_thread`) wakes a
   thread only if it is blocked on an IPC event; any other thread, or one already woken, is
   refused, so wake-ups are neither duplicated nor misdirected.

## Resume flow

`resume::finish_blocked` runs on the woken thread, under the locks:

1. `PendingTable::take(thread, owner)` removes the thread's own record (`NoPending` /
   `OwnerMismatch` otherwise; a mismatching record is removed and reported, not completed).
2. `IpcDomain::resume(thread)` yields `Sent` / `Received` / `Signalled` or `Closed`, `Revoked`,
   `Interrupted`.
3. `ipc_abi::complete(svc, mem, pending, result)` writes the **original** user buffer recorded in
   the pending operation and returns the value for RAX (`0`, `EPIPE`, `ECANCELED`, `EINTR`, or
   `EFAULT` if the copy-out failed, in which case the delivered capability is closed again).
4. On a protocol failure the domain's own result is still collected and any capability it carries is
   closed, so nothing is left behind.

Results of a blocked call when the record is gone while sleeping (process teardown) or the
subsystem was shut down: `EINTR`. `EIO` signals a protocol violation or a failed context switch
(the latter is a fatal kernel condition, as for `dispatch::reschedule`; the thread's IPC records
are dropped first).

## User-memory safety (`user_memory.rs`)

`SpaceMemory { space, phys }` implements `UserMemory` over a process `AddressSpace`:
`usermode::user_range_ok` first (user half only, no wrap, no null page, no kernel addresses),
then the address space's own page-table walk (`validate_user_range` with `Read` or `Write`, per
page: present, user-accessible, writable for writes), then the copy. Every failure is
`SyscallError::Fault`. `SpaceMemory::for_process` ties the memory to a process-table handle.
`mem` passed to `dispatch` must be the calling process's memory; the dispatcher cannot check that.

## Capability ownership and transfer

* Capabilities belong to a process id (D5 never reuses pids). A handle of another process, a stale
  handle, a forged or malformed value all fail identically (`EBADF`); a missing right is `EPERM`.
* `KernelIpcService` is bound to one thread and owner; every call first checks that the thread is
  still the running non-idle thread and that the owner is not the limbo owner.
* Transfer: the capability in a message must be held with `GRANT`; the domain derives it into limbo
  at send time (rights can only shrink: a sender derives a narrower capability first) and rebinds
  it to the receiver on delivery. A revoked-in-flight or quota-refused capability is reported with
  `cap_lost`. No capability survives a failed delivery (failed copy-out, killed receiver).

## Cleanup

* **Thread** (`thread_lifecycle::cleanup_thread`): removes the thread's pending record and its wait
  record, releases a capability delivered but never collected, discards a blocked sender's
  in-flight capability, then sweeps. The thread is never woken again; the process keeps its
  capabilities. Idempotent. `IpcCleanupEnv` wraps `ProcessEnv::kill_thread` so every kill runs it
  (also when the slot is already `Gone`; a failing cleanup reports `EnvError::Failed` so the process
  layer retries).
* **Process** (`process_lifecycle`): wrappers `exit`, `fault_exit`, `deliver`, `thread_exited`,
  `collect` around the `ProcessTable` operations. As soon as the process is `Exiting`/`Zombie`
  (or its handle is stale) `cleanup_process` removes its pending operations and closes every
  capability it holds in every table (`IpcDomain::owner_exit`), waking threads of other processes
  that waited on objects that died with it. Shared objects survive until their last owner is gone.
  An error from D5 is reported in preference to an IPC cleanup error; cleanup is idempotent so the
  retry repeats it. **The kernel must call these wrappers, not the bare `ProcessTable` methods.**

## Initialisation order (`initialization.rs`)

`init(ipc, InitConfig { user_memory_ready, owner_cap_limit })`: (1) scheduler ready
(`SchedPort::is_ready`: exists with an idle thread), (2) user memory ready (reported by the
caller), (3) a fresh `IpcState` is installed last. Failure leaves the domain uninitialised
(repeatable); an active domain is never replaced. Until then and after `shutdown`, IPC syscalls
answer `ENOSYS` without touching anything. `shutdown` moves the domain to `Down` under its lock,
clears pending records, destroys all objects and capabilities and wakes every blocked thread (they
return `EINTR`); `init` can then run again from empty state. `init` is a start-up call, never made
per syscall.

## Error handling summary

`ENOSYS` (-38) unknown number / IPC not initialised, `ESRCH` (-3) no running thread or process,
`EFAULT` (-14) bad user pointer or failed copy-out, `EBADF` (-9) bad/stale/foreign handle, `EPERM`
(-1) missing right, `EAGAIN` (-11) would block, `EINVAL` (-22) bad flags/arguments, `ENOMEM`/`EMFILE`
table/quota full, `EPIPE` (-32) object destroyed, `ECANCELED` (-125) revoked, `EINTR` (-4)
interrupted / torn down / shut down, `EIO` (-5) internal protocol error.

## Tests

`python3 scripts/test_b4.py` (EUK_TEST_BUILD overrides the build dir) builds the tree as the
kernel declares it and runs: every B3 and B4 module (each must run tests), the whole `b3` tree in
parallel, the D5 modules against the edited `usermode.rs`, a table of 44 **required** named tests
for 14 requirement groups (a deleted test fails the run), five repetitions of the real-context-switch
tests, the `no_std` `-D warnings` compile, an `unsafe` audit, `rustfmt --check` on every changed
Rust file, and the host-target image link check.

Final counts: B4 modules 72 (kernel_domain 5, scheduler_bridge 7, resume 11, user_memory 8,
ipc_service 7, syscall_dispatch 17, thread_lifecycle 7, process_lifecycle 6, initialization 4);
B3 83; whole `b3` tree 155; D5 99 re-run.

Covered: routing and unknown numbers; uninitialised/shut-down refusal and no scheduler access before
init; init order and refusal; ownership and cross-process handle rejection; pointer validation
(kernel, null, wrapping, unmapped, read-only, partial range, physical write error) with no state
change; blocking and resumption with **real** context switches; direct hand-off; queued delivery
and send order with a blocked sender; priority then FIFO waiter order; capability transfer,
attenuation and owner binding through a blocked receive; failed copy-out cleanup (resume, adapter and
dispatch levels); process exit, fault exit, kill event, thread exit, a thread killed while blocked
(never resumes), shared-object lifetime; interruption and no duplicate wake-up; shutdown while
blocked; stale and forged handles; 300 create/use/close cycles including deliberate failures with
no leaked objects, capabilities or pending records and the domain invariants checked.

Hand-applied mutations of the B4 sources (16, e.g. no owner check, no pending arm, skipped park,
no cleanup, no init check, write checked as read) were each rejected by the suite, except one:
removing the `user_range_ok` pre-check in `SpaceMemory` survives, because D5's page-table walk
rejects the same addresses independently (the pre-check is defence in depth).

## Limitations (nothing below is implemented or claimed)

* **Not booted.** No QEMU/hardware run. The image build is the host-target link check only; the
  pinned Rust 1.91.1 and `x86_64-unknown-none` were not available (host Rust 1.97 was used).
* **No live caller.** The default boot path creates no process table, never enters user mode and
  never calls `init`; `kernel.rs` contains no `static KernelDomain`, no syscall-entry call to
  `handle_syscall`, and no scheduler-side `ProcessEnv`. B4 supplies every piece (`handle_frame`,
  `init`, the lifecycle wrappers, `DispatchPort`), but the glue that instantiates and calls them
  lives in a kernel that does not yet run user processes. Consequently no user program has issued
  these syscalls on a booted kernel.
* Blocking happens inside the syscall handler on the thread's kernel stack with interrupts disabled
  and no lock held (the `dispatch::reschedule` contract); `DispatchPort::new` is `unsafe` for that.
* Single CPU only. The spin locks do not mask interrupts; interrupt context must use `try_with`.
  No interrupt-driven signalling (`kernel_signal`) is wired.
* `SpaceMemory::check` and the copies are not atomic against another CPU; a physical-memory error
  in the middle of a copy-out can leave a prefix written (reported as `EFAULT`, capability closed).
* A failed context switch is treated as fatal (`EIO` after dropping the thread's IPC records).
* No timeouts, no priority inheritance through IPC, no reply capabilities, no capabilities for
  threads/processes/memory; linear table scans (B3 limits stand).
* The process-table owner lookup is a linear scan.
