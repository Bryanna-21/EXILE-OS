# Memory

## Map ingestion (`src/multiboot.rs`)

The info structure must be 4-byte aligned and wholly inside the first GiB (the boot identity map). `parse_map` reads the
loader's memory map and fails closed on a short record, a bad record size, overflow, too many regions or overlapping
regions. `reserved_ranges` collects everything the loader handed over that must not be reused: the info structure, the
memory map, command line, loader name, module table, each module and each module string (strings get a 4 KiB reserve
because their length is unknown). `usable_ranges` subtracts all of that plus everything below the end of the kernel image
from the usable regions, narrowed to page boundaries and clipped to 1 GiB.

## Frame allocator (`src/frames.rs`)

Two bitmaps (pool, free) for up to 262,144 frames (1 GiB). `alloc` returns the first free frame at or after a hint, wrapping around; `free` rejects an address that is
misaligned, out of range, outside the pool, or not currently allocated (which covers a double free). Frames outside the
pool can never be handed out. The kernel only adds RAM below `ram_top`, the highest usable address rounded
down to 2 MiB, because only that RAM is mapped.

## Page tables (`src/paging.rs`)

4-level x86-64 tables built through a `Mapper` over a `PhysMem` trait, so the same code runs against a simulated memory
in unit tests and against identity-mapped RAM in the kernel. Rules enforced on every mapping:

* 4 KiB and 2 MiB pages must be aligned; physical addresses must be below 2^52.
* A mapping may not be writable and executable.
* Mappings with the user bit must lie wholly inside the user window `[0x4000_0000, 0x8000_0000)`; kernel mappings may
  not touch it.
* Re-mapping an existing page, or mapping through a huge page, is an error.
* `translate` reports effective permissions as the AND of the write and user bits and the OR of NX over all levels.

## Kernel address space

| range | mapping |
|---|---|
| 0 to 4 KiB | unmapped (null dereference faults) |
| 4 KiB to 1 MiB | read/write, no-execute |
| `.text` | read, execute |
| `.rodata` | read |
| `.data`, `.bss` (including boot stack, page-table scratch, thread stacks) | read/write, no-execute |
| end of image to 2 MiB | read/write, no-execute |
| 2 MiB to `ram_top` | read/write, no-execute, 2 MiB pages |
| everything else | unmapped |

Identity mapped (virtual = physical). There is no higher-half kernel in Batch 8.

## Page faults

Reports decode the error code (read/write/fetch, present/not-present, user/supervisor, reserved bit, protection key)
and print CR2. The boot self-test provokes and checks: write to text, execute from a no-execute page, read of an
unmapped address, read of the null page, plus positive controls (mapped read, write to RW memory, call into text).

## Status

Parsing, allocation and mapping logic: unit-tested on the host (32 tests, including negative cases). The CR3 switch and
the page-fault self-tests are compiled but **not executed**.

## Limits

1 GiB ceiling; no frame reclamation of page tables; no TLB shootdown (single CPU); no large-address (above 4 GiB)
memory; no PCID/PGE; no guard pages under thread stacks (Batch 7 uses a canary).
