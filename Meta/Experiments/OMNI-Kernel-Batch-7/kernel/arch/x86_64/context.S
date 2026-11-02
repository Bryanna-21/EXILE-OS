# x86-64 context switch (AT&T syntax, System V AMD64). Included with global_asm!.
#
# void omni_switch_context(usize *save_rsp /* rdi */, usize new_rsp /* rsi */)
#
# Saves rflags and the callee-saved registers (rbx, rbp, r12-r15) on the CURRENT stack,
# stores the resulting stack pointer in *save_rsp, loads new_rsp, restores the same set
# from the NEW stack and returns on it. All other registers are caller-saved by the ABI
# and are dead across the call. The saved stack pointer is always 16-byte aligned (the
# call made rsp = 8 mod 16; seven 8-byte pushes make it 0 mod 16).
#
# Not saved here, by design: the address space (the scheduler loads it), FPU/SSE state
# (every xmm register is caller-saved in this ABI, so a switch made by a call needs none).

    .text
    .globl omni_switch_context
    .type omni_switch_context, @function
omni_switch_context:
    pushfq
    pushq %rbp
    pushq %rbx
    pushq %r12
    pushq %r13
    pushq %r14
    pushq %r15
    movq %rsp, (%rdi)
    movq %rsi, %rsp
    popq %r15
    popq %r14
    popq %r13
    popq %r12
    popq %rbx
    popq %rbp
    popfq
    ret
    .size omni_switch_context, .-omni_switch_context

# First entry of a new thread: reached by the `ret` of omni_switch_context, with
#   r12 = entry function   (extern "C" fn(usize))
#   r13 = its argument
#   r14 = exit hook        (extern "C" fn(usize) -> !), called if the entry returns
#   r15 = the hook's argument
# rsp is 16-byte aligned here, so `call` leaves the callee with rsp = 8 mod 16 as the ABI
# requires.
    .globl omni_thread_trampoline
    .type omni_thread_trampoline, @function
omni_thread_trampoline:
    movq %r13, %rdi
    callq *%r12
    movq %r15, %rdi
    callq *%r14
    ud2
    .size omni_thread_trampoline, .-omni_thread_trampoline

