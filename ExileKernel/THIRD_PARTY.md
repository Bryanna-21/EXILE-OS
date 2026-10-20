# Dependencies and licensing review

No external crates, vendored kernel code, OS headers or firmware are distributed in this source-only package. Generated kernel binaries and GRUB/QEMU images are deliberately excluded.

| Dependency/reference | Version/source | Purpose | License/obligations |
|---|---|---|---|
| Rust compiler and core/compiler_builtins target libraries | 1.91.1, static.rust-lang.org; rust-lang/rust | bare-metal build/runtime | Rust generally MIT OR Apache-2.0 with third-party notices; verify component COPYRIGHT/licenses before binary distribution, including compiler_builtins/LLVM exceptions |
| Rustfmt | 1.91.1, rust-lang/rustfmt via Nix | formatting only | MIT OR Apache-2.0; not bundled |
| GNU GCC | 14.3.0, gcc.gnu.org via environment | assemble original entry.S | GPL-3.0-or-later, applicable runtime exceptions; no GCC runtime intentionally linked, not bundled |
| GNU binutils | 2.44, gnu.org/software/binutils | ELF link and inspection | GPL-3.0-or-later (tool components); not bundled |
| GRUB | 2.12, gnu.org/software/grub via Nix | BIOS Multiboot test loader | GPL-3.0-or-later; distributing boot images would require applicable source/license compliance; images not distributed |
| QEMU | 10.1.2, qemu.org via Nix | TCG emulator tests | GPL-2.0 family with component-specific licenses; not bundled |
| xorriso/libisoburn | 1.5.6, gnu.org/software/xorriso via Nix | generated ISO tests | GPL-3.0-or-later and component licenses; not bundled |
| Python | environment 3.13.12, python.org | build/test tooling | PSF license; not bundled |
| GitHub Actions checkout | v4, actions/checkout | planned CI checkout | MIT; remote CI tool only |
| GitHub Actions upload-artifact | v4, actions/upload-artifact | planned CI logs | MIT; remote CI tool only |
| Ubuntu CI image | 24.04, GitHub hosted runner | planned test environment | multiple distro licenses; not bundled |
| GNU Multiboot v1 specification | gnu.org/software/grub/manual/multiboot | protocol facts | documentation GFDL terms; no specification text/implementation copied |
| Intel/AMD architecture manuals | vendor public manuals | CPU transition facts | vendor documentation terms; no manual contents distributed |

This is a dependency inventory, not legal clearance. Before binary distribution, audit the exact linked Rust library dependency notices and generated bootloader components. No Secure Boot or UEFI interface is implemented or bundled.
