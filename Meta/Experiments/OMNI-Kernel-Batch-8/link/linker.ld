/* OMNI Batch 8 linker script: Multiboot v1, loaded at 1 MiB, x86-64 kernel in an ELF64
 * container with a 32-bit entry point. Every section starts on a page boundary so the kernel
 * can map text / rodata / data+bss with different permissions. */
ENTRY(_start)

SECTIONS
{
    . = 1M;
    __image_start = .;

    .text : ALIGN(4K)
    {
        __text_start = .;
        KEEP(*(.multiboot))
        *(.text.boot)
        *(.text .text.*)
        . = ALIGN(4K);
        __text_end = .;
    }

    .rodata : ALIGN(4K)
    {
        __rodata_start = .;
        *(.rodata .rodata.*)
        . = ALIGN(4K);
        __rodata_end = .;
    }

    .data : ALIGN(4K)
    {
        __data_start = .;
        *(.data .data.*)
        *(.got .got.plt)
    }

    .bss : ALIGN(4K)
    {
        *(.bss .bss.*)
        *(COMMON)
        . = ALIGN(4K);
        __data_end = .;
    }

    __image_end = .;

    /DISCARD/ :
    {
        *(.comment)
        *(.eh_frame*)
        *(.note*)
        *(.gnu.*)
    }
}

ASSERT(__image_end <= 2M, "kernel image must end below 2 MiB (the first 2 MiB are mapped with 4 KiB pages)")
ASSERT(__text_start == 1M, "text must start at 1 MiB")
