ENTRY(_start)
PHDRS
{
    text PT_LOAD FLAGS(5);
    rodata PT_LOAD FLAGS(4);
    data PT_LOAD FLAGS(6);
}
SECTIONS
{
    . = 0x40080000;
    __kernel_start = .;
    .text : ALIGN(16) {
        KEEP(*(.text.boot))
        KEEP(*(.text.vectors))
        *(.text .text.*)
    } :text
    . = ALIGN(4096);
    .rodata : { *(.rodata .rodata.*) } :rodata
    . = ALIGN(4096);
    .data : { *(.data .data.*) } :data
    .bss (NOLOAD) : ALIGN(4096) {
        __bss_start = .;
        *(.bss.stack)
        *(.bss .bss.*)
        *(COMMON)
        . = ALIGN(8);
        __bss_end = .;
    } :data
    . = ALIGN(4096);
    __kernel_end = .;
    ASSERT((euk_vectors & 2047) == 0, "AArch64 vector table must be 2 KiB aligned")
    ASSERT(__kernel_end > __kernel_start, "empty AArch64 image")
    /DISCARD/ : { *(.eh_frame*) *(.comment) *(.note*) }
}
