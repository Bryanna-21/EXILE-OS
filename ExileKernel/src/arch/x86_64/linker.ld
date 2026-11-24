ENTRY(_start)
PHDRS { text PT_LOAD FLAGS(5); rodata PT_LOAD FLAGS(4); data PT_LOAD FLAGS(6); }
SECTIONS {
    . = 1M;
    __kernel_start = .;
    .text : { KEEP(*(.multiboot)) *(.text.boot) *(.text .text.*) } :text
    . = ALIGN(4096);
    .rodata : { *(.rodata .rodata.*) } :rodata
    . = ALIGN(4096);
    .data : { *(.data .data.*) *(.got .got.*) } :data
    .bss : { *(.bss .bss.*) *(COMMON) } :data
    __kernel_end = ALIGN(4096);
    ASSERT(. < 16M, "EUK bootstrap exceeds reserved prefix")
    /DISCARD/ : { *(.eh_frame*) *(.comment) *(.note*) }
}
