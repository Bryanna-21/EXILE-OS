# Exile OS integration status

This directory contains the experimental OMNI-Kernel Batch 5 package.

- Hosted, kernel-facing IPC interface and proposed syscall boundary, with a test harness only.
- Not a bootable kernel; no kernel image is produced. No hardware isolation, no architecture syscall
  stubs (x86-64 and ARM64 adapters are specified, not implemented), no scheduler integration, no
  production ABI, no network IPC.
- Not integrated into the Exile OS build.
- Does not replace or modify Kernel/ or ExileKernel/.
- Does not modify Documentation/OMNI-Kernel/Batch-1 or Meta/Experiments/OMNI-Kernel-Batch-2, -3, -4; it
  compiles their sources and tests from their existing location without changing them.
- Depends on Batch 4, which must be merged first.
- Zig 0.15.2 verification with an official release remains pending; see REPORT.md for the compiler that
  was actually used.
