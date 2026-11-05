# Release-readiness review

Gaps that would stop Exile OS from becoming an independent, usable operating system, grouped by the areas requested. These are documented for later phases; none is implemented here. Evidence labels as in the audit.

| Area | Gap | Evidence | Needed |
|---|---|---|---|
| Boot and recovery | Verified boot unproven on a target; no recovery partition or environment; no rollback protection; only the EFI path has any verification | `Documentation/BootIntegrity.md`; addendum findings 5 and 13 | M2, M2b, M7; a monotonic counter from firmware or kernel |
| Hardware support | Inherited set only; no Wi-Fi; no GPU acceleration; ARM and RISC-V boards unverified | `Documentation/BareMetalInstallation.md`; CI matrix lists aarch64 and riscv64 builds only | Driver work (kernel side); a hardware compatibility test plan |
| Driver architecture | Drivers live in the kernel; user-space driver model absent | `Kernel/Devices`, `Kernel/Net`; older `REQUIREMENTS_GAP_MATRIX.md` item 4 | Kernel/EUK design decision |
| System services | No log, health, time or update service; `SystemServer` restart policy undocumented and untested | audit 3.2 | M3, M4 |
| Networking | No Wi-Fi, firewall or connection monitor; wired only | audit 3.4 | Drivers; kernel packet-filter hook |
| Storage | No mount manager, storage-health reporting, encryption or secure storage; Trash exists | audit 3.3 | Kernel interfaces for health and encryption |
| Application permissions | Store permissions are displayed, not enforced; no per-app sandbox profile | `ExileStore/docs/SECURITY.md` limit 1 | Kernel enforcement (pledge/unveil policy per app) |
| Package signing and updates | Dev-only trust store; unsigned catalog; no HTTPS transport; no root of trust; no update or rollback | `ExileStore/docs/SECURITY.md` limits 4 to 6 | M6 |
| User management | `LoginServer`, `UsersSettings` exist; authentication integration and first-user setup flow not verified | `Userland/Services/LoginServer`, `Userland/Applications/UsersSettings` | Phase 2 audit and tests |
| Logging and diagnostics | `dmesg` and `CrashDaemon` only; no structured log, retention or access control | `Userland/Utilities/dmesg.cpp`, `Userland/Services/CrashDaemon` | M4 |
| Reproducible builds | None demonstrated; the EUK README describes a two-output hash check for its own build only | `ExileKernel/README.md` | A two-build comparison for the OS image (M7) |
| Installation and upgrades | Manual `dd` of a built image; no installer, ISO, upgrade path | addendum findings 3 and 4 | M7 |
| Build health | The host build does not compile on current `main` | addendum finding 1 | M0 (blocking everything that needs host tests) |
