/* EUK AArch64 QEMU virt bootstrap. SPDX-License-Identifier: Apache-2.0 */
.section .text.boot, "ax"
.align 7
.global _start
.type _start, %function
_start:
    msr daifset, #0xf
    mov x20, x0                  // QEMU -kernel passes the DTB address in x0.
    mrs x9, CurrentEL
    cmp x9, #0x8                 // EL2
    b.eq .Lfrom_el2
    cmp x9, #0x4                 // EL1
    b.eq .Lenter_el1
    b .Lbad_el

.Lfrom_el2:
    mov x9, #(1 << 31)           // HCR_EL2.RW: EL1 is AArch64.
    msr hcr_el2, x9
    mov x9, #3
    msr cnthctl_el2, x9
    msr cntvoff_el2, xzr
    adr x9, .Lenter_el1
    msr elr_el2, x9
    mov x9, #0x3c5               // EL1h, mask D/A/I/F on entry.
    msr spsr_el2, x9
    eret

.Lenter_el1:
    // Clear the complete NOLOAD BSS range before using the stack or Rust statics.
    // This loop deliberately uses registers only; SP is not established yet.
    ldr x9, =__bss_start
    ldr x10, =__bss_end
.Lzero_bss:
    cmp x9, x10
    b.hs .Lbss_done
    str xzr, [x9], #8
    b .Lzero_bss
.Lbss_done:
    ldr x9, =__boot_stack_top
    mov sp, x9
    ldr x9, =euk_vectors
    msr vbar_el1, x9
    isb
    mov x0, x20
    bl aarch64_main
.Lhalt:
    msr daifset, #0xf
    wfe
    b .Lhalt

.Lbad_el:
    msr daifset, #0xf
    wfe
    b .Lbad_el
.size _start, . - _start

.section .bss.stack, "aw", %nobits
.align 12
.global __boot_stack_bottom
__boot_stack_bottom:
.skip 65536
.global __boot_stack_top
__boot_stack_top:
.section .note.GNU-stack, "", %progbits
