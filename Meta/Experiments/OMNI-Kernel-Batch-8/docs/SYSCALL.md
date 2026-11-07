# Syscall infrastructure

Batch 8 adds nothing to the syscall path; it supplies what Batch 7 needed and, in the `ring3` variant, invokes it.

* Entry: Batch 7 `omni_syscall_entry` (swapgs; `gs:0` kernel stack; 128-byte `SyscallFrame`; `sysretq`).
* Setup: `msr::init_syscall` writes EFER.SCE, STAR (kernel CS 0x08, sysret base 0x10), LSTAR, FMASK (clears IF, DF, TF, AC,
  NT on entry) and KERNEL_GS_BASE (the per-CPU block).
* Enabled calls: 0x1F0 yield, 0x1F1 exit, 0x1F2 sleep, 0x1F3 thread_self, 0x1F4 space_self. Debug calls 0x1F5 to 0x1F7
  stay off. Numbers are provisional, not an ABI.
* Register convention: rax number; rdi, rsi, rdx, r10, r8, r9 arguments (Batch 5 proposal), different from `Kernel/`'s.
  The choice is still open.
* `validate_frame` rejects a user rip or rsp at or above the user top and sanitises RFLAGS on return.

## What may be claimed

A syscall counts as invoked from ring 3 only if the ring-3 variant boots under QEMU and prints
`OMNI8 RING3: cpl3 int81 seen, syscall exit code=1`. In this environment it was **not run**, so no claim is made.
