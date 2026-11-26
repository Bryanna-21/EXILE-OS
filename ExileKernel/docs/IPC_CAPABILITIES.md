# EUK capabilities and bounded IPC (Mission B3)

Milestone gate 5 of `ROADMAP.md` ("capability enforcement, IPC") as host-tested kernel code.
Everything here is **host-tested only**. It is **not declared in `kernel.rs`, not in the kernel
image and not part of the normal boot path**; nothing has run under QEMU or on hardware.

## Status at a glance

| Level | State |
|---|---|
| Host unit and scheduler-driven tests (`python3 scripts/test_b3.py`) | done, 79 tests |
| `#![no_std]` `-D warnings` compile of the B3 modules | done |
| Linked into the kernel image | **no** (not wired) |
| Booted under QEMU | **no** |
| Real hardware | **no** |
| User-space program exercising the syscalls | **no** |

## Files

| File | Responsibility |
|---|---|
| `src/b3/cap_rights.rs` | `Rights` bit set, per-object-kind validity, attenuation, checked decoding of untrusted rights |
| `src/b3/kobject.rs` | Generation-checked object table (`ObjId`), retire-on-wrap slots |
| `src/b3/cap_space.rs` | Owner-bound capability table, derivation tree, revoke, close, limbo transfer, quotas |
| `src/b3/ipc_message.rs` | Fixed-size message, user wire format (56 B in, 64 B out), delivery record |
| `src/b3/ipc_endpoint.rs` | Bounded FIFO message queue |
| `src/b3/ipc_notify.rs` | 64-bit coalescing signal word |
| `src/b3/ipc.rs` | `IpcDomain`: scheduler-integrated send/recv/signal/wait, hand-off, sweeping, teardown |
| `src/b3/ipc_abi.rs` | Syscalls `0x40..=0x48`, user-pointer validation, errno mapping, `Bound` adapter |
| `scripts/test_b3.py` | Builds and runs the B3 tests, `no_std` check, no-`unsafe` check, rustfmt check |
| `docs/IPC_CAPABILITIES.md` | This document |

The B3 modules depend on existing EUK code only: `thread`, `runqueue`, `sched` (D1-D3) and
`usermode` (D5: `SyscallArgs`, `UserMemory`, `user_range_ok`), plus their own dependencies
(`frame`, `cpu`, `gdt`). They contain no `unsafe`, no allocation and no new Cargo dependency.
They are independent of the earlier B2 deliverable.

## Capability model

* A capability is `(object, rights)` held by an **owner** (a process id). User space sees a
  64-bit handle `(generation << 32) | (slot + 1)`; 0 is never valid, and generations stop at
  `0x7FFF_FFFF` (a slot is retired instead), so a handle is always `< 2^63` and can never be
  confused with a negative error return.
* A lookup with the wrong owner, wrong generation, empty slot, out-of-range slot or malformed
  value fails identically (`BadHandle`): a process cannot probe another process's table. A
  missing right is `Perm`.
* Rights: `SEND`, `RECV` (endpoints), `SIGNAL`, `WAIT` (notifications), `GRANT`, `REVOKE`.
  A capability may only carry rights valid for its object kind. Untrusted rights words are
  rejected, not masked (unknown bits, empty set, wrong kind).
* **Derivation**: `derive(mask)` yields `rights & mask` and must be non-empty; rights never grow
  along the tree (also verified by `check_invariants`). Handing a capability to a different
  owner needs `GRANT`.
* **Revoke** (needs `REVOKE`) deletes every descendant in every owner's space and keeps the
  capability. **Close** deletes one capability and re-parents its children to its parent.
  **Owner exit** closes everything an owner holds.
* **Quota**: `set_owner_cap_limit` bounds how many capabilities one owner holds; the table also
  has a fixed size. Both are reported (`Quota`, `NoSlots`) and roll back cleanly.
* **Object lifetime**: an object lives while a real owner holds a capability to it. Capabilities
  in flight (limbo) do not count, so a capability sent through its own endpoint cannot keep it
  alive. When the last owner capability goes the object is destroyed, queued messages and their
  limbo capabilities are discarded, and threads blocked on it wake with `Closed`.

## IPC semantics

* **Endpoint**: bounded FIFO (`Q` messages). A full queue blocks the sender (or `WouldBlock`
  with `FLAG_NONBLOCK`); messages are never dropped or overwritten. A waiting receiver gets a
  message directly; when `recv` frees a slot the best blocked sender's message is moved into the
  queue in the same step, so queue order is send order.
* **Notification**: `signal(bits)` ORs into a word and never blocks (usable from interrupt
  context through `IpcDomain::kernel_signal`, which uses only `Scheduler::wake`); signals coalesce;
  `wait` takes the whole word or blocks. `signal(0)` is rejected.
* **Message**: label, up to 4 data words, at most one capability. Unused words must be zero.
* **Capability transfer**: the carried capability must be held with `GRANT`; it is derived into
  limbo (unusable by anyone) at send time and rebound to the receiver on delivery. If it was
  revoked in flight, or the receiver's quota is full, the message is still delivered with
  `cap_lost` set (wire flag bit 0) and `Stats::caps_lost` counts it.
