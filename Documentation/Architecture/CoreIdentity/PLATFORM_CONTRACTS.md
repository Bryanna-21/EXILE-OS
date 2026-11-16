# Shared platform contracts (Priority 4)

Rule: one implementation per concern. Existing services are reused or wrapped, not duplicated.

| Concern | Single owner | Others must |
|---|---|---|
| Permission vocabulary and scope matching | `LibExilePolicy::permission` | Never parse or compare permission strings themselves |
| Allow/deny decisions | `LibExilePolicy::Broker` | Ask; never cache an Allow across a revocation epoch |
| Package integrity/authenticity | `ExileStore` `trust.*`, `installer.*` | Reuse; extend with `signingPayloadV2` after review |
| SHA-256 | `ExileStore/src/core/sha256.*` | LibExilePolicy reuses it by path; do not add another |
| Kernel confinement | `Kernel/` pledge/unveil/unshare | Go through `kernel_map` for the grant→promise mapping |
| Service restart | `SystemServer` | Broker hooks (`onProcessExit`, re-registration) are added *to* it, not a second supervisor |

## Interfaces

1. **XAM → Broker:** `registerPrincipal(Principal, PermissionSet declared)` after verification; `updateDeclared` on update; `revokeAll` on removal. Declared sets come only from a verified manifest (or `mapLegacyIds` for catalog-only packages).
2. **Native app / EIS agent → Broker:** `request(principal, permission, resource, justification)` → `Decision{allow|deny|needs-approval, reason}`. Agents get `AgentFacade` only. (Library API today; IPC form **Proposed** and must authenticate the caller by kernel-provided identity, never by a string the caller supplies.)
3. **Service (D2) → Broker:** before acting on behalf of a caller, `request` and obey; no answer = deny.
4. **Launcher → Kernel:** `projectAndMark(principal)` → promises string + unveil list, applied before `exec`, then veil lock. Items in `brokerMediated` are enforced by services via interface 3.
5. **SystemServer → Broker:** `onProcessExit(principal, crashed)` on service/app exit; re-register and re-project on restart.
6. **Everything → Audit:** only the broker writes the audit chain.

## Compatibility with existing architecture

New code lives in `Userland/Libraries/LibExilePolicy/` (Exile-prefixed, per `EXILE_ARCHITECTURE.md` §2) and `Documentation/Architecture/CoreIdentity/`. No change to `Kernel/`, `ExileKernel/`, `AK/`, ExileStore sources, or any OMNI batch. It is a standalone host-built C++20 library with its own `CMakeLists.host.cmake`; it is **not** wired into the native Exile build (unverifiable here), and it has no dependency on the experimental OMNI Rust capability model (a future kernel capability table can replace `kernel_map` without changing the broker interface).

## Open decisions for the maintainers

1. Confirm the intended "three execution environments" (README §1).
2. Confirm what "XAM" and "EIS" are meant to cover.
3. Whether the production package format carries the manifest and v2 signature (blocks wiring).
