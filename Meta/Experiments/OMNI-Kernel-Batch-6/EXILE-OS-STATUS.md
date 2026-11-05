# Exile OS integration status

This directory contains the experimental OMNI-Kernel Batch 6 package.

- Hosted models and contract checkers for the architecture and scheduler boundary of OMNI-Kernel IPC, with a test
  harness only.
- Not a bootable kernel; no kernel image is produced. No real syscall entry (x86-64 `syscall`, ARM64 `svc`), no real
  context switch, no scheduler, no interrupt handling and no MMU code. These are specified in `docs/`, not implemented.
- Not integrated into the Exile OS build.
- Does not replace or modify Kernel/ or ExileKernel/. It was written against the actual source of both (see
  `docs/ARCHITECTURE-INTERFACE.md`): the existing Kernel/ uses a different syscall register convention from the one
  proposed in Batch 5, and that difference is documented, not resolved.
- Does not modify Documentation/OMNI-Kernel/Batch-1 or Meta/Experiments/OMNI-Kernel-Batch-2 to -5; it compiles their
  sources and tests from their existing location without changing them.
- No production syscall ABI is introduced. Syscall numbers, layouts and status codes remain those of Batch 5 (provisional).
- Depends on Batches 4 and 5, which must be merged first.
- Zig 0.15.2 verification with an official release remains pending; see REPORT.md for the compiler that was actually used.
