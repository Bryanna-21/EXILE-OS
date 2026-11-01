# Test plan

Totals from the last full run (see REPORT.md): Batch 6: 53 named tests plus the root test block (54 reported by the Zig
test runner); the unmodified regression suites add Batch 5 (57), Batch 4 (55), Batch 3 (57), Batch 2 (59): 282 in all.

Every Batch 6 test is HOSTED. Tests whose behaviour stands in for hardware or a scheduler are labelled SIM in their name.
No test runs on real hardware, an emulator or a real scheduler.

| required area | test ids |
|---|---|
| architecture register mapping, syscall argument validation | ARCH-01..04, ENT-01, ENT-05, ENT-06 |
| thread-state transition validity | TS-01, TS-02, TS-04 |
| blocking and waking | WK-01..07, IPC-01, IPC-02, IPC-09 |
| cancellation and timeout races | WK-08, IPC-03, IPC-04, IPC-12, IPC-13 |
| IPC wake-up and scheduler coordination | IPC-01..06, IPC-09, IPC-11, IPC-14 |
| thread termination while blocked | WK-05, IPC-07, IPC-08, PR-01, PR-02, PR-03 |
| address-space ownership validation | IPC-10, ENT-02, ENT-03, UM-05 |
| user-memory copy faults | UM-01..08 |
| invalid syscall and interrupt states | ARCH-05..08, ENT-02, ENT-04, ENT-06 |
| resource cleanup, process and thread termination | PR-01, PR-02, PR-04, IPC-06, IPC-07 |
| context-switch contract | CTX-01 |

Suites: `arch_tests` 8, `sched_tests` 12, `ipc_tests` 14, `entry_tests` 6, `mem_tests` 8, `proc_tests` 5.
Randomised: ENT-04 (seeds 1..16, 300 calls each; failing seed printed). Exhaustive: TS-01 (49 pairs), WK-06 (384
interleavings), ARCH-08 (all trap x context). Mutation check done by hand: disabling the ticket comparison in
`Sched.blockLocked` made WK-02 fail; the change was reverted.

Not tested: real hardware, emulators, interrupts, SMP, real page tables, riscv64, Batch 5 blocking syscalls through the
scheduler (specified only), more than 4 CPUs, 64+ threads concurrently.

Run: `bash scripts/verify.sh`, or `zig build test --summary all` (Zig 0.15.2).
