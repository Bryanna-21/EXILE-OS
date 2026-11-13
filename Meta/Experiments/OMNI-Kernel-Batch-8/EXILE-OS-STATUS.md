# Exile OS integration status

This directory contains the experimental OMNI-Kernel Batch 8 package.

- A bare-metal x86-64 Multiboot v1 kernel image plus a hosted `no_std` library: GDT/TSS, IDT and exception reports,
  W^X page tables, a frame allocator, a timer test, a scheduler scenario on Batch 7, and an optional ring-3/SYSCALL stage.
- **Built and statically checked; never booted.** No QEMU, GRUB or `x86_64-unknown-none` target was available. The QEMU
  tests are written and report NOT RUN. Hosted unit tests (65) pass.
- **Not integrated** into `ExileKernel/` or `Kernel/` and not part of the Exile OS build. Neither was modified, nor were
  Batches 1 to 7.
- Built with rustc 1.97.0 for the host target as a documented fallback, not the 1.91.1 that ExileKernel pins.
- No asynchronous preemption (safe-point preemption requests exist in the `preempt` variant, not booted), no SMP, no ARM64. Syscall numbers remain provisional and the register convention differs from `Kernel/`.
