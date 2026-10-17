# REPOSITORY_INVENTORY.md (PROPOSED DRAFT, not written to the repo)

Scope: public `Bryanna-21/EXILE-OS`, branch `main`, commit `18a3c7007cf61eca2f7b26046620a9f6440c42a3`
("Integrate Exile Store and harden package installation", Bryanna, 2026-10-16 23:28 +0300).
Method: depth-1 clone read-only; 18,936 tracked files. Local `~/Downloads/EXILE-OS` was NOT reachable, so local
branch, status, stashes, ignored files and running builds are **unverified**. History/blame unavailable (shallow).

Evidence labels used below: **source present**, **compiles**, **links**, **emulator boots**, **hardware tested**, **unverified**.
Nothing in this audit was built or booted as a full OS (no cross toolchain or QEMU in the audit sandbox, and CI is red).

## 1. Top-level inventory (tracked files)

| Path | Files | What it is | Class |
|---|---:|---|---|
| Userland | 7,571 | Libraries (LibWeb, LibJS, LibGUI, LibGfx, LibCrypto, LibTLS...), Services (WindowServer, SystemServer, AudioServer, NetworkServer...), Applications, Utilities, Shell, Games, Demos | Essential (base) / many optional apps |
| Tests | 4,168 | Per-library tests, Kernel tests (62 files), LibWeb text/screenshot/WPT imports | Essential (CI) |
| Base | 3,862 | Root filesystem payload: themes, fonts, icons, wallpapers, man pages, sample home files | Essential, branding work pending |
| Kernel | 1,223 | In-kernel everything: Arch, Boot, Bus, Devices, FileSystem, Net, Memory, Tasks, Syscalls, Security | Essential |
| Ports | 1,063 | 367 `package.sh` recipes + patches (sources fetched at build time, not verified per file) | Optional |
| Meta | 493 | Build/run scripts (`exile.sh`, `run.py`, image builders), CMake, Lagom host tools, lint, website | Essential |
| AK | 246 | Core C++ container/utility library | Essential |
| Ladybird | 127 | Qt/AppKit browser frontends (macOS/Linux host build) | Optional |
| Toolchain | 60 | GNU/Clang/QEMU/EDK2/Jakt build scripts, patches, stub `.so` per arch | Essential for cross-build |
| Documentation | 60 | Build/dev docs, RISC-V, Raspberry Pi, kernel notes | Essential, partly stale |
| .tmp | 22 | Windows build experiments (see REMOVAL_CANDIDATES) | Removal candidate |
| .github | 18 | CI, actions, CODEOWNERS, dependabot, 3 disabled workflows | Essential, needs rework |

Exile-authored additions found: `Userland/Applications/ExileStore/`, `Kernel/Boot/Exile/`,
`Meta/Exile/IDENTITY.md`, `Meta/CMake/exile_*.cmake`, `Meta/exile.sh`, `Meta/Tools/ExileBootManifest.cpp`,
`Meta/Lagom/Tools/ExileBootManifest/`, `.github/workflows/exile-template.yml`. Everything else is SerenityOS-derived.

## 2. Subsystem findings

| Subsystem | Source paths | Evidence | Limitations | Class |
|---|---|---|---|---|
| Kernel core | `Kernel/Tasks` (Process, Thread, Scheduler), `Kernel/Memory`, `Kernel/Locking` | source present; compile/boot unverified | Drivers, filesystems and network stack all live in-kernel: **monolithic-style, not hybrid** | Essential |
| Syscalls | `Kernel/Syscalls` (73 files), `Kernel/API/Syscall.h` (~158 enumerated syscalls by grep) | source present | POSIX-like ABI inherited | Essential |
| Security | `Kernel/Security`, `Syscalls/pledge.cpp`, `unveil.cpp`, `Credentials.*` | source present (pledge/unveil/credentials/KASLR claimed in README, KASLR not independently checked) | No update signing/secure boot chain in use | Essential |
| Boot | `Kernel/Prekernel`, `Kernel/EFIPrekernel` (x86_64/aarch64/riscv64 subdirs), `Meta/build-image-*.sh` (extlinux, grub, limine, uefi, rpi, qemu) | source present for all three arches | Boot integrity not enforced (see Exile Boot Guardian) | Essential |
| Exile Boot Guardian | `Kernel/Boot/Exile/*` | SHA-256 verified correct (13/13 lengths incl. padding boundaries vs Python hashlib) | `verify_kernel_integrity()` **always returns true**, has **no callers**, not in any CMake source list | Incomplete / dead code |
| Filesystems | `Kernel/FileSystem` (Ext2FS, FATFS, ISO9660FS, FUSE, procfs/sysfs/devpts) | source present | No journaling/atomic-update FS seen | Essential |
| Network | `Kernel/Net` (IPv4/IPv6, TCP, UDP, local sockets; e1000, RTL8168, Cadence GEM, loopback) | source present | No Wi-Fi/cellular/Bluetooth found by path | Essential, incomplete for mobile |
| Graphics | `Kernel/Devices/GPU` (Bochs, Intel, VMWare, VirtIO, 3dfx, generic) | source present | No ARM SoC GPU drivers found beyond RPi framebuffer | Partial |
| Audio | `Kernel/Devices/Audio` (AC97, IntelHDA) | source present | No ARM audio found by path | Partial |
| Input | `Kernel/Devices/Input` (PS2, HID, VirtIO) | source present | No touch/gesture stack found | Partial |
| Power | `Kernel/Arch/*/PowerState.*`, x86 ACPI | source present | No suspend/battery/cpufreq files by filename search | Partial |
| Desktop | `Userland/Services/WindowServer`, `Taskbar`, `LibGUI`, `LibGfx` | source present, README screenshot | Inherited Serenity look | Essential |
| Browser | `Userland/Applications/Browser`, LibWeb/LibJS, `Ladybird/` | source present | Large optional surface | Optional |
| Exile Store | `Userland/Applications/ExileStore` (47 src files) | Host build + tests: configure 0, build 0, `ctest` 2/2 pass (scratch copy). Native app is a 220-line read-only catalog browser | Install/launch disabled; no privileged installer, no enforcement of permissions, no TLS transport, no root of trust (its own INTEGRATION.md) | Incomplete |
| Android runtime | none | no binder/ART/Dalvik; grep hits are incidental strings | Entire layer missing | Missing |
| Mobile shell | none found | Store's web UI is responsive but has no device launcher | Missing | Missing |

## 3. Architecture support

| Arch | Arch files (tracked) | Build selects it | Links | Emulator boots | HW tested | CI |
|---|---:|---|---|---|---|---|
| x86-64 | 89 (+ ACPI, APIC, HPET, PIC, PIT, Bochs/VMware drivers) | `EXILE_ARCH=x86_64` in `Kernel/CMakeLists.txt` | unverified | unverified | unverified | GNU+Clang matrix and ALL_DEBUG job; **latest runs red** |
| ARM64 | 101 (GICv2/v3, PSCI, MMU, Raspberry Pi + `virt` platform init, RP1 drivers) | `EXILE_ARCH=aarch64` | unverified | unverified | unverified (RPi code present) | GNU job; red |
| RISC-V | 45 (SBI, PLIC, MMU, boot.S) | `EXILE_ARCH=riscv64` | unverified | unverified | unverified | GNU job; red. `Documentation/RISC-V.md`: no SMP, no ACPI |

All three: **source present**. Compile/link/boot/hardware: **unverified**.
