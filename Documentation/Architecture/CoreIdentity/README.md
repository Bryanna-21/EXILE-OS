# EXILE Core Identity Architecture — assessment and inventory

Status: design + one implemented host-tested component (`Userland/Libraries/LibExilePolicy`).
Evidence vocabulary: **S** read in source · **T** a host test ran (and was mutation-checked) · **N** nothing executed.
Nothing here is runtime-validated on a booted kernel: this environment has no QEMU, GRUB or cross toolchain.

## 1. What the repository actually says (inspection findings)

| Brief term | Finding | Consequence |
|---|---|---|
| "XAM" | No occurrence in any file on `HEAD`, `main`, the integration branches or the EUK/Copilot branches (`git grep`). | Not an existing component. This work treats "XAM" as *the application trust layer around `.expkg`* and does not guess an expansion. |
| "EIS" | No occurrence (only an unrelated AArch64 register bit `EIS`). | Not an existing component. Specified here as a proposal; only its permission broker is built. |
| "Three execution environments" | Not defined for EXILE as such. The repo contains three different "threes": (a) OMNI Batch 1's **hosted / virtualized / bare-metal** backends; (b) `Documentation/Kernel/Containers.md`'s three kernel isolation mechanisms: **VFS root contexts, process lists, hostname contexts**; (c) `EXILE_ARCHITECTURE.md`'s **dual-kernel** split (`Kernel/` C++ legacy, `ExileKernel/` Rust EUK) plus polyglot userland. | I did not pick one silently. `ISOLATION_AND_RECOVERY.md` defines three **runtime trust domains** (privileged core, system services, applications/agents) from what exists, and maps each (a)–(c) onto them. **If the intended three are different, this is the one thing to correct first.** |
| `.expkg` | Exists: `Userland/Applications/ExileStore/src/core/expkg.{h,cpp}`, `Documentation/ExileFileFormats.md` §5. Declared a **development** format; production format "a separate integration point". | Not promoted to production here. See `XAM_PACKAGE_TRUST.md`. |

## 2. Existing-component inventory

| Area | Component | Where | Evidence | Notes |
|---|---|---|---|---|
| Isolation | `pledge` (28 promise names) | `Kernel/Syscalls/pledge.cpp`, `Kernel/Tasks/Process.h` | S | Real syscall; promise list is machine-checked by a LibExilePolicy test (**T**) |
| Isolation | `unveil` (paths, `rwxcb`) | `Kernel/Syscalls/unveil.cpp` | S | Path scoping only; no network scoping |
| Isolation | `unshare_open/create/enter` (VFS root ctx, process list, hostname ctx) + jail via `prctl(PR_SET_JAILED_UNTIL_EXIT/EXEC)` | `Kernel/Syscalls/unshare.cpp`, `Kernel/API/prctl_numbers.h`, `Documentation/Kernel/Containers.md` | S | Containers doc describes them; never exercised here |
| Isolation | SMEP/SMAP/UMIP/NX enabled in `Kernel/`; no KPTI | Batch 11 `GAP_MATRIX.md` | S | |
| Isolation | Capability tables + channel IPC with `PEER_DEAD` | `Meta/Experiments/OMNI-Kernel-Batch-11/src/{caps,ipc}.rs` | T (hosted, 18 tests) | Not in any booted kernel |
| Recovery | `SystemServer` `KeepAlive`: restart after non-zero exit, up to 3 quick failures then give up | `Userland/Services/SystemServer/Service.cpp:255-285` | S | No health checks, no state recovery, no backoff beyond count |
| Recovery | `CrashDaemon` + `Coredump` | `Userland/Services/CrashDaemon`, `Kernel/Tasks/Coredump.cpp` | S | Diagnostics, not recovery |
| Recovery | OMNI supervisor idea: clients get `PEER_DEAD`, must re-acquire | Batch 1 `ARCHITECTURE.md` §8; Batch 11 `ipc.rs` | S/T | Contract only |
| Trust | Ed25519 package signature over `(appId, version, package id, sha256, size)`; key store with revocation and per-publisher key restriction | `ExileStore/src/core/trust.*` | T (ExileStore suite passes; baseline run) | Permissions are **not** inside the signed payload |
| Trust | Install pipeline with explicit state machine; success only after backend confirmation | `installer.*` | T | |
| Trust | Atomic swap with `previous` copy, crash recovery, receipt file | `backend.cpp` `LocalPrefixBackend` | S/T | Unprivileged **dev** backend; TOCTOU, no sandbox (`docs/SECURITY.md` limits 1–9) |
| Trust | Permission *display* (id + reason, 13 coarse ids) | `catalog.cpp`, `ui/render.cpp` | T | **No enforcement anywhere** (`SECURITY.md` limit 1) |
| Boot | EUK Milestone-1 Multiboot skeleton | `ExileKernel/` | S (QEMU-verified per its own docs, not here) | No user mode, scheduler, syscalls or IPC |

## 3. Missing-component inventory

| Missing | Needed by | Built here? | Blocked on |
|---|---|---|---|
| Permission vocabulary shared by manifests, broker, EIS | XAM, EIS, native apps | **Yes** — `permission.*` (T) | — |
| Permission broker (grant/revoke/approve/suspend, default deny) | EIS, XAM, services | **Yes** — `broker.*` (T) | Enforcement point + trusted IPC (kernel) |
| Tamper-evident audit chain | EIS, XAM | **Yes** — in `broker.*` (T) | Off-host anchor for tail-truncation detection |
| Grants → pledge/unveil projection | launcher | **Yes** — `kernel_map.*` (T, vs real promise list) | Never applied to a live process (needs a booted `Kernel/`) |
| Permissions bound into the package signature | XAM | **Partly** — `signingPayloadV2` function (T); **not wired** into `trust.cpp`/`installer.cpp`/catalog | Review of catalog schema + re-signing sample packages |
| Manifest *inside* the package | XAM | No — spec only | Format decision (`.expkg` v2 vs production format) |
| Privileged installer service, TOCTOU fix, signed catalog, rollback protection | XAM | No | Kernel privilege separation; key management |
| Enforcement of permissions on running apps | all | No | Needs boot-validated kernel and launcher |
| Supervisor with health checks, backoff, state recovery | recovery | No — spec only | SystemServer redesign; runtime testing |
| EIS runtime, model hosting, local/cloud provider adapters | EIS | No — spec only | Not feasible to verify without a model runtime |
| ABI for IPC from userland to broker | contracts | No — spec only | Kernel IPC (Batch 11 not in `Kernel/`) |

## 4. Blocked by the current boot-validation milestone

Everything that must run on a booted kernel is blocked until a boot is actually validated (see `Meta/Exile/validation/exile_validate.py`: all `boot.*` stages are BLOCKED here for lack of QEMU/GRUB/xorriso). Concretely blocked: applying `kernel_map` output, any claim that revocation stops an in-flight process, jail/unshare behaviour, supervisor restart semantics, EUK anything-with-userland.
Not blocked and done: policy logic, grammar, audit chain, projection *specification*, tests.

## 5. Document map

- `ISOLATION_AND_RECOVERY.md` — domains, boundaries, interfaces, failure containment (Priority 1)
- `XAM_PACKAGE_TRUST.md` — package format status, trust model, manifest/permission spec, install/update/remove/rollback (Priority 2)
- `EIS_PERMISSION_BROKER.md` — local-first AI architecture and the broker contract (Priority 3)
- `PLATFORM_CONTRACTS.md` — interfaces between kernel, XAM, EIS, native apps; anti-duplication rules (Priority 4)
