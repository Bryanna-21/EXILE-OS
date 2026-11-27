# D6 — Capability, IPC and service isolation

Status: host-tested and compile-checked. **Not activated at boot.** Enabled only by `build.py --d6` (cfg `euk_d6`).

## Layers (ten files, `ExileKernel/src/`)
| File | Role |
|---|---|
| `object.rs` | Kernel objects (generation-checked ids, Active→Revoked→destroyed), owners, the single D6 `Error` enum, per-owner quotas |
| `capability.rs` | Capabilities: closed rights bitset, attenuation-only derivation, depth limit 8, subtree revocation, sweeps by object/owner |
| `handle.rs` | Process-local handle tables (`gen<<16 | slot+1`, 0 invalid) and the process registry |
| `ipc_message.rs` | Wire format v1 (16-byte header, ≤128 B payload, ≤4 transfers), kernel-stamped sender, fuzzed decoder |
| `ipc_endpoint.rs` | Channels, listeners, connect/accept (backlog 4), endpoint reaping |
| `ipc_transfer.rs` | Two-phase capability transfer (stage → deliver), COPY/MOVE, all-or-nothing, revocation of in-flight caps |
| `ipc_wait.rs` | Blocking recv/send/accept on D5 `SyncDomain` queues, cancel, timeout, thread exit, process teardown |
| `ipc.rs` | The `Ipc` core: send/recv, queues (8/direction), process refs, invariants, shared test rig |
| `service.rs` | Service registry/broker with no ambient authority; restart policy interface; `EchoService` |
| `driver.rs` | Driver interface reference monitor (protocol-level mediation) |

## Guarantees (host-tested)
* A handle is an unforgeable index into the caller's table; every use re-checks generation, owner and rights.
* Rights only shrink on derivation; unknown rights bits are rejected; revoking a capability revokes its descendants.
* Transfer is atomic: on any failure nothing is queued and no reference moved. A capability revoked while in flight yields `TransferRevoked` and the message is consumed.
* Object memory is reclaimed only when revoked and unreferenced (`refs`, `inflight`, `ops` all zero).
* Blocking uses D5 wait queues; check-and-block is atomic under the lock chain (kernel slot → sync slot → IPC slot); blocking is refused in interrupt / preempt-disabled context.
* Process teardown releases every handle, capability, wait and endpoint; peers see `PeerClosed`.

## Driver isolation — what is and is not claimed
`driver.rs` mediates the **message path** only (RECV-only ends on both sides, kernel-built messages, request validation, kernel-assigned tags, bounded pending table, failure containment). It does **not** confine driver code: a driver running in Ring 0 is not isolated, and `Isolation::hardware_isolated()` is `false`. Hardware isolation needs the D5 user-mode path, which D6 does not activate.

## Limits
Fixed capacities: 160 objects, 256 capabilities, 48 endpoints, 16 channels, 16 processes, 32 handles/process, 2 drivers (4 clients each), 8 services.
