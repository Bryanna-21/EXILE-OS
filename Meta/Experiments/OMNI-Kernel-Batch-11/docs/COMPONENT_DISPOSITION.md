# OMNI and EUK component disposition

Columns: **correct** (behaviour matches its own spec, as far as tests show), **tested** (what ran), **useful**, **compatible** with
`Kernel/` as primary, **decision**: integrate (port into `Kernel/`), redesign, or retain (keep as reference/test bed). Test statements are hosted unless noted.

| Component | Correct / tested | Useful | Compatible | Decision |
|---|---|---|---|---|
| Batch 1 spec (prose) | draft | reference | n/a | Retain |
| Batch 2 wire format, rights, handles (Zig) | 59 declarations, hosted | Defines the ABI Batch 11 follows (checked by 39 conformance checks) | Header and rights portable | Retain; port semantics (step 4/5) |
| Batch 3 capability lifecycle (Zig) | 56 declarations | Slot retirement idea reused | Portable concept | Retain |
| Batch 4 IPC core (Zig) | 63 declarations | Move/copy rules reused | Portable | Retain |
| Batch 5 kernel IPC + syscall layout (Zig) | 56 declarations | Source of 0x100..0x107 and layouts | Numbers need reserving in `Kernel/` | Retain |
| Batch 6 scheduler/arch contracts (Zig SIM) | 53 declarations | Contract reference | no code reuse | Retain |
| Batch 7 Rust scheduler + syscall entry | 56 tests incl. real context-switch asm | Base of Batches 9 to 11 | Cooperative uniprocessor: not a replacement for `Kernel/` scheduler | Retain as test bed |
| Batch 8 boot image | 65 hosted tests; never booted | Page-table `Mapper`, GDT/IDT | Overlaps `Kernel/` boot | Retain; boot test pending (blocked) |
| Batch 9 PagedBacking + conformance | 34 hosted tests | Safe user-copy design | Concept portable | Retain; port copy discipline |
| Batch 10 ABI adaptor, txmap, irqlock | hosted | `AbiMode::Exile` bridges the convention gap | Directly relevant to userland | Integrate the adaptor idea when `Kernel/` ABI is decided |
| Batch 11 capabilities + IPC | 18 hosted tests, 39 conformance checks | First Rust enforcement of capabilities | Portable design; no code link to `Kernel/` | Retain; port per migration plan |
| EUK `ExileKernel/` | QEMU-verified in another environment (its docs); not re-run here | Boot reference | Separate Multiboot kernel | Retain as reference |
| `Kernel/` | not built here | The primary kernel | n/a | Primary; unchanged |

Nothing was deleted or rewritten.
