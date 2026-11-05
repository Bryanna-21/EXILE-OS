# Test plan

Counts are from the last full run (see REPORT.md): Batch 5: 57 tests; with the unmodified regression suites of
Batch 4 (55), Batch 3 (57) and Batch 2 (59) the total is 228.

| required area | test ids |
|---|---|
| authorization | SYS-02, SYS-12 |
| invalid endpoint | SYS-03 |
| send/recv | SYS-01, BLK-01, BLK-02, STRESS-01 |
| queue exhaustion | RES-01, RES-02 |
| resource exhaustion | RES-03, RES-04, RES-09..11, BLK-13 |
| capability transfer and rollback | SYS-12, RES-04, STRESS-02 |
| endpoint destruction | RES-07, RES-08, BLK-08, BLK-09 |
| cancellation | BLK-05..07, BLK-17 |
| timeout | BLK-03, BLK-16 |
| blocked sender / receiver | BLK-01, BLK-02, BLK-14 |
| wake-up correctness | WQ-01..05, BLK-12, BLK-14..17 |
| concurrent senders/receivers | STRESS-01, BLK-14 |
| races | STRESS-02, STRESS-03, BLK-16, BLK-17 |
| repeated create/destroy | RES-06, STRESS-03 |
| accounting consistency | RES-05, ACC-01..04 |
| invalid syscall args | SYS-04..08, STRESS-04 |
| integer overflow | SYS-06, UMEM-01 |
| boundary validation | SYS-07, SYS-08, UMEM-01 |
| failure recovery | RES-12 |

Suites: `units.zig` 11, `syscall_tests.zig` 12, `wait_tests.zig` 17, `resource_tests.zig` 12, `stress_tests.zig` 4.
STRESS-04 is a seeded fuzz over seeds 1..24 (400 calls each); a failing seed is printed.
Run: `bash scripts/verify.sh`, or `zig build test --summary all`.
