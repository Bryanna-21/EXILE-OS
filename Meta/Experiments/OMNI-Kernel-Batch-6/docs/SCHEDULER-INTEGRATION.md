# Scheduler integration

Status: a HOSTED MODEL (`src/sched.zig`, `src/thread_state.zig`, `src/ipc_sched.zig`) plus a specification. The
model picks no thread and preempts nothing; hosted OS threads stand for kernel threads. It is not the scheduler of
`Kernel/` and does not call it.

## Thread states and transitions

States are those of `Kernel/Tasks/Thread.h`: invalid, runnable, running, blocked, stopped, dying, dead. The allowed
transitions (11) are checked exhaustively by TS-01:

```
invalid -> runnable          runnable -> running | dying
running -> runnable | blocked | stopped | dying
blocked -> runnable          stopped  -> runnable | dying
dying   -> dead              dead     -> (nothing)
```

Derived from `Thread::set_state`, `block_impl`, `unblock`: setting the current state is a PANIC in the existing kernel
(so `SameState` is an error here); blocking needs `running`; `stopped` only from `running`; `dying` is never entered
from `blocked` (wake first, then die). `runnable -> dying` and `stopped -> dying` are this package's reading of how the
source is used; they are not asserted by the existing code and should be reviewed.

## Block / wake protocol (eventcount)

```
tid -> ticket = prepareBlock(tid)      // only a running thread; snapshots the wake counter, clears stale wake-ups
try the operation without blocking     // the wait condition is tested AFTER the ticket
if it would block: block(ticket)       // immediate return if the counter moved or a kill is pending; else running -> blocked
waker: change the state, THEN wake(tid, reason)   // counter += 1; blocked -> runnable
woken thread: resumeThread              // runnable -> running, returns the reason (killed outranks)
```

No lost wake-up: a wake that lands between `prepareBlock` and `block` moves the counter and `block` returns at once
(WK-02); the 384-case exhaustive interleaving test WK-06 covers every placement of {wake, timeout, kill} before,
between and after. A wake before `prepareBlock` is deliberately stale (WK-01): the caller re-tests its condition.
`kill` is sticky: a killed thread never blocks again (WK-05).

Mapping to the existing kernel: `Thread::block_impl` achieves the same guarantee differently, by holding
`g_scheduler_lock` and `m_block_lock` while `Blocker::setup_blocker()` tests the condition and registers the blocker.
A real integration may keep that mechanism; what must be preserved is "test the condition and become blocked as one
step with respect to wakers".

## Ownership

| concern | owner |
|---|---|
| thread state, run queue, CPU assignment | the scheduler, exclusively |
| when a wait is satisfied (wait condition) | the IPC layer, which calls `wake` after changing state |
| which thread is woken | the IPC layer's waiter registry (`IpcSched.registered`) |
| the waiter slot (Batch 5 ledger) | the waiting thread: taken before blocking, released on every exit path |
| timeouts | the waiter's deadline; the model implements them inside `blockWait` |
| cancellation | a cancel object signalled through `IpcSched.cancelSignal`, which wakes waiters |
| termination | `terminateSpace`: kill flags, close, wake; each thread exits itself |

## Behaviour

* **Blocking**: `IpcSched.send/recv` use Batch 5 only through its non-blocking interface (timeout 0), register as
  waiters, take a ticket, test, block via the scheduler (IPC-01, IPC-02).
* **Waking**: every state change through `IpcSched` (send, recv, close, release, revoke, cancel signal) wakes all
  registered waiters; they re-test. Broadcast, so spurious wake-ups happen and are harmless (IPC-14).
* **Timeout**: no earlier than requested, then one last attempt, so arrival beats timeout (IPC-03, IPC-12, WK-08).
* **Cancellation**: a signalled cancel wins before anything is consumed (IPC-04, IPC-13).
* **Close while blocked**: peer closure wakes with `PeerDead` (IPC-05).
* **Kill while blocked**: the thread wakes, the operation returns `Cancelled`, nothing is consumed, the slot is returned,
  the thread exits (IPC-07, IPC-08, PR-01).
* **Invalid transitions** are refused with no change (TS-02).

## Lock ordering

`Sched.mutex` (models `g_scheduler_lock`) and `IpcSched.reg_mutex` are leaves: never nested, never held across a call
into Batch 5/4/3, and no callback runs under them. Batch 5's `WaitQueue.mutex` and `Ledger.mutex` are leaves too.
A real kernel must additionally order these below `m_block_lock` and above spinlocks held by IRQ handlers; that order
is not defined here.

## Requirements on a real scheduler (specified)

1. `wake` callable from thread context under the scheduler lock, and from IRQ context only through a deferred call;
2. a blocked thread must be wakeable by kill without holding any IPC lock;
3. a monotonic clock and timer wake-up;
4. per-thread owner address space readable by the syscall path;
5. `exit` must not run while the thread holds a waiter slot (the IPC layer guarantees this by returning first).

## Known gap (important)

Batch 5 `syscall.dispatch` blocks inside Batch 5's own hosted `WaitQueue`. A thread blocked there is `running` as far as
`Sched` knows and cannot be woken by `Sched.kill`. `terminateSpace` still frees it because closing the space notifies
Batch 5's waiters (PR-03), but a kill of a SINGLE thread of a live process does not. Fixing this needs a wait-provider
hook in Batch 5 `dispatch` (a function table `{prepare, wait, notify}` passed in); that is a proposed change to Batch 5
and is NOT made here. Until then the syscall path and `IpcSched` are two separate blocking paths.
