# OMNI-Kernel Batch 3: capability lifecycle, revocation and lifetime (hosted)

**Status: an experimental, hosted, architecture-neutral model with automated tests. Nothing here
boots. No architecture is operational. No security guarantee is claimed.**

Batch 3 builds on Batch 1 (specification, OMNI-ABI 0.1 draft) and Batch 2 (hosted core foundation).
It adds a `Kernel` that owns address spaces, objects and capabilities and implements their full
lifecycle, including revocation (Batch 1 option R2), in-flight operation pinning, resource accounting
and a stated concurrency model. It lives under `Meta/Experiments/` and changes nothing in `Kernel/`,
`ExileKernel/`, Batch 1 or Batch 2.

## What Batch 3 delivers

| Area                     | Where                         | Summary                                                                                                                                                              |
| ------------------------ | ----------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Generation-checked arena | `src/gen_arena.zig`           | One mechanism for all tables: generations never 0, slots retire on generation exhaustion, two-step reserve/commit allocation                                         |
| Kernel                   | `src/kernel.zig`              | `createSpace`, `closeSpace`, `createObject`, `lookup`, `derive`, `release`, `revoke`, `transfer`, `beginOp`/`endOp`/`opStillAuthorized`, `checkInvariants`, teardown |
| Reuse of Batch 2         | `build.zig`, `src/batch2.zig` | Batch 2 `error`, `ids`, `capabilities` (rights, types), `resources` are compiled unmodified from `../OMNI-Kernel-Batch-2/src/core`                                   |
| Tests                    | `src/tests/*.zig`             | Lifecycle, generation, revocation, accounting, allocation failure, concurrency, randomized reference model                                                           |
| Regression               | `build.zig`                   | The unmodified Batch 2 suite (59 tests) runs on every `zig build test`                                                                                               |
| Docs                     | `docs/`                       | `CAPABILITY-LIFECYCLE.md`, `REVOCATION-MODEL.md`, `CONCURRENCY.md`, `TEST-PLAN.md`, `PROPOSED-ABI-CHANGES.md`                                                        |
| Script                   | `scripts/verify.sh`           | Files, Zig version, format, build and test, manifest                                                                                                                 |

## What Batch 3 does not deliver

- No kernel image, boot code, assembly, scheduler, interrupt handling, page tables or system-call entry.
- No IPC integration: handles are moved between spaces by `transfer`, not by messages.
- No isolation: a hosted model enforces nothing against code in the same process.
- No forced object destruction, copy-transfer, R1 indirection, epochs, synchronous revocation,
  cancellation of running operations or derivation-depth bound.
- No fine-grained locking: one global mutex. No interrupt-context model.
- No release-mode, freestanding, aarch64, race-detector or multi-machine results.

## Architecture support status

Unchanged from Batch 2: x86-64 and ARM64 are **not implemented** and do not boot; RISC-V is not
addressed. Tests run on an x86-64 Linux host in Debug mode only.

## Toolchain requirements

- **Zig 0.15.2** (`build.zig.zon` sets `minimum_zig_version = "0.15.2"`; `scripts/verify.sh` requires
  `zig version` to print `0.15.2`).
- Bash and standard Unix tools for `scripts/verify.sh`; `python3` for the optional manifest check.
- The package expects its sibling `../OMNI-Kernel-Batch-2` to be present and unmodified.
- Tested configuration: a Zig 0.15.2 compiler built from source (tag `0.15.2`) **without LLVM**,
  Debug mode, Linux x86-64. That compiler lacks optimizing code generation, so results say nothing
  about release builds. See `REPORT.md`.

## Build and test commands

From this directory, with `zig` 0.15.2 on `PATH` (or `ZIG=/path/to/zig`):

```sh
zig build test --summary all     # Batch 3 suite and the unmodified Batch 2 suite
zig build fmt                    # check formatting
scripts/verify.sh                # files, version, format, tests, manifest
```

Set `ZIG_LOCAL_CACHE_DIR` to a directory outside the repository if you do not want `.zig-cache`
created in the tree. Compilation of each test executable took roughly 40 seconds in the authoring
environment.

`scripts/verify.sh` exit codes: 0 every executed check passed; 1 a check failed; 2 the Zig compiler was
not found.

## Reading order

1. This file.
2. `docs/CAPABILITY-LIFECYCLE.md`: operations, ownership, accounting, generations, Batch 1 comparison.
3. `docs/REVOCATION-MODEL.md`: tree rules, linearization points, in-flight operations.
4. `docs/CONCURRENCY.md`: ownership, lock rules, invariants, what the tests prove.
5. `docs/TEST-PLAN.md`: requirement to test mapping.
6. `docs/PROPOSED-ABI-CHANGES.md`: changes proposed for a later ABI draft (not applied).
7. `REPORT.md`: what was run, results, unresolved risks, untested claims.
