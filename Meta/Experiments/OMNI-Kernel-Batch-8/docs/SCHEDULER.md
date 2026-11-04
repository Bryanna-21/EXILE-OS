# Scheduler integration

Batch 8 does not modify Batch 7. The kernel owns one `omni_kernel7::syscall::Kernel`, whose embedded `Sched` is used for
every scheduling test, so the syscall path and the boot self-test exercise the same scheduler.

## Boot-time scenario (`selftest::run_order_scenario`)

Three kernel threads each record their number and yield three times. Expected order `0 1 2 0 1 2 0 1 2`; expected context
switches into threads 12 (three threads times four slices); every thread slot must be reclaimed afterwards. The same
function runs in the hosted tests on the real Batch 7 assembly (`selftest_tests`) and in the kernel after paging is on.
A second run on a full log is detected as a failure.

## No preemption

The timer handler counts ticks and sends EOI. It never calls the scheduler and never switches context. Interrupts are
enabled only for the bounded timer test, then disabled again with every IRQ masked. Preemption would need per-thread
saved interrupt frames, a lock discipline between interrupt and thread context, and FPU state handling; none is built.

## FPU and SSE

* Compiled code uses SSE2. Boot sets CR4.OSFXSR and OSXMMEXCPT, clears CR0.EM and (in `omni8_main`) CR0.TS, and checks
  CPUID for FXSR/SSE/SSE2 first.
* Every interrupt saves and restores the interrupted context's FPU/SSE state with `fxsave`/`fxrstor` in `isr_common`.
* A cooperative context switch is an ordinary function call, so caller-saved XMM registers need no saving. The x87 control
  word and MXCSR control bits are formally callee-saved in the System V ABI and Batch 7 does not save them. No Batch 8 code
  changes them.
* Threads have no private FPU state. Ring 3 code that uses SSE would leak register contents to and from the kernel.
  Not mitigated; the ring-3 demo does not use SSE.

## Limits

16 threads, 16 KiB stacks with a canary (no guard page), uniprocessor, no priorities, sleep granularity is the scheduler
tick that Batch 8 does not drive from the timer.
