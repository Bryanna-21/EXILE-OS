# Third-party components and references

## Scope

This document records third-party tools and technical references identified
in the EUK source, build scripts, verification records, and documentation.
It is an inventory, not a complete software bill of materials or legal
clearance for redistribution.

The original EUK contribution is licensed under Apache-2.0 as described in
[LICENSE](LICENSE) and [NOTICE.md](NOTICE.md). That declaration does not
change the licenses of third-party software or existing Exile OS code.

## Build and verification tools

| Component | Recorded use | Verification / distribution note |
|---|---|---|
| Rust compiler and standard libraries | Compile the freestanding kernel and host tests | The pinned Rust 1.91.1 compiler and target/host standard-library archives have SHA-256 values recorded in `docs/TOOLCHAIN.md`. Consult the official Rust license and component notices for the exact archives. |
| GNU GCC | Assemble the x86-64 entry source | The documented environment used GCC 14.3.0. Check the exact distribution package and applicable GCC runtime exception and license notices. |
| GNU binutils (`ld`) | Link the kernel image | The documented environment used binutils 2.44. Check the exact package license and notices. |
| GRUB tools and modules | Create standalone Multiboot test images | The documented environment used GRUB 2.12. `grub-mkrescue` places GRUB components in generated ISO images; review the exact modules and their license notices before distributing an ISO. |
| xorriso | Create ISO images through the GRUB tooling | The documented environment used xorriso 1.5.6. Check the exact package license and notices. |
| QEMU | Execute emulator boot tests | The documented environment used QEMU 10.1.2. QEMU is a test tool; it is not part of the EUK kernel source. Check its applicable license and notices if redistributing it. |
| Python 3 | Run build, test, audit, and boot-test scripts | The scripts use the Python standard library. Check the license and notices for the exact Python distribution. |

Versions above describe the recorded verification environment; they are
not a guarantee that every copy of the package was built with those versions.
See `docs/TOOLCHAIN.md`, `docs/VERIFICATION.md`, and `build/BUILD_NOTES.txt`.

## Technical references

The project documentation identifies these external technical references:

- GNU Multiboot version 1 boot protocol.
- Intel architecture manuals and instruction-set documentation.
- AMD architecture programmer's manuals.
- Official Rust documentation for `no_std` and target support.

These are technical references, not evidence that their authors contributed
code to EUK or endorsed this project. Consult the publishers' terms for
reproduction or redistribution of any copied material.

## Generated artifacts and redistribution

The `build/` directory may contain kernel ELF files, static libraries,
test executables, serial logs, and ISO boot images. These are generated
artifacts, not a declaration that all embedded or linked components are
solely original EUK work.

In particular:

- The recorded build notes identify an unpinned Rust 1.97 host build using
  host libraries. It is not the pinned Rust 1.91.1 bare-metal build.
- An ISO created with GRUB tooling may contain GRUB modules and other
  third-party components.
- Linked compiler runtime or standard-library material may carry obligations
  distinct from the EUK source license.
- The checksum manifest establishes file integrity against the listed
  digests; it does not establish copyright ownership or license compliance.

Before distributing any generated binary or boot image, identify its actual
contents and linked components, preserve applicable copyright and license
notices, and satisfy the obligations for the exact versions used.

## Existing Exile OS boundary

Existing Exile OS and Serenity-derived code is outside the licensing scope
of this EUK package. See `docs/INTEGRATION.md` and `docs/PROVENANCE.md`.
