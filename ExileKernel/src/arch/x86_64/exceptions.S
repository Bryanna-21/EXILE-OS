/* EUK x86-64 exception and interrupt entry. SPDX-License-Identifier: Apache-2.0
 *
 * Every vector 0..255 has a 16-byte stub. A stub makes the stack frame uniform, then enters the
 * shared path, which saves the general-purpose registers, calls Rust and returns with iretq.
 *
 * Stack at stub entry (hardware-built; low address first):
 *     [error code]   only for vectors 8,10,11,12,13,14,17,21,29,30 (matches idt.rs EXCEPTIONS)
 *     rip, cs, rflags, rsp, ss
 * The CPU aligns RSP to 16 bytes before pushing this frame in 64-bit mode.
 *
 * After the stub and the common path the stack holds a `TrapFrame` (see frame.rs, 176 bytes):
 *     r15..rax (15 pushes), vector, error code (real or a dummy 0), rip, cs, rflags, rsp, ss
 * 176 is a multiple of 16, so RSP is 16-byte aligned at the `call`, as the SysV ABI requires.
 *
 * Software `int N` for a vector that expects an error code does NOT push one and would corrupt
 * this layout; those gates are DPL 0 (see idt.rs default_dpl) so ring 3 cannot do it.
 *
 * swapgs is intentionally absent: there is no user mode yet. It becomes mandatory on entry and
 * exit when a ring 3 frame can arrive (CS.RPL == 3).
 * Rust handlers must not use SSE/x87: only general-purpose registers are saved here.
 */
.code64

.section .text.isr,"ax",@progbits
.p2align 4
.global euk_isr_stubs
euk_isr_stubs:
.set vec, 0
.rept 256
    .if (vec == 8) || (vec == 10) || (vec == 11) || (vec == 12) || (vec == 13) || (vec == 14) || (vec == 17) || (vec == 21) || (vec == 29) || (vec == 30)
    /* The CPU already pushed an error code. */
    .else
    push $0
    .endif
    push $vec
    jmp euk_isr_common
    /* Pad the slot with int3 (0xcc). `.org` refuses to move backwards, so a stub that grew past
     * 16 bytes is an assembler error rather than a table pointing into the wrong stub. */
    .org euk_isr_stubs + (vec + 1) * 16, 0xcc
    .set vec, vec + 1
.endr

.global euk_isr_common
.type euk_isr_common,@function
euk_isr_common:
    push %rax
    push %rbx
    push %rcx
    push %rdx
    push %rsi
    push %rdi
    push %rbp
    push %r8
    push %r9
    push %r10
    push %r11
    push %r12
    push %r13
    push %r14
    push %r15
    mov %rsp, %rdi              /* &mut TrapFrame, 16-byte aligned */
    cld                         /* interrupt delivery does not clear DF; the ABI requires it clear */
    call euk_trap_dispatch
    pop %r15
    pop %r14
    pop %r13
    pop %r12
    pop %r11
    pop %r10
    pop %r9
    pop %r8
    pop %rbp
    pop %rdi
    pop %rsi
    pop %rdx
    pop %rcx
    pop %rbx
    pop %rax
    add $16, %rsp               /* discard vector and error code */
    iretq
.size euk_isr_common, . - euk_isr_common

/* Absolute handler address for each vector, consumed by traps.rs when building the IDT. */
.section .rodata.isr,"a",@progbits
.p2align 3
.global euk_isr_table
.type euk_isr_table,@object
euk_isr_table:
.set vec, 0
.rept 256
    .quad euk_isr_stubs + vec * 16
    .set vec, vec + 1
.endr
.size euk_isr_table, . - euk_isr_table

.section .note.GNU-stack,"",@progbits
