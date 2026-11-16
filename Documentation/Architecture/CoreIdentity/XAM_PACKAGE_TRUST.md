# XAM application trust (Priority 2)

"XAM" is not defined in the repository (see README §1). This document specifies the application trust layer around `.expkg`
and labels every item **Exists**, **Built here**, or **Proposed**.

## 1. Audit of the existing `.expkg` implementation

| Property | Finding | Evidence |
|---|---|---|
| Container | `EXPKG1\n`, `file <size> <path>\n`…, `data\n`, concatenated bytes. Regular files only. | `expkg.h` (S) |
| Hardening | Path validation (no `..`, absolute, `\`, duplicates incl. case-folded, file/dir conflicts), entry and size limits, no symlinks on pack/extract, extraction confined to destination | `SECURITY.md`, `test_expkg_backend.cpp` (T — ExileStore suite passes in this environment) |
| Metadata | **None in the package**: no manifest, no permissions, no signature. Name, version, permissions, signature metadata live in the *catalog* | `model.h`, `catalog.cpp` (S) |
| Authenticity | Ed25519 over `appId\|version\|package id\|sha256\|size`; key must be in `trusted-keys.json`, not revoked, authorised for that publisher | `trust.*`, `test_trust.cpp` (T) |
| Permissions | 13 coarse ids, displayed only; unknown ids are shown as "not recognised"; **nothing enforces them** | `render.cpp`, `SECURITY.md` limit 1 (S/T) |
| Status | Declared a **development** format; production format is "a separate integration point" and no second extension may be introduced before spec, installer contract, signature policy and migration plan are agreed | `Documentation/ExileFileFormats.md` §5 |

Decision: `.expkg` stays the development container. This work adds **no new extension and no format change**.

## 2. Trust model

Principals and what each is trusted for:

| Party | Trusted for | Not trusted for |
|---|---|---|
| Publisher key (in trust store) | Authoring a package for its authorised `publisherId` | Safety of the code; permissions beyond what it declared and the user granted |
| Catalog | Discovery metadata | Anything security-relevant unless signed (it is **not** signed today — `SECURITY.md` limit 4) |
| Store GUI | Presenting intent | Installing, verifying, or granting (must stay unprivileged) |
| Installer service (**Proposed**, privileged) | Verification, unpacking, receipt | — |
| Broker | Deciding every runtime permission question | — |
| User | Approving grants | — |
| The package file / its extension | **Nothing.** Never decides whether code runs | — |

**Execution is governed by system state, not file names.** A program may run as an app only if all hold (Proposed launcher rule): (1) the installer's own database has a verified receipt for that exact app+version+digest; (2) a broker principal exists whose declared set came from that verified manifest; (3) the launcher projects only broker grants onto pledge/unveil (`kernel_map`). Opening an `.expkg`, or any file with any extension, runs nothing.

## 3. Manifest and permission declaration (Proposed format; Built here: grammar + digest + signing payload)

Permission grammar (**Built**, `permission.*`): `<domain>.<action>[:<scope>]`, closed vocabulary of 16 permissions, strict scope syntax (absolute normalised paths or `$symbolic` roots; `host:port` with optional `*.suffix`; one named program for `proc.spawn`; settings key). Anything else is rejected, never guessed.

Manifest (**Proposed**, would be a file `exile-manifest.json` inside a production package, parsed by the installer before anything else):
```
{ "schema": 1, "appId": "...", "version": "...", "publisher": "...",
  "permissions": ["fs.read:$documents", "net.connect:api.example.com:443", "ui.notify"],
  "entrypoint": "bin/app" }
```
Rules: permissions are the **declared ceiling**; the broker can never grant outside it (**Built**, T). Unknown permission strings fail verification (**Built** in the grammar; not yet called from the installer).

Binding to the signature (**Built as a function, not wired**): `signingPayloadV2` = v1 fields + `permissions-sha256:<digest of the canonical sorted set>` under a distinct prefix `exile-package-signature-v2`, so a signature cannot be replayed with a different permission set, and a v2 payload can never be confused with a v1 one. Wiring requires changing the catalog schema, `trust.cpp`, `installer.cpp` and re-signing sample packages; that was **not** done because it alters existing verified behaviour and needs review.

Legacy catalogs: `mapLegacyIds` converts the 13 Store ids to canonical permissions. Notably `network` → `net.connect:*:*` and `system-settings` → `sys.settings:*` are **broad**, so the broker never grants them standing to an agent. Unknown ids are returned as `unmapped`, never dropped silently (T).

## 4. Verification pipeline (target)

1. Download to a location only the installer can write (fixes TOCTOU — **Proposed**; today the cache is user-writable).
2. Size + SHA-256 (**Exists**).
3. Signature v2 over payload including permission digest; key trusted, unrevoked, publisher-authorised (**Exists** for v1; v2 **Built as function**).
4. Parse manifest; every permission valid under the grammar; manifest `appId`/`version` equal the signed ones (**Proposed**).
5. Compatibility/dependencies/space (**Exists**).
6. Install with receipt; success only after backend confirmation (**Exists**).
7. Register broker principal with the declared set (**Built** API: `registerPrincipal`).

## 5. Install, update, removal, rollback

| Operation | Safe behaviour | Exists | Missing |
|---|---|---|---|
| Install | Pipeline above; no permission is granted at install, only declared; grants happen at first use through approval | Pipeline, staging dir, atomic swap | Privileged installer; manifest; broker registration wiring |
| Update | Never downgrade (**Exists**). Compute `old.escalationTo(new)`; if non-empty, the update installs but new permissions are **ungranted** and require user approval (**Built**: `PermissionSet::escalationTo`, T). Narrowed declarations immediately revoke now-uncovered grants (**Built**: `updateDeclared`, T) | No-downgrade; atomic swap | Wiring; user-facing escalation prompt |
| Removal | `revokeAll(principal)` then remove files; data retention is an explicit user choice | `uninstallApp` (dependency check) | Broker call; data policy |
| Rollback | A failed activation restores `previous` (**Exists**, `backend.cpp`); crash between renames recovered by `recoverInterrupted` (**Exists**). A user-initiated rollback must re-run `updateDeclared` with the old set | Atomic swap + `previous` | **Rollback protection** (a signed catalog with minimum version/freshness; `SECURITY.md` limit 4) so a downgrade attack cannot reinstall a vulnerable signed version |

## 6. Blocked or not done

- No wiring of v2 payload / manifest into the Store (needs review of verified behaviour).
- Privileged installer, signed catalog, TLS transport, protected root of trust (`SECURITY.md` limits 2, 4–6).
- Enforcement of declared permissions at run time (needs a booted kernel; see README §4).
