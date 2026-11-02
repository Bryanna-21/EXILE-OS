# Exile OS integration status

This directory contains the experimental OMNI-Kernel Batch 7 package.

- A `no_std` Rust crate for x86-64: real context-switch assembly, a cooperative uniprocessor scheduler, a SYSCALL entry
  stub with Rust dispatch, and user-address validation.
- **Executed only on a Linux x86-64 host** by the test suite. **Never booted**, never run in ring 0, never run under
  QEMU. The SYSCALL entry stub has never been executed.
- **Not integrated** into `ExileKernel/` or `Kernel/`, and not part of the Exile OS build. Neither was modified.
  `ExileKernel/` lacks the IDT, TSS, user segments, allocator, timer and paging that this code needs; its own roadmap
  schedules them first. See `docs/ARCHITECTURE.md`.
- No preemption, no hardware address-space isolation, no ARM64, and the Batch 3-5 IPC layer (Zig, hosted) is not
  connected.
- Syscall numbers 0x1F0-0x1F7 are provisional and are not an ABI. The register convention is the Batch 5 proposal, which
  differs from the one `Kernel/` uses; the choice is still open.
- Does not modify Documentation/OMNI-Kernel/Batch-1 or Meta/Experiments/OMNI-Kernel-Batch-2 to -6.
- Verified with the host rustc 1.97.0, not the 1.91.1 that `ExileKernel/scripts/build.py` pins.
