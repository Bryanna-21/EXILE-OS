# Security analysis (hosted, kernel-facing)

Scope: the syscall boundary and the kernel-facing IPC layer as tested in a hosted process. Hardware
isolation, real user/kernel memory, interrupts and timing side channels are out of scope.

| threat | mitigation | evidence |
|---|---|---|
| forged or stale handle | generation-checked handles; lookup per call; handle values are not secrets | SYS-03 |
| handle of another space | handles are per space; the caller space comes from the kernel | SYS-03 |
| missing rights | rights checked on every operation by the capability layer | SYS-02 |
| user pointer misuse (null, kernel range, wrap-around) | all copies via `UserMemory`, checked arithmetic, all-or-nothing | UMEM-01, SYS-06, SYS-10, STRESS-04 |
| time-of-check/time-of-use on user data | user structures are copied into kernel memory once and decoded from the copy | SYS-05..08 |
| oversized lengths, integer overflow | lengths bounded before use; checked add/mul; 32-bit handles with upper bits rejected | SYS-04..07 |
| capability amplification via transfer | transfer cannot add rights; copy needs DUPLICATE; rollback restores authority | SYS-02, SYS-12, STRESS-02 |
| authority loss on failure | all-or-nothing send; rollback; failed copy-out releases delivered handles | SYS-11, RES-09, RES-10 |
| resource exhaustion | bounded queues, quotas, waiter limits; failures are explicit | RES-01..04, BLK-13 |
| stuck or lost waiters (denial of service) | deadlines, cancellation, wake on close; no unbounded wait without cancel | BLK-03..10, 16, 17 |
| use after close or double release | generation checks; exactly-once destruction; ledger underflow detection | RES-07, RES-08, ACC-02 |
| concurrency races | leaf locks; eventcount; stress runs | STRESS-01..03 |

Known weaknesses: waiters are woken by broadcast (a hostile space with many waiters costs wake-ups, bounded by
the waiter limits); direct Batch 3/4 state changes do not wake waiters until the deadline; the
post-consume copy-out fault drops the message; thread-safety evidence is stress testing on x86-64 Linux in
a Debug build only. This document is an analysis, not an audit.
