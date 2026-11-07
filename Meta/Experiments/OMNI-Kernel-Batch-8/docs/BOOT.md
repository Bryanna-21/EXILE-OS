# Boot, toolchain and image

## Protocol

Multiboot v1 (flags 3: page-aligned modules, memory info). The loader enters in 32-bit protected mode at `_start` with
`eax = 0x2BADB002` and `ebx` pointing at the info structure. This is the same contract as ExileKernel, so the same GRUB
ISO recipe and QEMU command line apply.

`arch/x86_64/boot.S` then:

1. clears interrupts, sets a 64 KiB boot stack, saves `eax`/`ebx`;
2. initialises COM1 (115200 8N1, polling, no interrupts);
3. rejects a wrong magic with `OMNI8 FAIL: boot magic`;
4. checks CPUID, the FXSR/SSE/SSE2 bits, long mode and NX, rejecting with `OMNI8 FAIL: CPU features`;
5. zeroes and fills a bootstrap PML4/PDPT/PD (1 GiB identity map, 2 MiB pages, supervisor only);
6. enables PAE, OSFXSR and OSXMMEXCPT (CR4 = 0x620), EFER.LME and EFER.NXE (0x900), CR0.PG | WP | MP;
7. loads a private bootstrap GDT and far-jumps into 64-bit code, calling `omni8_main(magic, info)`.

The bootstrap map is executable and writable (2 MiB pages). It is replaced by the kernel's W^X tables (`MEMORY.md`)
before any self-test runs.

## Image layout (`link/linker.ld`)

Load address 1 MiB. Page-aligned `.text` (the Multiboot header first), `.rodata`, `.data` + `.got`, `.bss`. The script
exports `__text_start/_end`, `__rodata_start/_end`, `__data_start/_end`, `__image_end` and asserts the image ends below
2 MiB and text starts at 1 MiB. The ELF is ELF64 with a 32-bit entry point; GRUB's Multiboot loader accepts that.
`kernel32.elf` (objcopy to elf32-i386) is also produced for `qemu -kernel`; that path is **not tested**.

## Toolchain: the documented deviation

The pinned toolchain (rustc 1.91.1, target `x86_64-unknown-none`) could not be installed offline. `scripts/build.sh`
instead builds the `no_std` crates with the installed rustc for the **host target** using
`-C panic=abort -C relocation-model=static -C no-redzone=yes -C codegen-units=1`, produces a static library, assembles the
three `.S` files with gcc, and links with GNU ld and the linker script. `scripts/check_image.py` and
`scripts/check_asm.py` then confirm the result is freestanding: no undefined symbols, no dynamic or TLS segments, no
AVX/x87 instructions, no red-zone addressing, no RWX segment.

What this does and does not prove: it shows the code compiles and links into a freestanding image with those properties.
It does not show the code runs. The first QEMU run under the pinned toolchain is the real test; pass
`--target x86_64-unknown-none` to `build.sh` once that target is installed.

## Build and run

```
scripts/build.sh --variant base --test-exit --out DIR       # DIR/kernel.elf, kernel32.elf, build-info.txt
python3 scripts/check_image.py DIR/kernel.elf --elf32 DIR/kernel32.elf
python3 scripts/check_asm.py DIR/kernel.elf
python3 scripts/qemu_test.py                                 # builds each variant, makes a GRUB ISO, boots, compares
```

Manual boot (needs QEMU, GRUB and xorriso; **not run in the authoring environment**):

```
mkdir -p iso/boot/grub && cp DIR/kernel.elf iso/boot/omni8.elf && cp boot/grub.cfg iso/boot/grub/
grub-mkrescue -o omni8.iso iso
qemu-system-x86_64 -machine pc -accel tcg -cpu qemu64 -m 128M -boot d -display none -serial stdio -monitor none \
    -no-reboot -device isa-debug-exit,iobase=0xf4,iosize=0x04 -cdrom omni8.iso
```
Exit status 33 means the kernel wrote 0x10 to the debug-exit port (success), 35 means 0x11 (failure).

## Variants

| variant | what it builds | expected result |
|---|---|---|
| `base` | boot, GDT/TSS/IDT, paging, allocator, traps, scheduler scenario, timer | exit 33, transcript `tests/expected/base.txt` |
| `ring3` | base plus the ring-3 / SYSCALL stage | exit 33, `ring3.txt` |
| `ring3-fault` | ring 3 reads kernel memory | exit 35, fatal #PF naming ring 3 |
| `trap-fault` | unexpected #UD after the IDT is loaded | exit 35, fatal report |
| `magic-fault` | corrupts the boot magic | exit 35, `OMNI8 FAIL: boot magic` |
| `cpu-fault` | forces the CPU feature check to fail | exit 35, `OMNI8 FAIL: CPU features` |

`--test-exit` makes the kernel and bootstrap write to the debug-exit port; without it they halt.