* **Waiter choice**: highest effective priority, FIFO within a level (same rule as `d5::sync`).
* **Wake-up results**: a blocked thread learns what happened from `resume`: `Sent`,
  `Received`, `Signalled`, or an error: `Closed` (object destroyed), `Revoked` (its capability
  was revoked or closed), `Interrupted` (woken by something else; the record is removed).
* **Teardown**: `thread_exit` removes the wait record, discards the limbo capability of a
  blocked send and releases a capability delivered but never collected. A reused thread slot
  never inherits a predecessor's result or wait record (handles are compared, and the slot's
  next IPC call cleans leftovers).

## Syscall ABI (`ipc_abi.rs`)

Numbers `0x40..=0x48` do not overlap D5's `Sysno` (0..=4); `ipc_abi::owns(nr)` selects them.

| nr | name | a0 | a1 | a2 | returns |
|---|---|---|---|---|---|
| 0x40 | `EP_CREATE` | | | | endpoint handle |
| 0x41 | `NF_CREATE` | | | | notification handle |
| 0x42 | `SEND` | endpoint | message ptr (56 B) | flags | 0 |
| 0x43 | `RECV` | endpoint | out ptr (64 B) | flags | 0 |
| 0x44 | `SIGNAL` | notification | bits | | 0 |
| 0x45 | `WAIT` | notification | out ptr (8 B) | flags | 0 |
| 0x46 | `DERIVE` | handle | rights mask | | new handle |
| 0x47 | `CLOSE` | handle | | | 0 |
| 0x48 | `REVOKE` | handle | | | capabilities deleted |

`flags` bit 0 = non-blocking; other bits are `EINVAL`.

Send buffer (little endian): `label, len, words[4], cap` (7 x u64). Receive buffer: the same
followed by `flags` (8 x u64). `cap` is a handle in the caller's space (0 = none).

Errors (RAX): `EBADF -9` bad handle, `EPERM -1` missing right, `EAGAIN -11` would block,
`EFAULT -14` bad user pointer, `EINVAL -22` bad argument, `ENOMEM -12` table full,
`EMFILE -24` quota, `EPIPE -32` closed, `ECANCELED -125` revoked, `EINTR -4` interrupted,
`ESRCH -3` no current thread, `EIO -5` internal error, `ENOSYS -38` not an IPC syscall.

User pointers are checked with `user_range_ok` and `UserMemory::check` **before** the operation
runs, so a bad output pointer cannot consume a message or signal word.

## Tests

`python3 scripts/test_b3.py` (host rustc; `EUK_TEST_BUILD` overrides the build directory):

1. B3 unit and integration tests under a generated crate root, per module, each module must run
   at least one test. The IPC tests drive the real `Scheduler` (block, run, wake, kill, reap).
2. `#![no_std]` + `-D warnings` metadata compile of the same modules.
3. No `unsafe` in B3 sources.
4. `rustfmt --check` on the B3 files only (`skip_children`).

Covered paths include: valid use, wrong owner, stale and forged handles, missing rights, wrong
object kind, unknown/empty rights, oversize and dirty messages, queue-full back-pressure,
non-blocking `WouldBlock`, blocked send/recv/wait and their wake-ups, priority and FIFO waiter
order, capability transfer, revoke in flight, quota refusal at the receiver, revoke/close/
destroy waking blocked threads, object and capability exhaustion with rollback, generation wrap
retirement, killed blocked threads, reused thread slots, owner teardown, interrupt-style
signalling, bad user pointers, read-only user memory, and the invariant checkers themselves
(corrupted tables are detected). Hand-applied mutations of the sources (23 of them, among
them removing the owner check, the GRANT check, attenuation, re-parenting, priority order and
the stale-record cleanup) were each rejected by the suite.

## What is not done (integration and limits)

Nothing below is implemented or claimed.

* **Not wired**: no `mod` declarations in `kernel.rs`, no hook in the syscall entry, no change
  to `scripts/test.py` or `build.py`. To integrate: declare the B3 modules at crate root (the
  flat layout `test_b3.py` uses), chain `ipc_abi::handle` after D5's `dispatch` for
  `ipc_abi::owns(nr)`, and create a global `IpcDomain` behind the kernel lock.
* **Blocking glue**: `handle` returns `Abi::Blocked`; the kernel must reschedule, call
  `IpcDomain::resume`, and pass the result to `ipc_abi::complete`. This glue (like
  `d5::sync`'s x86-64 wrappers) is not written, so the blocking path is verified against the
  scheduler *model* only, not a real context switch.
* **Process layer**: `owner` is a plain `u64` supplied by the caller. Calling
  `owner_exit` / `thread_exit` from process teardown is an integration step.
* If the final `copy_to_user` of a received message fails after `UserMemory::check` passed
  (a race the host cannot model), the message has already been consumed and is lost.
* No timeouts, no cross-CPU use, no priority inheritance through IPC, no reply capabilities,
  no capability for threads/processes/memory.
* The capability table scans linearly; sizes are meant to be small and fixed at compile time.
* Hardware and QEMU behaviour is unverified; the pinned Rust 1.91.1 toolchain and a bare-metal
  target were not available when this was developed (host Rust only).
