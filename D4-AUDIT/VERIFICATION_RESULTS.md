# D4 verification results (every line below was executed; raw logs in `verification/`)

Toolchain: rustc 1.97.0 (NOT the pinned 1.91.1: unobtainable here, proxy returned 403), QEMU 8.2.2, GRUB 2.12, gcc 13.3.0. Build env: `EUK_ALLOW_UNPINNED_RUSTC=1 EUK_TARGET=host`.

## Executed and passed
| Check | Result |
|---|---|
| `python3 scripts/test.py` | exit 0: A1/C1 modules 18+12+0+0+13+14+13 passed; D1-D4 binary **184 passed, 0 failed** (108 D1-D3 + 49 new D4 + 27 gdt/idt); `#![no_std] -D warnings` check of the D1-D4 modules passed |
| Host stability | 40 repeated runs of the D1-D4 binary, 0 failures |
| `python3 scripts/boot_test.py` | exit 0, 16 PASS lines (log: `verification/qemu_boot_suite.log`): normal pc+q35, 6 fault-injection cases, table_dirty, d3_demo pc+q35, **d4_demo pc+q35 (12 ordered checkpoints, exit 33)**, d4_fault_ud (exit 35, vector 6), d4_fault_gp (exit 35, vector 13 error=0x28), d4_nopreempt (exit 35, watchdog) |
| QEMU stability | d4 demo 20 runs (10 pc, 10 q35), 0 failures (tick/switch counts vary; only deterministic lines are asserted) |
| `scripts/check.py` (ELF64/Multiboot/entry/static link) | PASS on every image built by boot_test |
| `scripts/audit.py` | exit 0 (declaration gate only; not legal clearance) |
| rustfmt 1.97 `--check` | clean on every D1-D4 source file |
| Whitespace | no trailing whitespace in changed files (workspace is not a git repo, so `git diff --check` was not applicable) |
| D3 reference | 0 hash mismatches vs manifest; changed/added files listed in CHANGE_INVENTORY.md |

## Mutation checks (deliberate breakage in scratch copies, then discarded)
Detected with a clean assertion: host `sleep_early` rounding, `lock` not disabling IF, exit action ignoring preemption depth, missing blocking guard in `yield_now`, `reschedule` without IF-off, wrong PIC ICW4, wrong gate DPL, HARDIRQ lowered after the exit hook; QEMU: no `fxsave`, no `fxrstor`, a skipped `pop r8` (all: "register or SSE state corrupted across preemption").
Detected without a clean diagnostic: no EOI and no `irq::enable` in first-run (both hang until the harness timeout); wrong error-code flag for #GP (QEMU aborted before printing `error=0x28`).

## NOT executed / not met
- **Pinned Rust 1.91.1 build and QEMU run: not done (toolchain unobtainable).** The "pinned-toolchain kernel build" criterion is therefore NOT met.
- Comparison with the owner's `EUK-D3-INSPECT` baseline: not possible here.
- Real hardware, SMP, APIC/HPET, user mode, mutexes/semaphores/wait queues (Mission D phase 4), processes and ELF loading: not implemented.
- The unhandled-IRQ-ack path is host-tested with fake ports only; the stubs are executed only on QEMU.

## Findings
- `idt::has_error_code` correctly marks #CP (21) as having an error code; my first stub table omitted it and the host test `stub_error_code_flags_match_idt_policy` caught it before any boot.
