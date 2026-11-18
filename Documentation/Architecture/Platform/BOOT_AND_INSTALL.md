# Boot sequence, handoff, installation and update (WO6)

## What exists today (facts)
* **BIOS/Multiboot1 live image path**: Batch 8 harness builds `boot/omni8.elf` and a GRUB `grub-mkrescue` ISO with
  `multiboot /boot/omni8.elf`. It was **never booted** in this environment.
* **No UEFI bootloader exists.** The orchestrator and build pipeline (`Meta/Exile/build`) produce BIOS/GRUB ISOs only.
* **No installer exists.** `Meta/Exile/installer/exile_disk.py` writes a *partition-table skeleton* into a regular file.

## Boot sequence (target design; steps marked ⛔ are unimplemented)
1. Firmware (UEFI) loads `\EFI\BOOT\BOOTX64.EFI` from the ESP. ⛔ bootloader not written.
2. Loader reads A/B record (`boot::slots`, two CRC'd copies) → chooses slot, consumes a try. ✅ logic (hosted)
3. Loader loads the kernel ELF from the chosen root slot, gathers UEFI memory map + RSDP + framebuffer. ⛔
4. `ExitBootServices`; memory map is now final (`RegionKind::BootloaderReclaimable` becomes usable later). ✅ parser
5. Loader builds a `BootInfo` blob (`boot::info`) and jumps to the kernel entry with its address. ✅ structure, ⛔ jump
6. Kernel validates `BootInfo` (magic, major version, length, CRC, every range) before use; refuses on any error. ✅
7. Userspace confirms health → `Record::mark_good(slot)`; otherwise tries run out and the loader falls back. ✅ logic

## BootInfo v1 (`platform/src/boot/info.rs`)
Little-endian, offset-based (no struct cast, no alignment hazard). 48-byte header: magic `EXBI`, major/minor u16, total_len,
CRC-32 (header field zeroed during computation), flags (UEFI / exited-boot-services / recovery), RSDP, memmap (off,len),
cmdline (off,len). Compatibility rule: **different major ⇒ reject; newer minor ⇒ accept**. CRC detects accidental corruption only;
it is not authentication (Secure Boot / signed images are PLANNED, not implemented).

## Partition layout (`exile_disk.py`, GPT, 1 MiB aligned)
ESP 64 MiB · ROOT-A 256 MiB · ROOT-B 256 MiB · STATE (remainder). Protective MBR, primary + backup GPT, CRCs.
Safety: regular files only; `/dev/*`, block/char devices and symlinks to them are refused with no override.

## Update and rollback
`begin_update` writes to the inactive slot and marks it Trial with 3 tries; unconfirmed trials fall back automatically;
`rollback()` only to a Good slot; updates never start from an unconfirmed slot. Persistence survives power loss at every
byte offset of the record write (simulated).

## Recovery / rescue (design only)
`flags::RECOVERY` is carried in BootInfo; a rescue kernel/initramfs entry and installer UI are PLANNED.

## Capability matrix — live image vs installer
| Capability | Status | Evidence |
|---|---|---|
| Bootable BIOS live ISO (existing GRUB/Multiboot1) | **UNVERIFIED**: built-by-script only on host fallback; never booted | none |
| Bootable UEFI live ISO | **NOT IMPLEMENTED** | — |
| BIOS vs UEFI investigation | Documented here: BIOS path exists (GRUB), UEFI needs a new loader + FAT ESP image + El Torito EFI entry | design |
| Handoff structure + versioning | IMPLEMENTED-HOSTED | 5 tests |
| A/B slots, rollback, power-loss safety | IMPLEMENTED-HOSTED | 9 tests |
| GPT disk-image skeleton + validator | IMPLEMENTED-HOSTED | 8 tests (1 skipped: no `sfdisk`) |
| Filesystem formatting, kernel/bootloader copy | **NOT IMPLEMENTED** | — |
| Installer (disk detection, UI, flow) | **NOT IMPLEMENTED** | — |
| **End-to-end virtual install** | **NOT RUN / BLOCKED** (needs QEMU, firmware, mkfs) | — |

**No installation claim is made.** A "bootable live image" and a "functioning installer" are separate deliverables and
only parts of the second's foundation exist.

## Reproduction
```
CARGO_TARGET_DIR=/tmp/plat cargo test --offline --manifest-path Meta/Exile/platform/Cargo.toml
python3 -I -B Meta/Exile/installer/tests/test_exile_disk.py
python3 Meta/Exile/installer/exile_disk.py build /tmp/x.img --size-mib 700 --seed demo && python3 Meta/Exile/installer/exile_disk.py validate /tmp/x.img
```
