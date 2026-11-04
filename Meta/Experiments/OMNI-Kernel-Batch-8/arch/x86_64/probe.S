/* OMNI Batch 8 boot-time self-test probes and the ring-3 entry/test programs.
 *
 * Each omni8_probe_* routine records its own stack pointer and a resume address, then
 * executes ONE instruction that is expected to trap. The kernel's trap dispatcher, when it
 * has been told to expect that vector, records what it saw and rewrites the trap frame so
 * execution continues at the resume label with the stack as it was on entry. A routine
 * whose instruction does not trap simply falls through to the same `ret`.
 */
    .text
    .macro PROBE name
    .globl omni8_probe_\name
    .type omni8_probe_\name, @function
omni8_probe_\name:
    movq %rsp, omni8_probe_sp(%rip)
    leaq 1f(%rip), %rax
    movq %rax, omni8_probe_resume(%rip)
    .endm
    .macro PROBE_END name
1:  ret
    .size omni8_probe_\name, .-omni8_probe_\name
    .endm

    PROBE read          /* rdi = address */
    movq (%rdi), %rax
    PROBE_END read

    PROBE write         /* rdi = address, rsi = value */
    movq %rsi, (%rdi)
    PROBE_END write

    PROBE exec          /* rdi = address to call */
    callq *%rdi
    PROBE_END exec

    PROBE ud
    ud2
    PROBE_END ud

    PROBE bp
    int3
    PROBE_END bp

    PROBE de
    xorl %ecx, %ecx
    movl $1, %eax
    xorl %edx, %edx
    divl %ecx
    PROBE_END de

    /* Executable, readable kernel function used as the positive control for exec probes. */
    .globl omni8_probe_nop
    .type omni8_probe_nop, @function
omni8_probe_nop:
    ret
    .size omni8_probe_nop, .-omni8_probe_nop

/* void omni8_enter_user(const uint64_t frame[5]): load the iretq frame (RIP, CS, RFLAGS,
 * RSP, SS) as the stack and iretq. Never returns. */
    .globl omni8_enter_user
    .type omni8_enter_user, @function
omni8_enter_user:
    movq %rdi, %rsp
    iretq
    .size omni8_enter_user, .-omni8_enter_user

/* Ring-3 test program, copied into the user window. Position independent.
 *   1. syscall SYS_SPACE_SELF (0x1F4): result is the caller's address-space id
 *   2. int $0x81: software interrupt through the DPL-3 gate (kernel records ring 3)
 *   3. syscall SYS_EXIT (0x1F1) with the id from step 1 as the exit code
 */
    .section .rodata
    .globl omni8_user_blob_start
    .globl omni8_user_blob_end
omni8_user_blob_start:
    movl $0x1F4, %eax
    syscall
    movq %rax, %rbx
    int $0x81
    movq %rbx, %rdi
    movl $0x1F1, %eax
    syscall
    ud2
omni8_user_blob_end:

/* Negative ring-3 program: read kernel memory from CPL 3. Expected: #PF (present, user),
 * a fatal report naming ring 3, and a failing exit status. rbx = 0x100000 (kernel image). */
    .globl omni8_user_fault_blob_start
    .globl omni8_user_fault_blob_end
omni8_user_fault_blob_start:
    movl $0x100000, %ebx
    movq (%rbx), %rax
    ud2
omni8_user_fault_blob_end:

    .bss
    .align 8
    .globl omni8_probe_sp
    .globl omni8_probe_resume
omni8_probe_sp:     .skip 8
omni8_probe_resume: .skip 8

    .section .note.GNU-stack, "", @progbits
