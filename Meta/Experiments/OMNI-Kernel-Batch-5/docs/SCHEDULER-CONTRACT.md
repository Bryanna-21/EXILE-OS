# Scheduling and wait contract

Status: HOSTED WAIT MODEL. There is no scheduler. Blocking is modelled with an OS thread, a mutex and a
condition variable (`src/wait_queue.zig`). This is the model a real scheduler must reproduce; it is not
evidence that a bootable kernel schedules correctly.

## Protocol (eventcount)

```
ticket = wq.prepare()                 // read the sequence number
result = try the operation, NON-BLOCKING (Batch 4 timeout 0)
if the result is WouldBlock and the caller allows waiting:
    acquire a waiter slot (bounded; QuotaExceeded if none)
    outcome = wq.wait(ticket, deadline)   // returns at once if the sequence moved after prepare
    retry (one last try after a timeout; arrival beats timeout)
after every state change: wq.notifyAll()  // sequence += 1, then wake all waiters
```

Because the ticket is taken before the attempt and the state change increments the sequence after the
change, a change between "try" and "wait" is seen as a moved sequence: no wake-up is lost (WQ-01, WQ-05, BLK-16,
BLK-17).

## Wake-up conditions

A blocked caller is woken by: a send or receive through `KernelIpc`; `endpointClose`, `releaseHandle`,
`revoke`, `spaceClose`, `cancelSignal` through `KernelIpc`; its deadline. After waking it re-evaluates
all conditions through Batch 4, so a closure gives `PeerDead`, a revoked capability `InvalidHandle`, a
signalled cancel `Cancelled`, and a message `ok`.

## Guarantees and limits

* Every wait has a deadline (default maximum 30 s) or a cancellation handle; there is no unbounded wait
  without a cancel path.
* Blocked callers are bounded per space (16) and in total (64); beyond that `QuotaExceeded`. Slots are
  returned on every exit path (BLK-13, ACC-01, `audit()`).
* Notification is broadcast (thundering herd accepted; waiters re-check). Fairness is not guaranteed.
* Changes made behind the interface (direct Batch 3/4 calls) do not notify; waiters see them at their
  deadline (BLK-11).
* Timeouts use a monotonic timer; if no timer is available the deadline is treated as expired (fails closed).

## Lock ordering

Order: Batch 3 kernel mutex and Batch 4 internal locks are never held when `WaitQueue.mutex` or
`Ledger.mutex` is taken; those two are leaves, never nested with each other and never held across a call
into Batch 3/4. No lock is held while sleeping except the wait-queue mutex inside the condition wait.

## What a real scheduler must provide

1. a sleep that atomically releases the wait-queue lock and blocks (so the ticket check is race-free);
2. a wake-all primitive callable from any context that can call the wrappers;
3. a monotonic clock for deadlines;
4. cancellation of a blocked thread's wait when its space is closed (the wrappers already notify);
5. priority and fairness policy, not defined here.
