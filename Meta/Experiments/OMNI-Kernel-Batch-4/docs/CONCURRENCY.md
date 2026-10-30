# Concurrency (Batch 4)

## 1. Strategy

One `Ipc.mutex` and one condition variable guard all IPC state (queues, counters, registry, per-end flags).
The Batch 3 `Kernel` has its own single mutex.

**Lock ordering:** the two locks are never nested. `Ipc.mutex` is never held across a call into the kernel,
and the kernel runs destructor callbacks only after releasing its lock, so the kernel never calls IPC while
holding its lock. Hence there is no lock order to violate. Operations that need both use
reserve → work → commit: a reservation (message/byte counters plus a `users` reference on the end) is taken
under the IPC lock, kernel work runs unlocked, and a short critical section publishes or aborts.

An end is torn down only when `closed && users == 0`; whoever drops the last `users` reference (or closes)
performs teardown exactly once (`torn_down` flag set under the lock) outside the lock.

## 2. Linearization points

| operation | point |
|---|---|
| send | publication critical section (reserved → queued) |
| recv | claim (message marked) and commit (unlink) |
| closeEnd / destructor | critical section setting `closed` |
| signalCancel | critical section setting `signaled` |
| capability authority | Batch 3 `beginOp` (entry) and `opStillAuthorized` (re-checks) |

## 3. Claims supported by tests

Tested with real threads (Debug build, Linux): 4 producers + 3 consumers on one channel (each message
received exactly once, per-producer order preserved per consumer); capability copies sent while the parent
is revoked concurrently; close/destroy storms with senders and receivers running (60 rounds, all threads
terminate, no leaks); cancel-vs-arrival races (300 rounds). Randomised single-thread model comparison with
recorded seeds. **No claim** is made for: weak memory models/other architectures, more than one `Ipc` over
one kernel, concurrent `Kernel.destroy` with live operations (single-threaded teardown is required),
fairness or starvation freedom, or real-time behaviour. Tests are evidence, not a proof; no formal model
checking or ThreadSanitizer run was performed.

## 4. Known concurrency limits

* Revocation does not wake blocked waiters (CANCELLATION-AND-CLOSURE.md section 4).
* A send racing a revoke of its own copy source can fail with `InvalidHandle` (documented outcome).
* A single mutex serialises all channels (hosted prototype, not a scalability design).
* Hooks (`Ipc.hook`) are test instrumentation and must be set before threads start.
