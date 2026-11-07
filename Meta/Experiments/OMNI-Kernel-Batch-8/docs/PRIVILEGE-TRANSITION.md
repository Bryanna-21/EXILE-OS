# Ring 3 transition (variant `ring3`)

Only built with `--cfg omni8_ring3`. **Compiled and statically checked; never executed.** Nothing in the base variant
touches ring 3.

## Sequence

1. Allocate two frames; copy the ring-3 test program (`omni8_user_blob`, position independent, in `probe.S`) into one.
2. Build a second address space: the same kernel mappings as the kernel root, plus user code at `0x4000_0000`
   (user, read, execute) and a one-page user stack at `0x4001_0000` (user, read/write, no execute). The mapper refuses any
   user mapping outside the window and any write+execute mapping; the kernel re-checks the resulting translations.
3. Set TSS.rsp0 and the per-CPU block (`kernel_rsp`) to a dedicated kernel stack; call Batch 7 `msr::init_syscall`
   (EFER.SCE, STAR, LSTAR, FMASK, KERNEL_GS_BASE).
4. Install the Batch 7 `Kernel`; set a scheduler space hook that loads CR3 for the thread's space.
5. Spawn a kernel thread in space 1. When scheduled, the hook switches to the user CR3 and the thread builds a validated
   `iretq` frame (`usermode::iret_frame`: canonical, non-null, 8-aligned stack, inside the window) and executes
   `iretq` to ring 3.

## What the ring-3 program does

`syscall(0x1F4)` (space id), `int $0x81` through the DPL 3 gate, `syscall(0x1F1)` (exit) with the space id as the code.

## What a pass would show

`OMNI8 RING3: cpl3 int81 seen, syscall exit code=1`: the dispatcher records `int 0x81` only when the saved CS has RPL 3,
and the exit code can only be 1 if the SYSCALL entry stub, `Kernel::syscall`, `SYS_SPACE_SELF` and `SYS_EXIT` all ran.

## Negative test

`ring3-fault` runs a program that reads `0x100000` (kernel text) from ring 3. Expected: #PF with present+user in the error
code, a report whose CS line says `ring3`, exit status 35.

## Limits and risks

* One user thread. The Batch 7 stub uses one per-CPU kernel stack, so two user threads blocked in the kernel would
  corrupt each other. The demo has exactly one thread and it exits.
* Ring 3 shares FPU/SSE state with the kernel (`SCHEDULER.md`).
* No SMEP/SMAP, no PCID, no user-memory copying (the `UserBacking` is a stub that refuses everything; none of the five
  enabled syscalls copies user memory).
* The window and layout are demo values, not an ABI.
