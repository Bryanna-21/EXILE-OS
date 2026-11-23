# Provenance record

The EUK milestone source, build/test tooling, documentation, and workflow
were prepared with AI assistance for this project on 2026-10-19. The source
reconstructs work after earlier temporary files became unavailable and is
not claimed to be byte-identical to that earlier work. The project owner has
authorized public release of the original EUK contribution under Apache-2.0.

The recorded implementation files are src/memory.rs, src/kernel.rs,
src/arch/x86_64/entry.S, src/arch/x86_64/linker.ld, and the Python tooling.
The project record states that no implementation was copied from Serenity,
Linux, or another kernel. This statement is a provenance record, not an
independent legal certification or guarantee against intellectual-property
claims.

Technical references include the GNU Multiboot v1 protocol, Intel/AMD
architecture documentation, and Rust no_std and target documentation.
Review of existing Exile OS boot/build paths was for integration boundaries;
existing Exile OS and Serenity-derived code is not relicensed by this package.

See NOTICE.md and THIRD_PARTY.md. Toolchain components and any future
distributed binaries or boot images require their own applicable license
and attribution checks.
