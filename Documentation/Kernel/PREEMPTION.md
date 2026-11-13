# Kernel preemption: current mechanism, constraints and verification status

Status date: 2026-11-13. Written from a read of the code at commit `b426330b`. Nothing in this document was observed
at runtime: no kernel build, image or QEMU boot has been possible in the authoring environment (see "Evidence").

## 1. Finding: timer-driven preemption already exists

The Serenity-derived `Kernel/` already preempts threads from the timer interrupt, using deferred rescheduling. No
second mechanism was added. The OMNI Batch 8 kernel (`Meta/Experiments/OMNI-Kernel-Batch-8`) is a separate
experiment; its own docs state "No preemption" and "Built and statically checked; never booted". Its `src/preempt.rs`,
named in the task brief, is not in this repository, so it was not read.

## 2. The path, with integration points

| Step | Where | What happens |
|---|---|---|
| Timer source | `Kernel/Time/TimeManagement.cpp` (PIT/HPET/APIC/RTC setup; `OPTIMAL_TICKS_PER_SECOND_RATE` = 250 in `TimeManagement.h`) | Hardware timer is programmed to about 250 Hz. |
| IRQ handler | `TimeManagement::update_time()` (line 493), then `system_timer_tick()` (line 550) | Only the bootstrap processor does timekeeping; `system_timer_tick()` runs `TimerQueue::fire()` then `Scheduler::timer_tick()`. |
| Tick accounting | `Scheduler::timer_tick()` (`Kernel/Tasks/Scheduler.cpp`, line 442) | Runs with interrupts disabled and `in_irq` set (both `VERIFY`d). Calls `Thread::tick()` (`Thread.cpp:620`), which charges the tick to user or kernel time and decrements `m_ticks_left`. |
| Slice expiry | same function | If `tick()` returns false (slice used up) and another thread is runnable (`peek_next_runnable_thread()`), it calls `Processor::invoke_scheduler_async()`. If nobody else is runnable the thread gets a fresh slice and nothing switches. |
| Deferral | `Kernel/Arch/Processor.h:150` | `invoke_scheduler_async()` only sets `m_invoke_scheduler_async = true`. No context switch happens inside the IRQ handler. |
| Switch point 1: trap exit | `Kernel/Arch/x86_64/Processor.cpp`, trap-exit code around line 1282 | After leaving the IRQ's critical section, if `!m_in_irq && !m_in_critical`, `check_invoke_scheduler()` runs. |
| Switch point 2: leaving a critical section | `ProcessorBase::do_leave_critical()` / `clear_critical()` (`Kernel/Arch/Processor.cpp`) | Same check when the outermost critical section ends outside an IRQ. A switch requested while a spinlock is held therefore waits until the lock is released. |
| The switch | `Scheduler::invoke_async()` -> `pick_next()` -> `context_switch()` (`Scheduler.cpp` lines 204-330) | Takes `g_scheduler_lock`, pulls the next thread from `g_ready_queues`, sets its slice via `time_slice_for()`, switches. |
| Voluntary path | `Scheduler::yield()` (line 255) | If in an IRQ or critical section it defers via `invoke_scheduler_async()`, otherwise switches immediately. |

Time slice: `time_slice_for()` gives 2 ticks to a normal thread and 1 to idle, so about 8 ms at 250 Hz. The in-code
comment says "4ms"; at 250 ticks/s one tick is 4 ms, so two ticks are 8 ms.

## 3. Answers to the brief's design questions

- **Where timer interrupts enter:** the hardware timer handler registered in `TimeManagement` (see table).
- **Counting and acknowledgement:** `update_time()` advances time; the generic interrupt dispatch acknowledges it (`handler->eoi()`, `Kernel/Arch/x86_64/Interrupts.cpp:326`).
  Per-thread tick counts live in `Thread::tick()`.
- **How the next thread is picked:** `Scheduler::pull_next_runnable_thread()` over `g_ready_queues`, priority buckets,
  first eligible thread (not active on another CPU, affinity allows this CPU) in list order, idle thread if none.
- **Locks that may be held at interrupt time:** `g_scheduler_lock` and `g_ready_queues` are spinlocks. Taking a
  spinlock enters a critical section and disables interrupts on that CPU, so an IRQ cannot arrive on the same CPU
  while the lock is held. The tick path never acquires `g_scheduler_lock` in the ordinary case. Two exceptions, both
  safe for that reason: `peek_next_runnable_thread()` takes the `g_ready_queues` lock, and the
  `should_die()` branch takes `g_scheduler_lock`. On SMP another CPU can hold either lock, so the IRQ may spin; that
  spin is bounded by the other CPU's short critical section.
- **Interrupt context vs thread context:** `m_in_irq` and `m_in_critical` per processor. A context switch is refused
  while either is nonzero; the request is remembered and honoured at the switch points above.
