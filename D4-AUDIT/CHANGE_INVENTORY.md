# D4 change inventory (against the frozen D3 tree; hashes in `D3_SOURCE_SHA256SUMS.txt`)

D3 baseline here = the D3 tree built in this session (A1/C1 70 tests + D1-D3 108 tests = 178). It was NOT compared with your `EUK-D3-INSPECT` tree, which is not available to this session.

## New files (6 source + 2 docs)
src/irq.rs, src/preempt.rs, src/timer.rs, src/d4_demo.rs, src/arch/x86_64/pic.rs, src/arch/x86_64/trap.rs; docs/INTERRUPTS_AND_PREEMPTION.md (+ this inventory and VERIFICATION_RESULTS.md outside the tree)

## Modified files (7)
- src/dispatch.rs: IrqSpinLock in KernelSlot; IrqOff around reschedule/block/sleep/join; check_may_block; preempt_disable/preempt_enable; irq_exit_reschedule; 5 new real-switch tests (`d4_*`)
- src/kthread.rs: Error::{InterruptContext, PreemptDisabled, PreemptCount}
- src/kernel.rs: declares irq, preempt, timer, pic, trap, d4_demo (cfg); after_boot selects D4 demo
- scripts/build.py: `--d4 {demo,fault_ud,fault_gp,nopreempt}`
- scripts/boot_test.py: D4 demo (pc, q35) and three expected-failure cases
- scripts/test.py: D1-D4 module set, no_std check over it
- docs/THREADS.md: status line only

## Unchanged (byte-identical to the D3 reference)
src/d3_demo.rs, sched.rs, thread.rs, runqueue.rs, spin.rs, kstack.rs, context.rs, switch.rs, gdt.rs, idt.rs, cpu.rs, and all A1/C1 sources, entry.S, linker.ld.
