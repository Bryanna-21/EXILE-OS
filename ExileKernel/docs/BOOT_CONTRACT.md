# EUK Boot Contract (Mission A)

Status: implemented for the validation layers described below; **compiled and
boot-tested only in the owner's environment** â€” the delivery environment had
no toolchain, so nothing here carries emulator evidence yet. Run the commands
at the end and record results.

## 1. Supported protocols

| Protocol | Role | Status |
|---|---|---|
| Multiboot v1 | development/test path (preserved; all existing fault gates intact) | implemented (original milestone + MISSION-A layers) |
| UEFI x86-64 | production target | Batch A5 (design below; NOT implemented in this bundle) |
| QEMU -kernel (AArch64 virt) | development/test path for AArch64 | implemented (platform contract stage) |
| UEFI/DTB (AArch64 production) | production target | Batch A5 (NOT implemented; DTB not yet parsed) |

## 2. BootInfo v1 ABI

- Magic `"EUKB"` (0x424B5545), version 1 exactly; unsupported versions are
  rejected with `UnsupportedVersion(v)` â€” no silent interpretation.
- Fixed 88-byte C-layout header, 8-byte aligned; 24-byte entries; sizes pinned
  by compile-time assertions (layout drift = build failure).
- Physical addresses only; no raw pointers. Ownership: the loader owns all
  handoff objects until ExitBootServices-equivalent; the kernel owns them
  after validation. Lifetime: boot-lifetime, immutable once validated.
- Flags must match their objects exactly (framebuffer present â‡” size>0 and
  paddr page-aligned; RSDP present â‡” paddrâ‰ 0).
- Kernel range: page-aligned, start<end, checked arithmetic.
- Validation is fail-closed: any error aborts boot with a diagnostic.

## 3. Memory-map semantics

Two layers: (a) the STRICT loader ABI (`bootinfo::validate_regions` â€” the
loader promises well-formed entries), and (b) the TOLERANT firmware-map
normalizer (`memmap_norm` â€” defends against unsorted/overlap/duplicates/
zero-length/overflow). Precedence (strongest wins): Unusable > AcpiReclaim >
Reserved > Hibernated > Bad > Usable. Unknown firmware kinds classify as
Reserved, never Usable. Every output interval records which input entry
decided it (provenance) and adjacent same-kind/same-source intervals merge.

## 4. Reservations

Applied before the early allocator sees memory: kernel image (<16 MiB on
x86-64; 0x40080000..0x400C0000 on the AArch64 platform contract), loader map,
BootInfo objects, page tables, boot stack, TSS/IST stacks, framebuffer, ACPI,
MMIO. The early allocator is alloc-only (boot-lifetime ownership); handoff to
the full frame allocator receives validated free ranges through `phys_handoff::handoff`; the allocator exposes `free_ranges()` while active.

## 5. Virtual layouts
- x86-64 (this milestone): 1 GiB supervisor identity map, RWX 2 MiB pages.
  W^X/4 KiB remap is deliberately deferred to Batch A5 â€” a boot-protocol
  change that must carry its own emulator evidence (mission rule 13).
- AArch64: MMU off in this stage (identity = physical); full VA layout with
  W^X arrives with the translation-table batch (A5).
- Guard pages: x86-64 double-fault uses a dedicated IST1 stack; the boot stack
  has a not-allocated hole below it (unmapped â‡’ fault). AArch64 boot stack has
  a 4 KiB unmapped guard below it.

## 6. Panic/exception policy

All CPU faults during boot are FATAL: report (serial/PL011) and halt with
  interrupts masked. No recovery is claimed â€” the mission forbids fake
  recovery. #DF lands on the IST1 stack on x86-64. The Rust panic handler is
  bounded (no allocation, no recursion) and reuses the fail() path.

## 7. Security
- Secure Boot: NOT supported in this milestone. No signature verification or
  trust chain exists. The UEFI loader (A5) must state its policy explicitly.
- No interrupts are enabled in Mission A; only synchronous fault gates exist.

## 8. Not implemented (explicit, not silently deferred)

UEFI loader + ExitBootServices + memory-map-key retry (A5); ACPI signature/
checksum parsing (A5); DTB parsing (A5); CPU topology (A5); 4-KiB W^X page
tables on both architectures (A5); SMP (later mission; explicitly not claimed).

## 9. Verification commands (record exact results)

    # host unit tests (expect: 18 + 14 + 14 + 10 = 56)
    python3 ExileKernel/scripts/test.py
    # x86-64 build + reproducibility + all 8 existing boot cases
    export EUK_RUSTC=... ; export EUK_SYSROOT=...
    python3 ExileKernel/scripts/build.py --output /tmp/eukMA/a
    python3 ExileKernel/scripts/build.py --output /tmp/eukMA/b
    cmp /tmp/eukMA/a/euk.elf /tmp/eukMA/b/euk.elf
    python3 ExileKernel/scripts/boot_test.py
    # expected new serial lines in the normal case (before BOOT_OK):
    #   EUK: fault gates installed
    #   EUK: memory map normalized
    #   EUK: early allocator live, first 4K page at 0x...
    #   EUK: bootinfo v1 contract validated
    # AArch64 cross-build + QEMU boot:
    python3 ExileKernel/scripts/build_aarch64.py

## 10. Five-file batch ledger
A1: bootinfo.rs, memmap_norm.rs, test.py, BOOT_CONTRACT.md (4)
A2: early_alloc.rs (1)
A3: exceptions.rs, kernel.rs, entry.S, linker.ld (4)
A4: boot.S, vectors.S, linker.ld, main.rs, build_aarch64.py (5)
A5: none yet (design only).


## 11. Mission A2: early physical memory allocator

Ledger correction: A2 is five files, not one.
`early_alloc.rs` (new at this base), `memmap_norm.rs` (extended), `reserve.rs` (new),
`range.rs` (new), `phys_handoff.rs` (new). Boot wiring touches `kernel.rs`,
`arch/x86_64/linker.ld` (`__kernel_start`, `__kernel_end`), `scripts/build.py`
(`a2_nomem` fault), `scripts/boot_test.py`, `scripts/test.py`.

Boot order (x86-64): Multiboot magic and map bounds, strict `parse_map`, usable-memory
check, then A2: normalize the map, reserve, `EarlyAllocator::init`, one-page self-check
plus invariant audit. Any failure prints `EUK FAIL: <reason>` and exits non-zero.
No heap exists at any point.

Reserved before the allocator sees memory: physical [0, 16 MiB) (legacy low memory and
the bootstrap prefix the linker script asserts the image fits in), the kernel image,
the 116-byte Multiboot info structure, and the memory-map storage. Physical page 0
and everything at or above the 1 GiB identity-map limit are never managed.

Rules: only `Usable` normalized intervals are eligible; gaps between entries and every
unknown kind are not usable; ACPI NVS maps to Unusable and kind 5 to Bad; partly-usable
or partly-reserved pages are dropped; sizes round up to pages; `init` can never run twice;
a failed `alloc` or `reserve_late` changes nothing.

Ownership and lifecycle: Uninit -> Active -> Sealed. `phys_handoff::handoff` audits,
re-validates every free range, offers the whole list to a `FrameSink` in ONE call, and
seals only if it is accepted. Reserved and already-allocated pages are never offered.
Mission B was not present in this repository: `FrameSink` is the smallest receiving
interface, not a verified Mission B interface. An adapter is needed only if Mission B's
real shape differs.

Note: section 9 above predates A2 (test count, aarch64 script); the commands that
matter for A2 are `python3 scripts/test.py` and `python3 scripts/boot_test.py`.
