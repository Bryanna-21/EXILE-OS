# Exile OS integration status

This directory contains the experimental OMNI-Kernel Batch 3 package.

- Hosted capability lifecycle, revocation and lifetime model with a test harness only.
- Not a bootable kernel; no kernel image is produced.
- Not integrated into the Exile OS build.
- Does not replace or modify Kernel/ or ExileKernel/.
- Does not modify Documentation/OMNI-Kernel/Batch-1 or Meta/Experiments/OMNI-Kernel-Batch-2; it
  compiles the Batch 2 sources and tests from their existing location without changing them.
- Zig 0.15.2 verification with an official release remains pending on the receiving machine; see
  REPORT.md for the compiler that was actually used.