- **Existing safe rescheduling points:** trap exit and outermost critical-section exit (rows above).
- **Does deferred rescheduling already exist:** yes, it is the design.

## 4. Constraints any change must keep

1. No context switch inside the IRQ handler; request it with `invoke_scheduler_async()`.
2. `Scheduler::timer_tick()` must keep its `VERIFY`s on interrupts-disabled and `in_irq`.
3. Do not take `g_scheduler_lock` from the tick path beyond the existing `should_die()` case.
4. Nested critical sections must keep deferring switches until the outermost one ends.
5. SMP: `smp_process_pending_messages()` is part of trap exit. Timekeeping stays on the bootstrap processor only.
   Cross-CPU scheduling behaviour was not changed or tested here.

## 5. What this change adds

| File | Purpose | Status |
|---|---|---|
| `Tests/Kernel/TestPreemption.cpp` (+ line in `Tests/Kernel/CMakeLists.txt`) | Guest runtime test: more CPU-bound threads than CPUs, no yields; main thread sleeps. Prints `PREEMPT-TEST:` lines. | Syntax-checked and run on the **host** only. The host run shows the test logic works under Linux's scheduler; it is **not** evidence about this kernel. |
| `Base/etc/SystemServer.ini` (`[TestRunner@ttyS0]`) | Uses the repository's standard `/home/anon/Tests/run-tests-and-shutdown.sh` runner in `self-test` mode. The runner discovers installed LibTest executables, writes `test-results.log`, emits `TEST-SUITE-RESULT: failed=N` on its configured serial output, and shuts down after the suite. | Configuration inspected; guest execution remains unverified. |
| `Meta/Exile/verify-preemption-serial.py` + `Meta/Exile/tests/test_verify_preemption_serial.py` | Checks a serial log for panic markers, exactly one well-formed zero-failure `TEST-SUITE-RESULT` marker, the preemption result line, `threads > cpus`, no starvation, the fairness bound, and completion marker. | 17 unit tests on **synthetic** logs pass. They test the parser only. |
| `.github/workflows/kernel-preemption-boot.yml` | Manual-trigger job: build with `Meta/exile.sh`, boot with 1 CPU, verify, upload logs. | Workflow file inspected, but YAML syntax was **not independently validated** (`PyYAML` and `actionlint` unavailable). **Never run.** |

No file under `Kernel/` was changed, and no scheduler policy was changed. Reason: the existing mechanism already
meets the brief's requirements 1-8 on reading, and changing it without being able to build or boot it would add risk
with no way to check it.

## 6. Evidence: what was and was not demonstrated

| Definition-of-done item | State |
|---|---|
| Real kernel builds | **Not demonstrated.** No cross toolchain (`Toolchain/Local` absent), no network for the toolchain build. |
| Bootable image | **Not demonstrated.** |
| Boots in QEMU | **Not demonstrated.** No QEMU, GRUB or xorriso here. |
| Timer interrupts in runtime logs | **Not demonstrated.** |
| Actual preemption shown | **Not demonstrated.** `TestPreemption` is written to show it; it has not run on this kernel. |
| Regression tests | Host-side tests of the new verifier pass (17, using synthetic logs). Kernel regression tests were not run. |
| CI reproduces the result | **Not demonstrated.** Workflow exists, never executed. |
| Documentation | This file. |

## 7. Known limits and open questions

- The workflow uses `Meta/exile.sh run x86_64 GNU`, sets `EXILE_CPUS=1` (the environment variable resolved by `exile_env("CPUS")` in `Meta/run.py`), and directs
  the build's QEMU working directory to `Build/x86_64`. In `ci` mode, serial output is captured from the command's
  stdout and debug output is expected in `Build/x86_64/debug.log`. These paths and the single-CPU configuration are
  derived from the runner source but remain unverified until the GitHub Actions job runs.
- The supported toolchain in `Toolchain/Dockerfile` is based on `ubuntu:22.10`, which is end of life. The workflow
  installs packages on `ubuntu-24.04` instead and builds the toolchain through `Meta/exile.sh`; whether that
  reproduces the pinned compiler is unconfirmed.
- With one CPU and threads that never block, `TestPreemption` fails by hanging, not by an assertion, if preemption is
  broken. The workflow's bounded `timeout` turns a hang into a nonzero run result; the verifier also reports missing
  completion evidence. The workflow preserves the original build/boot exit status rather than converting failures
  into success.
- Only x86_64 is in scope. aarch64 and riscv64 are present in the tree and were not examined. All unexamined
  architectures should be treated as unverified.
- The `Kernel/` tree also has the Clang 18 `AK` defects documented in
  `Documentation/Project-Audits/M0-HOST-BUILD-STABILISATION.md` (proposal not applied). A kernel build with Clang 18
  hits the same `AK/Platform.h` problem until the owner approves that fix.
