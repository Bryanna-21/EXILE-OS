# Exile OS integration status

This directory contains the experimental OMNI-Kernel Batch 4 package.

- Hosted IPC prototype (channels, bounded queues, capability transfer, cancellation, closure) with a
  test harness only.
- Not a bootable kernel; no kernel image is produced. No hardware isolation, no syscall or scheduler
  integration, no production ABI, no network IPC.
- Not integrated into the Exile OS build.
- Does not replace or modify Kernel/ or ExileKernel/.
- Does not modify Documentation/OMNI-Kernel/Batch-1, Meta/Experiments/OMNI-Kernel-Batch-2 or
  Meta/Experiments/OMNI-Kernel-Batch-3; it compiles their sources and tests from their existing
  location without changing them.
- Zig 0.15.2 verification with an official release remains pending on the receiving machine; see
  REPORT.md for the compiler that was actually used.
