# Hardware abstraction layer and driver capability matrix (WO4)

Target order: x86_64 + UEFI first. Code: `Meta/Exile/platform/src/hal/`. All 67 platform tests are host tests against
fixtures and mocks. **They do not validate physical devices** and no QEMU run happened in this environment.

Levels: **D** discovery (find/identify) · **I** initialization (reset/enable/configure) · **T** data transfer · **F** full driver
(lifecycle, errors, interrupts, power, hot-unplug).

| Component | D | I | T | F | What exists | Tested how | Needs real HW / QEMU |
|---|---|---|---|---|---|---|---|
| UEFI memory map | ✅ parse | — | — | — | `memmap::parse_uefi` (descriptor stride aware) | fixtures | Real firmware maps |
| Multiboot memory map | ✅ parse | — | — | — | `memmap::parse_multiboot1` | fixtures | QEMU boot |
| ACPI RSDP/RSDT/XSDT/MADT/MCFG | ✅ parse | — | — | — | `acpi.rs`, checksum/length hardened | hand-built tables | Real tables (OVMF/QEMU first) |
| CPU features | ✅ | — | — | — | `cpu.rs` decode + required-feature check | host's own CPUID + fixtures | Other CPUs |
| PCI/PCIe config | ✅ enumerate, bridge recursion, BAR sizing, caps | ❌ | — | — | `pci.rs` over `ConfigAccess` | mock topology (loop-safe) | QEMU/HW; ECAM mapping not implemented |
| LAPIC | — | 🟡 register logic | — | — | `lapic.rs` enable/EOI/timer/IPI encodings | mock registers | QEMU; calibration unimplemented |
| I/O APIC | 🟡 MADT entry only | ❌ | — | — | — | — | — |
| NVMe | 🟡 CAP/VS decode | ❌ | ❌ | ❌ | `devices::decode_nvme_*` | fixtures | **No driver**: no queues, no DMA |
| AHCI/SATA | 🟡 CAP/PI/port signature | ❌ | ❌ | ❌ | `decode_ahci`, `classify_sata` | fixtures | **No driver** |
| xHCI/USB | 🟡 capability decode | ❌ | ❌ | ❌ | `decode_xhci` | fixtures | **No driver** |
| Network | ❌ | ❌ | ❌ | ❌ | `NetDevice` trait only | — | **No driver** |
| Framebuffer | 🟡 descriptor check | — | ✅ software pixel/fill into a slice | ❌ | `fb.rs` | heap buffers | Real GOP framebuffer mapping |
| Block interface | — | — | 🟡 `BlockDevice` + `RamDisk` mock | — | `devices.rs` | RAM mock | Real controller |
| Device registry / lifecycle | ✅ | ✅ state machine | — | 🟡 | `registry.rs` | mock driver | Real drivers |

Honest summary: **discovery parsers exist; no storage, USB or network driver exists.** Do not read "decoder" as "driver".
Mocks (`Mock` config space, `Mock` MMIO, `RamDisk`) exist so parsers can be exercised on a host; passing them proves parser
logic only.

Hardening applied (and mutation-tested): checksum + length validation before trusting any ACPI table; zero-length MADT entries
rejected (would loop); bridge visited-bitmap + depth limit; BAR decode disabled during sizing and restored; capability list hop
bound; all I/O range checks overflow-safe.
