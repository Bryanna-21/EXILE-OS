# Isolation and recovery (Priority 1)

## Three runtime trust domains (proposal, grounded in existing mechanisms)

| Domain | Contents today | Authority | Boundary mechanism that exists today (S) |
|---|---|---|---|
| **D1 Privileged core** | `Kernel/` (C++); future `ExileKernel/` EUK (Rust) | Ring 0; all hardware | Hardware privilege; SMEP/SMAP/UMIP/NX (`Kernel/`); syscall entry |
| **D2 System services** | `SystemServer`-launched: WindowServer, LookupServer, RequestServer, ... | Ordinary user processes, each with a pledge/unveil set | Process isolation, `pledge`, `unveil`, local sockets / LibIPC |
| **D3 Applications and agents** | Native apps, `.expkg`-installed apps, EIS agents | Only what the broker granted | `pledge`/`unveil` at launch, `unshare` contexts + jail, broker for what the kernel cannot scope |

Mapping to the other "threes" found in the repo: hosted/virtualized/bare-metal (OMNI Batch 1) are *where D1 runs*, not separate trust domains — hosted results never validate D1 isolation; VFS-root / process-list / hostname contexts are the *D3 containment primitives*; the dual-kernel split is two implementations of D1, only one of which (legacy `Kernel/`) can host D2/D3 today.

## Boundaries and what crosses them

| Boundary | Crosses it | Enforced by | Status |
|---|---|---|---|
| D3 → D1 | syscalls | pledge promises, unveil veil, jail/unshare | Exists (S); never exercised here (N) |
| D3 → D2 | requests to services (files, network, windows) | Service checks the caller with the **broker** before acting | Broker built (T); service-side check **not implemented** |
| D3 → D3 | IPC, shared files | unshare process-list/VFS contexts, unveil | Exists (S) |
| D2/D3 → broker | `Request{principal, permission, resource}` → `Decision` | Broker policy | Library (T); transport/identity of caller **not implemented** |
| D3 (EIS agent) → anything | only `AgentFacade::request` | Type-level: the facade has no grant/revoke/approve (compile-time checked, T) | Process-level separation needs D1 |

Limits the kernel imposes (read from source): pledge cannot restrict **hosts/ports** (`inet` is all-or-nothing); pledge/unveil can only be **tightened**, never loosened, so revoking authority already projected into a process requires restarting it. LibExilePolicy models this: `RevokeResult.needsProcessRestart`, and `KernelProjection.brokerMediated` lists what must be enforced by the owning service.

## Failure containment

| Failure | Containment (designed) | Implemented / tested |
|---|---|---|
| App crash | `onProcessExit(crashed)` ends Once+Session grants and pending approvals; Standing/Until grants survive for the restart | Broker side T; SystemServer hook N |
| EIS agent misbehaves (flood, anomalous requests) | `suspend()` denies everything, closes approvals; only a user `resume()` lifts it; pending approvals capped at 32/principal | T |
| Update shrinks declared permissions | `updateDeclared()` revokes grants no longer covered immediately | T |
| Revocation of authority already in the kernel | Reported via `needsProcessRestart`; launcher must restart the process | T (logic); restart N |
| Broker unavailable | Enforcement points use `failClosedAllows(optional<Decision>)`: no answer = deny | T (helper); wiring N |
| Service crash (D2) | Existing `KeepAlive` restart, max 3 rapid failures | S only |
| Kernel fault (D1) | Panic/halt (EUK: serial-and-halt, no recovery). **No recovery claimed.** | S |

## Recovery gaps (honest list)

1. `SystemServer` restarts on count only: no backoff, health check, dependency ordering on restart, or state handover. A broker-aware supervisor should re-register principals and re-project grants on restart; not designed in code.
2. No A/B or rollback for D1/D2 images (EUK ROADMAP lists rollback policy as a future gate).
3. Audit log is in-memory; durable storage and an off-host anchor are required for the tamper-evidence to mean anything after a crash.
4. EUK has no exception recovery (no IDT): a fault triple-faults.

## What was implemented under this priority

Only the broker-side containment primitives above (suspend/resume/exit/updateDeclared/revoke and their audit trail), because they are pure policy and testable on the host. Nothing that needs a running kernel was implemented or claimed.
