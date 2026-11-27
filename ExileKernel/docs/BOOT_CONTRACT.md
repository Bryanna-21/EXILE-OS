# EUK Boot Contract — Section A(i)

## Status

The versioned BootInfo contract, checked physical ranges, deterministic memory-map normalizer, reservations, early allocator, and physical handoff validation are implemented as independent no_std components.

The current executable x86-64 path remains the existing Multiboot v1 path. It is preserved deliberately: the current Multiboot information block is not a valid EUK BootInfo v1 object and this batch does not invent kernel physical bounds, entry metadata, or loader-owned BootInfo storage that Multiboot does not provide.

The `boot_foundation::ValidatedBoot` façade is the staged loader boundary. It accepts bounded, already-read `BootInfo` and `MemRegion` slices, validates the complete logical handoff, normalizes the map, audits the result, and exposes usable ranges only after successful validation. It performs no raw-pointer access. A future EUK loader must call this boundary before allocator setup.

## BootInfo v1 ABI

- Magic: `EUKB`, numeric value `0x424B5545`.
- Version: exactly `1`; unsupported versions are rejected.
- Fixed `repr(C), align(8)` header size: 88 bytes.
- Fixed `repr(C), align(8)` memory entry size: 24 bytes.
- Physical addresses are represented as integers; no raw pointers occur in the ABI.
- `memmap_count` is non-zero and limited to 256 entries.
- `memmap_paddr` is non-zero, 8-byte aligned, and its byte range is checked.
- `total_size` must equal `SIZE + memmap_count * MEM_REGION_SIZE`.
- Unknown flags are rejected.
- Kernel physical bounds are page-aligned and strictly increasing.
- A zero kernel entry is rejected.
- Optional framebuffer and ACPI RSDP fields must agree with their flags and use checked address arithmetic.
- Any malformed field fails closed.

## Memory-map semantics

`bootinfo::validate_regions` rejects empty entries, zero-size entries, excessive counts, and `start + size` overflow. `memmap_norm` is the defensive normalization layer. It rejects arithmetic overflow, sorts and deduplicates boundaries, emits no gaps, applies `Unusable > AcpiReclaim > Reserved > Bad > Usable`, maps unknown kinds to `Reserved`, preserves provenance, and merges adjacent intervals only when kind and source are identical.

Overlapping reserved, NVS, ACPI, or bad-memory entries cannot become usable RAM merely because a usable entry also covers the same bytes.

## Validation façade

`boot_foundation::ValidatedBoot::validate` requires a valid BootInfo, an exact BootInfo count/slice count match, valid non-empty region entries, sufficient bounded output storage, deterministic normalized output, and at least one usable interval. Only its `usable_ranges()` iterator is permitted as input to later physical memory initialization. The façade does not dereference `memmap_paddr`; the loader adapter owns the bounded physical read and must establish that the slice is safe.

## Existing Multiboot v1 path

`arch/x86_64/entry.S` still receives Multiboot magic in `EAX` and the Multiboot information pointer in `EBX`, checks protocol and CPU prerequisites, builds the bootstrap identity map, and calls `kernel_main(magic, info)`. `kernel_main` retains its bounded Multiboot map parser and reservation/allocator path.

This is an intentional staged boundary, not a claim that Multiboot is BootInfo v1. The missing dependency for live BootInfo integration is a loader that constructs and owns the 88-byte EUK BootInfo plus its 24-byte entry array, including truthful kernel physical bounds and entry metadata. No such loader is present in this repository. The Multiboot path remains operational and is not converted by manufacturing BootInfo values.

## Reservations and unsupported scope

The existing allocator path reserves the bootstrap prefix, kernel image, Multiboot information, and memory-map storage before exposing free ranges. `reserve.rs`, `early_alloc.rs`, and `phys_handoff.rs` retain their ownership invariants: reserved and allocated ranges are never offered as free memory; handoff is all-or-nothing and seals only after receiver acceptance.

UEFI, ExitBootServices, DTB parsing, SMP, AArch64 live loader integration, and a Multiboot-to-EUK conversion are outside this batch.

## Inherited platform and security constraints

These constraints describe the existing platform baseline; Section A(i) does
not claim to implement or change them.

- x86-64 retains the existing 1 GiB supervisor identity map using 2 MiB pages.
  W^X enforcement and 4 KiB remapping remain separate work.
- AArch64 remains at the existing MMU-off platform-contract stage. This batch
  does not provide a live AArch64 EUK loader or translation-table implementation.
- Boot-time CPU faults are fatal: the fault path reports the failure and halts.
  No general fault recovery is claimed.
- Secure Boot and signature verification are not supported by this milestone.
- The default boot path does not enable interrupts. Any separately configured
  interrupt self-test is not evidence that the default path enables them.

The existing Multiboot v1 path remains the executable x86-64 boot path.
UEFI loader integration, ExitBootServices handling, DTB parsing, SMP, and
architecture-specific live loader integration remain outside Section A(i).

## Verification commands

```text
python3 ExileKernel/scripts/test_a1.py
python3 ExileKernel/scripts/test.py
python3 ExileKernel/scripts/build.py --output /tmp/euk-a1
python3 ExileKernel/scripts/boot_test.py
```

The supported reproducible image build requires Rust 1.91.1 with `x86_64-unknown-none` libraries as documented in `TOOLCHAIN.md`. No QEMU result is claimed unless `boot_test.py` actually runs.
