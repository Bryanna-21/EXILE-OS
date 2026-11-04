/* OMNI Batch 8 Multiboot v1 bootstrap (x86-64). Original code; same boot contract as
 * ExileKernel (Multiboot v1, COM1 115200 8N1, isa-debug-exit at 0xf4) but self-contained.
 *
 * Build-time switches (gcc -D): TEST_EXIT (exit through isa-debug-exit), FAULT_MAGIC and
 * FAULT_CPU (inject the failure so the rejection path can be boot-tested).
 *
 * This file only gets the CPU into 64-bit long mode on a private bootstrap GDT and a
 * supervisor-only identity map of the first GiB (2 MiB pages, executable: replaced by the
 * kernel's W^X tables before any other code runs). Everything else is in Rust.
 */
    .section .multiboot, "a"
    .align 4
    .long 0x1badb002, 3, -(0x1badb002 + 3)

    .section .text.boot, "ax"
    .code32
    .global _start
_start:
    cli
    cld
    movl $boot_stack_top, %esp
    movl %eax, omni8_boot_magic
    movl %ebx, omni8_boot_info
    /* COM1: 115200 8N1, interrupts off. */
    movw $0x3f9, %dx
    xorl %eax, %eax
    outb %al, %dx
    movw $0x3fb, %dx
    movb $0x80, %al
    outb %al, %dx
    movw $0x3f8, %dx
    movb $1, %al
    outb %al, %dx
    movw $0x3f9, %dx
    xorb %al, %al
    outb %al, %dx
    movw $0x3fb, %dx
    movb $3, %al
    outb %al, %dx
#ifdef FAULT_MAGIC
    movl $0, omni8_boot_magic
#endif
    cmpl $0x2badb002, omni8_boot_magic
    jne bad_magic
    /* CPUID present? (ID flag writable) */
    pushfl
    popl %eax
    movl %eax, %ecx
    xorl $0x200000, %eax
    pushl %eax
    popfl
    pushfl
    popl %eax
    pushl %ecx
    popfl
    xorl %ecx, %eax
    testl $0x200000, %eax
    jz bad_cpu
    movl $1, %eax
    cpuid
    /* PSE, MSR, PAE, FXSR, SSE, SSE2 (the interrupt path uses FXSAVE). */
    andl $0x07000068, %edx
    cmpl $0x07000068, %edx
    jne bad_cpu
    movl $0x80000000, %eax
    cpuid
    cmpl $0x80000001, %eax
    jb bad_cpu
    movl $0x80000001, %eax
    cpuid
#ifdef FAULT_CPU
    xorl %edx, %edx
#endif
    /* Long mode and NX. */
    andl $0x20100000, %edx
    cmpl $0x20100000, %edx
    jne bad_cpu
    /* Zero the three bootstrap tables explicitly; never trust stale BSS. */
    movl $boot_pml4, %edi
    movl $3072, %ecx
    xorl %eax, %eax
    rep stosl
    movl $boot_pdpt, %eax
    orl $3, %eax
    movl %eax, boot_pml4
    movl $boot_pd, %eax
    orl $3, %eax
    movl %eax, boot_pdpt
    xorl %ecx, %ecx
1:
    movl %ecx, %eax
    shll $21, %eax
    orl $0x83, %eax
    movl %eax, boot_pd(,%ecx,8)
    incl %ecx
    cmpl $512, %ecx
    jne 1b
    lgdt boot_gdt_ptr
    /* CR4: PAE | OSFXSR | OSXMMEXCPT */
    movl %cr4, %eax
    orl $0x620, %eax
    movl %eax, %cr4
    movl $boot_pml4, %eax
    movl %eax, %cr3
    /* EFER: LME | NXE */
    movl $0xc0000080, %ecx
    rdmsr
    orl $0x900, %eax
    wrmsr
    /* CR0: PG | WP | MP, EM clear */
    movl %cr0, %eax
    andl $0xfffffffb, %eax
    orl $0x80010002, %eax
    movl %eax, %cr0
    ljmp $8, $long_entry
bad_magic:
    movl $msg_magic, %esi
    jmp fail32
bad_cpu:
    movl $msg_cpu, %esi
fail32:
    lodsb
    testb %al, %al
    jz exit32
    movb %al, %bl
    movl $100000, %ecx
2:
    movw $0x3fd, %dx
    inb %dx, %al
    testb $0x20, %al
    jnz 5f
    loop 2b
    jmp fail32
5:
    movw $0x3f8, %dx
    movb %bl, %al
    outb %al, %dx
    jmp fail32
exit32:
#ifdef TEST_EXIT
    movw $0xf4, %dx
    movl $0x11, %eax
    outl %eax, %dx
#endif
3:  hlt
    jmp 3b

    .code64
long_entry:
    movw $16, %ax
    movw %ax, %ds
    movw %ax, %es
    movw %ax, %ss
    xorl %eax, %eax
    movw %ax, %fs
    movw %ax, %gs
    leaq boot_stack_top(%rip), %rsp
    xorq %rbp, %rbp
    movl omni8_boot_magic(%rip), %edi
    movl omni8_boot_info(%rip), %esi
    call omni8_main
4:  cli
    hlt
    jmp 4b

    .section .rodata, "a"
    .align 8
boot_gdt:
    .quad 0, 0x00af9a000000ffff, 0x00cf92000000ffff
boot_gdt_ptr:
    .word 23
    .long boot_gdt
msg_magic: .asciz "OMNI8 FAIL: boot magic\n"
msg_cpu:   .asciz "OMNI8 FAIL: CPU features\n"

    .section .bss, "aw", @nobits
    .align 4
    .global omni8_boot_magic
    .global omni8_boot_info
omni8_boot_magic: .skip 4
omni8_boot_info:  .skip 4
    .align 4096
boot_pml4: .skip 4096
boot_pdpt: .skip 4096
boot_pd:   .skip 4096
    .align 16
boot_stack: .skip 65536
boot_stack_top:
    .global omni8_boot_stack_top
    .set omni8_boot_stack_top, boot_stack_top

    .section .note.GNU-stack, "", @progbits
