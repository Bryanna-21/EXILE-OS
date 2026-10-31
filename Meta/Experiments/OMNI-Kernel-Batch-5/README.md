# OMNI-Kernel Batch 5: kernel-facing IPC integration and syscall boundary

Experimental, hosted. This package adds a narrow kernel-facing interface and a proposed syscall boundary
on top of the Batch 4 IPC core. **It is not integrated into a bootable kernel**: no kernel image is produced,
there is no architecture entry stub, no scheduler, and no real user/kernel memory. Blocking is modelled with
OS threads in a hosted process.

## Contents

| path | purpose |
|---|---|
| `src/kernel_ipc.zig` | `KernelIpc`: the kernel-facing interface (send, recv, endpoint and cancel management, wake-ups, audit) |
| `src/syscall.zig` | proposed syscall numbers, argument decoding and validation, user-memory boundary, status codes |
| `src/wait_queue.zig` | bounded eventcount wait model with deadlines |
| `src/accounting.zig` | waiter ledger, policy, counters, audit |
| `src/hosted_user_memory.zig` | simulated user memory with fault injection (layout deviation: an extra file beyond the requested list) |
| `src/tests.zig`, `src/tests/` | Batch 5 tests (57) |
| `docs/` | integration, syscall contract, scheduler contract, accounting, security analysis, failure model, test plan |
| `scripts/verify.sh` | full verification |
| `manifest.json` | SHA-256 and size of every file |

Batches 2, 3 and 4 are compiled from their existing directories and are not modified; their test suites are
re-run unchanged as regressions. `Kernel/` and `ExileKernel/` are not touched.

## Build and test

Requires Zig 0.15.2.

```
cd Meta/Experiments/OMNI-Kernel-Batch-5
zig build test --summary all
bash scripts/verify.sh          # ZIG=/path/to/zig if zig is not on PATH
```

`verify.sh` exits 0 when every executed check passed, 1 on a real failure, 2 when a required tool is missing.

## Dependency note

Batch 5 depends on Batch 4 (`Meta/Experiments/OMNI-Kernel-Batch-4`). The Batch 5 branch is stacked on the
Batch 4 branch; merge Batch 4 first.

See REPORT.md for what is implemented, verified, specified only, and deferred.
