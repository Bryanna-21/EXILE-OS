# REQUIREMENTS_GAP_MATRIX.md (PROPOSED DRAFT, not written to the repo)

Baseline: `main` @ `18a3c70`. Status values: **Met**, **Partial**, **Missing**, **Unverified**.
"Unverified" means code exists but no build/boot evidence was obtainable in this audit.

| # | Requirement | Status | Evidence | Missing pieces | Depends on | Work |
|---|---|---|---|---|---|---|
| 1 | Independent OS, attribution preserved | Partial | Root `LICENSE` (BSD-2-Clause, SerenityOS developers) intact; `Meta/Exile/IDENTITY.md` states Serenity origin | README says "independent" and "built from-scratch by us"; ~7.5k files carry upstream notices | Legal review | Reword README, keep notices, add NOTICE/third-party file |
| 2 | Future EUK | Missing | No EUK directory or design doc | Whole kernel; clean-room process | #3, #4 | See EUK_INTEGRATION_PLAN.md |
| 3 | x86-64 + ARM64 support | Unverified | Arch source present, CMake selects, CI matrix lists both | Any boot evidence; CI is red | CI repair | Fix CI first, then record QEMU boot per arch |
| 4 | Hybrid kernel architecture | Missing | Drivers, FS and network all under `Kernel/` (in-kernel) | User-space driver/service model, IPC contract | EUK design | Decide service split before EUK code |
| 5 | Real privilege boundaries / least privilege | Partial | pledge, unveil, credentials, W^X claimed by README | Store permissions are display-only; no sandbox profile per app | #7 | Per-app sandbox policy, enforced install-time grants |
| 6 | Reliable, secure updates | Missing | Store has dev signing (Ed25519 via OpenSSL) and local trust file | Root of trust, rotation/revocation, rollback protection, OS image update, atomic A/B | Boot guardian, Store backend | Design signed-update chain |
| 7 | Verified boot / integrity | Partial | SHA-256 + manifest format + host tool exist | `verify_kernel_integrity()` returns true, uncalled, unbuilt; manifest unsigned | EFIPrekernel hook | Wire, sign, fail closed, test |
| 8 | Native desktop | Partial | WindowServer/LibGUI inherited, README screenshot | Exile identity, shell/launcher | Branding | Rebrand + UX pass |
| 9 | Native mobile | Missing | No touch/gesture stack, no mobile shell found | Input, shell, power, radios | #4, drivers | Separate roadmap |
| 10 | Native apps + functional Store | Partial | Host tests pass (2/2); native app lists catalog only | Install, launch, remote catalog, sandbox, real package format | Installer service | Implement `IPackageBackend` against OS |
| 11 | Networking | Partial | IPv4/IPv6, TCP/UDP, e1000/RTL8168/GEM | Wi-Fi, mobile data, Bluetooth (not found) | Drivers | Hardware-driven |
| 12 | Storage | Partial | ext2, FAT, ISO9660, NVMe/AHCI paths in Kernel | Journaling/atomic updates, encryption | #6 | FS decision |
| 13 | Graphics | Partial | Bochs/Intel/VMware/VirtIO | ARM SoC GPU, acceleration | Hardware targets | Pick target boards |
| 14 | Audio | Partial | AC97, IntelHDA | ARM audio | Hardware targets | Driver work |
| 15 | Input | Partial | PS2/HID/VirtIO | Touch | #9 | Driver + stack |
| 16 | Power management | Partial | PowerState per arch, ACPI | Suspend, battery, DVFS (none found by filename) | #9 | Driver + policy |
| 17 | Reliable boot, diagnostics, tests, crash, recovery | Partial | CrashDaemon, CrashHandler, Coredump, kernel self-test mode, 62 files in `Tests/Kernel` | No recovery partition/rollback; CI red | CI repair | Recovery design |
| 18 | Android app compatibility | Missing | No runtime found | Runtime, services layer, Binder-equivalent | #4, #10 | Long-term |
| 19 | Branding: black/white/green/blue, no purple | Partial | Store icon uses blue/green on near-black; OS themes untouched | Inherited themes: `VisitedLink=magenta` in many, `SyntaxType=violet` and periwinkle borders in Dark, a Plum theme; wallpapers/icons unreviewed | None | Theme/asset audit then rebrand |

Do not reimplement: filesystems, TCP/IP stack, scheduler, WindowServer, LibCrypto/TLS. They exist; the gaps are verification, integration and hardening.
