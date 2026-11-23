/* EUK Multiboot v1 bootstrap. SPDX-License-Identifier: Apache-2.0 */
.section .multiboot,"a"
.align 4
.long 0x1badb002, 3, -(0x1badb002 + 3)
.section .text.boot,"ax"
.code32
.global _start
_start:
    cli
    cld
    mov $stack_top, %esp
    mov %eax, saved_magic
    mov %ebx, saved_info
    /* COM1: 115200 8N1, no interrupts. */
    mov $0x3f9, %dx
    xor %eax, %eax
    out %al, %dx
    mov $0x3fb, %dx
    mov $0x80, %al
    out %al, %dx
    mov $0x3f8, %dx
    mov $1, %al
    out %al, %dx
    mov $0x3f9, %dx
    xor %al, %al
    out %al, %dx
    mov $0x3fb, %dx
    mov $3, %al
    out %al, %dx
#ifdef FAULT_MAGIC
    movl $0, saved_magic
#endif
    cmpl $0x2badb002, saved_magic
    jne bad_magic
    /* Verify CPUID is available before executing it. */
    pushfl
    pop %eax
    mov %eax, %ecx
    xor $0x200000, %eax
    push %eax
    popfl
    pushfl
    pop %eax
    push %ecx
    popfl
    xor %ecx, %eax
    test $0x200000, %eax
    jz bad_cpu
    mov $1, %eax
    cpuid
    /* PSE, MSR, PAE, SSE, SSE2. */
    and $0x06000068, %edx
    cmp $0x06000068, %edx
    jne bad_cpu
    mov $0x80000000, %eax
    cpuid
    cmp $0x80000001, %eax
    jb bad_cpu
    mov $0x80000001, %eax
    cpuid
#ifdef FAULT_CPU
    xor %edx, %edx
#endif
    and $0x20100000, %edx
    cmp $0x20100000, %edx
    jne bad_cpu
    /* Initialize every table entry explicitly; never depend on stale BSS. */
#ifdef FAULT_TABLE_DIRTY
    mov $pml4, %edi
    mov $3072, %ecx
    mov $0xffffffff, %eax
    rep stosl
#endif
    mov $pml4, %edi
    mov $3072, %ecx
    xor %eax, %eax
    rep stosl
    /* Build a supervisor-only 1 GiB map. */
    mov $pdpt, %eax
    or $3, %eax
    mov %eax, pml4
    mov $pd, %eax
    or $3, %eax
    mov %eax, pdpt
    xor %ecx, %ecx
1:
    mov %ecx, %eax
    shl $21, %eax
    or $0x83, %eax
    mov %eax, pd(,%ecx,8)
    inc %ecx
    cmp $512, %ecx
    jne 1b
    lgdt gdt_ptr
    mov %cr4, %eax
    or $0x620, %eax
    mov %eax, %cr4
    mov $pml4, %eax
    mov %eax, %cr3
    mov $0xc0000080, %ecx
    rdmsr
    or $0x900, %eax
    wrmsr
    mov %cr0, %eax
    and $0xfffffffb, %eax
    or $0x80010002, %eax
    mov %eax, %cr0
    ljmp $8, $long_entry
bad_magic:
    mov $msg_magic, %esi
    jmp fail32
bad_cpu:
    mov $msg_cpu, %esi
fail32:
    lodsb
    test %al, %al
    jz exit32
    mov %al, %bl
    mov $100000, %ecx
2:
    mov $0x3fd, %dx
    in %dx, %al
    test $0x20, %al
    jnz 5f
    loop 2b
    jmp fail32
5:
    mov $0x3f8, %dx
    mov %bl, %al
    out %al, %dx
    jmp fail32
exit32:
#ifdef TEST_EXIT
    mov $0xf4, %dx
    mov $0x11, %eax
    out %eax, %dx
#endif
3:  hlt
    jmp 3b
.code64
long_entry:
    mov $16, %ax
    mov %ax, %ds
    mov %ax, %es
    mov %ax, %ss
    xor %eax, %eax
    mov %ax, %fs
    mov %ax, %gs
    lea stack_top(%rip), %rsp
    xor %rbp, %rbp
    mov saved_magic(%rip), %edi
    mov saved_info(%rip), %esi
    call kernel_main
4:  cli
    hlt
    jmp 4b
.section .rodata,"a"
.align 8
gdt:
    .quad 0, 0x00af9a000000ffff, 0x00cf92000000ffff
gdt_ptr:
    .word 23
    .long gdt
msg_magic: .asciz "EUK FAIL: boot magic\n"
msg_cpu: .asciz "EUK FAIL: CPU features\n"
.section .bss,"aw",@nobits
.align 4
saved_magic: .skip 4
saved_info: .skip 4
.align 4096
pml4: .skip 4096
pdpt: .skip 4096
pd: .skip 4096
.align 16
stack: .skip 65536
stack_top:
.section .note.GNU-stack,"",@progbits
