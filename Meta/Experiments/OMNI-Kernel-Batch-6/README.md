# OMNI-Kernel Batch 6: architecture and scheduler integration boundary

Experimental, hosted. This package defines and tests the boundary between OMNI-Kernel's IPC layer (Batches 2 to 5) and
the architecture and scheduler layers a real kernel would provide. **It does not implement hardware syscall entry,
context switching or a scheduler, and it is not integrated into any kernel.** The modules are hosted models and
contract checkers; tests that stand in for hardware or a scheduler say SIM in their names.

## Contents

| path | purpose | nature |
|---|---|---|
| `src/arch.zig` | saved register state, two syscall register conventions, entry validation, execution-context and trap rules | hosted checker |
| `src/entry.zig` | architecture-independent syscall entry: trusted caller lookup, user-memory binding, call to Batch 5 `dispatch` | hosted simulation |
| `src/thread_state.zig` | thread states and the transition table | specification + checker |
| `src/sched.zig` | thread table, block/wake/kill protocol without lost wake-ups | hosted scheduler MODEL |
| `src/ipc_sched.zig` | blocking IPC through the scheduler model, process termination | hosted |
| `src/user_memory.zig` | page model, `UserMemory` bridge bound to one address space, registry | hosted simulation |
| `src/context.zig` | context-switch preconditions and bookkeeping, register sets as data | contract checker |
| `src/omni5.zig` | re-export of the unmodified Batch 5 sources | glue |
| `src/tests.zig`, `src/tests/` | 53 named tests | hosted |
| `docs/` | architecture interface, scheduler integration, context switching, user-memory integration, security analysis, test plan | |
| `scripts/verify.sh`, `manifest.json` | verification and file hashes | |

Batches 2 to 5 are compiled from their own directories (Batch 5 sources are copied byte-for-byte into the build
cache) and are NOT modified; their test suites run unchanged as regressions. `Kernel/` and `ExileKernel/` are not touched.
No syscall number, structure or status code of Batch 5 is changed.

## Build and test

Requires Zig 0.15.2.

```
cd Meta/Experiments/OMNI-Kernel-Batch-6
zig build test --summary all
bash scripts/verify.sh          # ZIG=/path/to/zig if zig is not on PATH; OMNI_BASE_REF=<ref> to also check the diff
```

`verify.sh` exits 0 when every executed check passed, 1 on a real failure, 2 when a required tool is missing.

## Dependencies

Batch 6 builds on Batch 5, which builds on Batch 4. Neither is on `origin/main` when this was written; merge in order
(4, 5, 6).

See REPORT.md for what is implemented, partial, specified only and deferred, and for the limitations.
