# x86-64 SYSCALL entry stub (AT&T syntax). Included with global_asm!.
#
# NEVER EXECUTED in Batch 7: it needs ring 0, a GDT/TSS and MSR setup that ExileKernel does
# not have yet. It is assembled, and its bytes and frame layout are checked by tests.
#
# Per-CPU block, addressed through %gs after swapgs (KERNEL_GS_BASE points at it):
#   gs:0  kernel_rsp   top of this CPU's kernel stack, 16-byte aligned
#   gs:8  user_rsp     scratch for the user stack pointer
#
# On entry (hardware): rcx = user rip, r11 = user rflags, rax = syscall number,
# rsp = USER stack (not switched), IF cleared by FMASK.
#
# Frame layout built here, lowest address first (matches SyscallFrame in syscall.rs):
#   r15 r14 r13 r12 rbp rbx r11 r10 r9 r8 rcx rdx rsi rdi rax user_rsp
    .text
    .globl omni_syscall_entry
    .type omni_syscall_entry, @function
omni_syscall_entry:
    swapgs
    movq %rsp, %gs:8
    movq %gs:0, %rsp
    pushq %gs:8
    pushq %rax
    pushq %rdi
    pushq %rsi
    pushq %rdx
    pushq %rcx
    pushq %r8
    pushq %r9
    pushq %r10
    pushq %r11
    pushq %rbx
    pushq %rbp
    pushq %r12
    pushq %r13
    pushq %r14
    pushq %r15
    movq %rsp, %rdi
    cld
    callq omni_syscall_common
    popq %r15
    popq %r14
    popq %r13
    popq %r12
    popq %rbp
    popq %rbx
    popq %r11
    popq %r10
    popq %r9
    popq %r8
    popq %rcx
    popq %rdx
    popq %rsi
    popq %rdi
    popq %rax
    popq %rsp
    swapgs
    sysretq
    .globl omni_syscall_entry_end
omni_syscall_entry_end:
    .size omni_syscall_entry, .-omni_syscall_entry
    .text
