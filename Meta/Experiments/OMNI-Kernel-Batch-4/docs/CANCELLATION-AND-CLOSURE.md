# Waiting, cancellation, timeout and closure (Batch 4)

## 1. Abstraction

`WaitOptions { timeout_ns: ?u64, cancel: ?Handle }`.

* `timeout_ns = 0` never waits (`WouldBlock`); `null` waits without deadline; `n` waits at most `n` ns then
  `TimedOut` (measured with a monotonic timer; may overshoot by scheduling latency, never undershoots).
* `cancel` is a `cancellation` capability (WAIT to wait on it, SIGNAL to signal). `signalCancel` is one-shot
  and idempotent; a cancellation object cannot be reset.
* Waiting uses one condition variable; waiters sleep and are woken by publish, receive, abort, close and
  signal. There is no polling or busy-wait (`WAIT-05` measures CPU time as a heuristic).

## 2. Race semantics

* A cancel already signalled when the call starts returns `Cancelled` before anything is consumed or
  reserved (even if a message is queued).
* If a message is available when a waiter wakes, it is delivered even if cancellation was also signalled
  ("arrival beats cancel"). The same holds for room in a full queue.
* After a message is claimed, cancellation is not consulted again: no double delivery, no accepted message
  is lost. A `Cancelled` receive leaves the queue untouched.
* Precedence when several conditions hold: closed end → available message → peer closed → non-blocking →
  cancelled → timed out.

## 3. Closure

See IPC-MODEL.md section 2. Closing wakes all blocked senders and receivers on both ends. Queued
messages of a closed end are dropped, their accounting refunded and their capabilities released exactly
once. Messages queued toward a *surviving* receiver when the *sender* closes remain deliverable.

## 4. Authority while waiting

Authority is checked once at entry (`beginOp`, which pins the object) and re-checked after every wake-up
with `opStillAuthorized`. If the waiting capability was revoked/released or its space closed, the call
returns `InvalidHandle` at the next wake-up. **Limitation:** Batch 3 offers no notification on revocation,
so a blocked waiter whose capability is revoked is not woken by the revocation itself; it notices at its
next wake-up (any state change, timeout or cancel). A waiter with no timeout and no further activity
can therefore remain blocked after its capability is revoked. Use a timeout or a cancellation object.
Closing the *end* (not merely revoking a capability) does wake waiters.

## 5. Cleanup

Resources are released on receive (accounting capability), drop (queued message at teardown), cancel
(nothing was acquired beyond the reservation, which is returned), close (hold space closed) and failure
(rollback). Destructors and teardown run outside all locks, exactly once (tracked by `torn_down` and the
Batch 3 exact-once destruction).
