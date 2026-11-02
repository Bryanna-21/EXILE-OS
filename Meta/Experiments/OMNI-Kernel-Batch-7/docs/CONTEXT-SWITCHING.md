# Context switching

Source: `kernel/arch/x86_64/context.S`, `kernel/context.rs`. This is the one part of Batch 7 that is real machine code
**executed** in the tests: on the Linux x86-64 host the test binary switches between real stacks with it.

## `omni_switch_context(save_rsp: *mut usize, new_rsp: usize)`

```
pushfq; push rbp rbx r12 r13 r14 r15        save on the current stack
mov %rsp,(%rdi)                             record the stack pointer
mov %rsi,%rsp                               load the other stack
pop r15 r14 r13 r12 rbx rbp; popfq; ret     restore and resume
```
Saved: rflags and the System V callee-saved registers (rbx, rbp, r12-r15) plus the return address implied by `call`.
Everything else is caller-saved and dead across the call, so a switch made by a function call needs no more.

**Not saved, and why this is acceptable here:** the address space (the scheduler loads it before switching, see
`SCHEDULER-INTEGRATION.md`); FPU / SSE / AVX state (all vector registers are caller-saved, so nothing live crosses a
call; a *preemptive* switch from an interrupt would have to save them, and this batch has no preemption); `mxcsr` and
the x87 control word (kernel code does not change them); segment bases (`fs`/`gs` are per-CPU, not per-thread, in this
design); debug registers.

## Initial frame of a new thread

Built by `init_stack`, lowest address first: `r15=hook_arg r14=exit_hook r13=arg r12=entry rbx=0 rbp=0
rflags=0x202 ret=omni_thread_trampoline`. The first switch into the thread pops these and `ret`s into the trampoline:

```
mov %r13,%rdi ; call *%r12        entry(arg)
mov %r15,%rdi ; call *%r14        exit_hook(sched)  -> exit_current(0), never returns
ud2                               unreachable guard
```
The whole stack is zeroed first (no data leaks from a previous owner), so `rbp = 0` ends frame-pointer walks.

## Stack alignment

The saved rsp is always 0 mod 16 (seven pushes after a `call` that left rsp = 8 mod 16). After the final `ret` the
trampoline therefore runs with rsp = 0 mod 16 and its `call` leaves the entry function with rsp = 8 mod 16, as the
ABI requires. Tests: `ctx_01` (layout and alignment arithmetic), `ctx_04` (an assembly entry function reads its own
rsp and reports 8).

## Safety checks around the switch

* **Canary:** the lowest 8 bytes of each stack hold a constant; checked at every switch-in and after every switch-out.
  A changed value frees the thread and it is never resumed (`ctx_06`).
* **Saved-rsp validation:** 16-byte aligned, at least 8 bytes above the stack base, and `rsp + 64 <= top`, overflow-safe
  (`ctx_02`, `ctx_07`). There is no guard page: a stack overflow that skips past the canary is not detected.
* **No switch into dead or invalid threads:** only Runnable slots with a matching generation are dequeued; a freed
  slot is Free and cannot be queued (`ctx_08`, `sch_11`).
* **State preserved:** `ctx_03` runs three threads that load distinct values into rbx, rbp, r12-r15 and the ID flag
  in rflags, yield through the real switch three times, and compare on return; all 9 results are 0.
  I also broke the switch (replaced one `pop %r12` with `pop %rax`) and the test binary crashed with SIGSEGV, which
  is how that mutation shows up.

## What a hosted run does not prove

It proves the instruction sequence, frame layout and alignment are correct under the Linux user-mode ABI. It does not
prove behaviour in ring 0: interrupt state after `popfq`, shadow stacks (CET) or other hardware that inspects `ret`
targets, a per-CPU kernel stack, or an NMI / double-fault stack. No interrupt arrives during any switch in these tests.
