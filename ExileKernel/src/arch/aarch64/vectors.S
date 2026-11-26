/* AArch64 EL1 vector table. Each slot is exactly 128 bytes. */
.section .text.vectors, "ax"
.balign 2048
.global euk_vectors
.type euk_vectors, %function
euk_vectors:
    b euk_sync; .balign 128
    b euk_irq;  .balign 128
    b euk_fiq;  .balign 128
    b euk_serr; .balign 128
    b euk_sync; .balign 128
    b euk_irq;  .balign 128
    b euk_fiq;  .balign 128
    b euk_serr; .balign 128
    b euk_sync; .balign 128
    b euk_irq;  .balign 128
    b euk_fiq;  .balign 128
    b euk_serr; .balign 128
    b euk_sync; .balign 128
    b euk_irq;  .balign 128
    b euk_fiq;  .balign 128
    b euk_serr; .balign 128

euk_sync:
    mov x0, #0
    b aarch64_exception
euk_irq:
    mov x0, #1
    b aarch64_exception
euk_fiq:
    mov x0, #2
    b aarch64_exception
euk_serr:
    mov x0, #3
    b aarch64_exception
.size euk_vectors, . - euk_vectors
.section .note.GNU-stack, "", %progbits
