# LICENSING_AND_PROVENANCE_AUDIT.md (PROPOSED DRAFT; not legal advice)

> **Status update (branch `fix/boot-integrity-and-ci`, based on `72c3049`; changes uncommitted pending review).**
> This audit was written against `18a3c70`. Verified since then, by running code on a development host: the boot guardian now implements Ed25519-authenticated verification (12 unit tests, 24 tamper checks pass); the Store host build passes 2/2 tests; the repo style checks pass. `.tmp/` is no longer tracked. **Not verified:** any full-OS build, QEMU boot, CI result, or the prekernel target compile. Verified boot is opt-in and not proven end to end; see `Documentation/BootIntegrity.md`. Items below not mentioned here remain proposals.

Baseline `main` @ `18a3c70`. This lists observed facts and open questions. Flag everything marked **LEGAL** for a qualified lawyer.

## Observed facts
- Root `LICENSE`: BSD-2-Clause, "Copyright (c) 2018-2026, the SerenityOS developers." Intact.
- File headers (outside `Ports/`): 7,536 `SPDX-License-Identifier: BSD-2-Clause`, 1 `MIT`, 1 `Apache-2.0 WITH LLVM-exception`.
- Named copyright holders appear throughout (matches by `Copyright (c)` grep, largest: Andreas Kling, "the SerenityOS developers", Sam Atkins, Linus Groh, Liav A., Tim Flynn, Sönke Holz, others). Only two files matched `Copyright.*Exile`: `Base/usr/share/man/man1/Applications/About.md` and `Documentation/QtCreatorConfiguration.md` (content not examined).
- `Meta/Exile/IDENTITY.md` states the project originated from SerenityOS and that upstream licenses and attribution are preserved.
- Only license-type files outside `Ports/`: `LICENSE`, `Tests/LibWeb/Text/input/wpt-import/LICENSE.md`, `Userland/Applications/ExileStore/NOTICE`.

## Matrix

| Component | Origin | License evidence | Obligation | Status |
|---|---|---|---|---|
| Kernel, AK, Userland, Base, Meta, Tests, Documentation | SerenityOS-derived | BSD-2-Clause headers, root LICENSE | Keep copyright notice and disclaimer in source and in binary-form documentation | Known |
| `Kernel/Boot/Exile/*`, `Meta/Tools/ExileBootManifest.cpp` | Exile-authored (from commit history, not verified: shallow clone) | No license header or copyright line in the files read | Choose and apply a license | Unclear |
| `Userland/Applications/ExileStore` | Exile-authored per its NOTICE | NOTICE: "No license has been chosen". `src/native_main.cpp` header says BSD-2-Clause | Resolve the contradiction | **LEGAL/owner decision** |
| Store dependency OpenSSL (libcrypto) | Third-party, linked from system | Not bundled | Check OpenSSL license obligations for distribution | Open |
| `Ports/` (367 recipes) | Third-party sources fetched at build time | No license metadata in `package.sh` (grep found none) | Per-port license collection if images ship ports | Unreviewed |
| `Toolchain/` | GCC, Clang/LLVM, binutils, QEMU, EDK2, CMake, Python, Ruby, GDB, e2fsprogs, Jakt built from upstream | Scripts only; sources downloaded | Toolchain output licensing, GPL runtime exception review for shipped libs | Unreviewed |
| Build-time downloads | `pci.ids`, `usb.ids`, `pnp.ids`, Raspberry Pi DTB (CMake `ENABLE_*_DOWNLOAD`), Unicode data | Not in tree | Check each dataset's terms | Unreviewed |
| `Tests/LibWeb/.../wpt-import` | Web Platform Tests | Own LICENSE.md | Preserve | Known |
| Fonts, wallpapers, icons, sounds, `Base/home` media, 3D models, `test-webm.webm` | Mixed/unknown | Not examined | Verify each asset's license | **Unclear provenance** |
| `Ladybird/` | Upstream browser frontends | Not examined | Preserve upstream notices | Unreviewed |
| Bootloader support (extlinux, GRUB, Limine scripts; EDK2) | Third-party tools referenced by scripts | Not bundled (scripts only) | Redistribution terms if images embed them | Unreviewed |
| Brotli dictionary `.dict.bin` | Third-party data | Not examined | Check Brotli license | Unreviewed |

## Questions needing qualified legal review (LEGAL)
1. README says "Independent graphical operating system" and "built from-scratch by us". With Serenity-derived code and its contributors named in thousands of files, is that wording acceptable?
2. Naming of legal entities in `IDENTITY.md` ("Exile Organization", "Exile Foundation"): do they exist, and who holds copyright in Exile-authored changes?
3. Can Exile add its own copyright lines to inherited files, and what is the right notice format?
4. What process makes EUK components credibly independent, and can they carry a different license from the base OS?
5. Redistribution obligations for images bundling Ports, GPL toolchain runtime libraries, fonts and media.
6. Trademark clearance for "Exile OS", "Exile Store" and the "X" icon.

## Do not
Do not remove copyright notices, replace the root license or describe the whole repository as independently authored.

## EUK components that could be independently authored and separately licensed
New code in a separate tree: kernel core, IPC, scheduler, memory manager, boot verifier, user-space servers, Store backend, update service. Anything derived from `Kernel/`, `AK/`, or LibC stays under the inherited BSD-2-Clause terms and notices.
