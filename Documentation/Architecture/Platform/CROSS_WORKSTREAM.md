# Cross-workstream dependencies

| This work | Depends on / informs | Interface | State |
|---|---|---|---|
| `build/exile_build.py` ↔ `validation/exile_validate.py` | Validation orchestrator (WO1 integration) | `build.bare-metal` stage; `build-manifest.json`; exit codes 0/3/4 | Integrated, 42 orchestrator tests pass |
| Platform `boot::info` flags (`RECOVERY`) | Core Identity: isolation & recovery (`wt-core`, `Documentation/Architecture/CoreIdentity/ISOLATION_AND_RECOVERY.md`) | A recovery-mode flag the loader passes to the kernel | Flag defined; consumer not written |
| `boot::slots` rollback | Core Identity recovery / ExileStore update pipeline | Slot `Good` confirmation must come from a trusted userspace service | Not wired |
| Device registry / `BlockDevice` | LibExilePolicy permission classes `dev.usb`, `dev.bluetooth`, `dev.camera`, `dev.microphone`, `dev.location` | Policy gates which service may bind/open a device; kernel registry does not enforce policy itself | Interfaces only |
| Installer | LibExilePolicy `sys.install` (treated as a critical permission) | Install flow must require it; installer is not written | Planned |
| Kernel pledge/unveil, Batch 11 caps/IPC | Process isolation claims | Executed ring-3 tests required before any claim | Blocked on QEMU |
| `toolchain.pin` | Everyone | Pins Rust 1.91.1 / Zig 0.15.2; crate edition 2024 and let-chains require ≥1.88 | **Crate verified only on rustc 1.97, not 1.91.1** |

No other workstream's files were modified. The `wt-core` worktree was read, not edited.
