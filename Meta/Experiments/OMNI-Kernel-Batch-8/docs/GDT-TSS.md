# GDT and TSS

| index | selector | descriptor |
|---|---|---|
| 0 | 0x00 | null |
| 1 | 0x08 | kernel code, 64-bit |
| 2 | 0x10 | kernel data |
| 3 | 0x18 (0x1B with RPL 3) | user data |
| 4 | 0x20 (0x23 with RPL 3) | user code, 64-bit |
| 5 to 6 | 0x28 | 16-byte TSS descriptor (available TSS, access 0x89) |

The order is fixed by SYSCALL/SYSRET: `STAR[47:32] = 0x08` (kernel CS, SS = +8) and `STAR[63:48] = 0x10` give a SYSRET
CS of 0x23 and SS of 0x1B. `desc::validate_selector_layout` and a Batch 7 compatibility test assert this against
`omni_kernel7::msr`, so changing either side breaks a test.

`desc::Gdt::new(tss_base)` builds the table, rejects a null or non-canonical TSS base, and `validate` re-checks every
entry (present, DPL, long-mode bit, TSS type and limit). The kernel loads it with `lgdt`, reloads CS with a far return and
the data selectors, then runs `ltr 0x28`.

## TSS

104 bytes, `repr(C, packed(4))`. `rsp0` is the kernel stack for ring 3 to ring 0 transitions; `iomap_base` equals the TSS
size so there is no I/O permission bitmap and any port access from ring 3 faults. IST1 to IST5 point at dedicated 16 KiB
stacks (`IDT-EXCEPTIONS.md`); all pointers must be non-zero where required, canonical and 16-byte aligned or construction
fails.

## Status

Implemented, compiled, unit-tested on the host (`desc_tests`). `lgdt`, `ltr` and the segment reload have **not been
executed**.
