# Integration points and missing system services

Nothing below is faked in the store. Where a service is missing the store refuses the operation or says so.

## 1. Services Exile OS must provide

| Needed service | Store-side interface | Current stand-in | Effect of the gap |
|---|---|---|---|
| Privileged package installer (authorised, isolated from the GUI) | `IPackageBackend` (`install`, `uninstall`, `isInstalled`, `launch`, `storagePath`, `recoverInterrupted`) | `LocalPrefixBackend` (unprivileged dev prefix) | Not a real OS install; installed files are not registered with the OS |
| App launcher / process service | `IPackageBackend::launch`, `canLaunch()` | none (`canLaunch()` is false) | The store shows **Installed**, never a fake **Open** button |
| Package format + metadata | `.expkg` dev container (`core/expkg.*`) | dev format only | Replace with the OS's real package format in the backend |
| Sandboxing and permission enforcement | Permission ids in the catalog (`Permission`) | display only | Permissions are explained to the user but not enforced |
| Signature infrastructure / root of trust | `ISignatureVerifier`, `TrustStore` | OpenSSL Ed25519 + local trust file | No protected root of trust, rotation or revocation feed |
| TLS download service | `ITransport` | local files only | Remote catalogs/packages cannot be fetched |
| Device capability API | `DeviceProfile` | compiler-derived architecture; class/OS version are defaults/flags | Needs real device class, OS version and channel settings |

To integrate: implement the interface, pass it through `InstallerEnv` (see `App::env()` in `main.cpp`). The pipeline, UI and tests do not change. The `ScriptedBackend` and `FlakyTransport` doubles in `tests/testing.h` are a ready-made harness for the new adapters.

## 2. Proposed remote catalog service (not built)

Keep it a static-friendly, signed-metadata design so it can be hosted anywhere and mirrored:

- `GET /v1/catalog.json`: the document format already parsed by `core/catalog.*` (schema version 1), **plus** (to add) a detached signature `catalog.json.sig`, a monotonically increasing `sequence`, and an `expires` timestamp so clients can reject stale or rolled-back catalogs (closes limitation 4 in `SECURITY.md`).
- `GET /v1/packages/<appId>/<version>/<buildId>.pkg`: immutable, content-addressed; the SHA-256 and size in the catalog are signed by the publisher.
- **Publisher identity**: publisher ids map to one or more public keys in the trust root; keys are scoped to a publisher (already enforced client-side).
- **Release channels**: `stable` / `beta` / `nightly` per build (already modelled and enforced).
- **Withdrawal**: `withdrawn` + `withdrawnReason` per build (already modelled; honoured by compatibility and installer).
- **Update discovery**: clients diff `currentVersion`/per-build version against the library (`findUpdates`).
- No account, social, payment or review platform is required for any of this, and none is built.

## 3. Multi-device extension points

- **Catalog**: unchanged; add `device` values or architectures to the enums in `core/model.*` and the parsers pick them up.
- **Phones/tablets/TV**: write a front-end that owns input and drawing and calls the same controller commands and renderers' data (`computeStatus`, `searchCatalog`, `findUpdates`, `InstallJob`). Bottom navigation is already the layout used below 72 columns; the destinations (Home, Explore, Search, Library, App Details) are the same on every device.
- **Touch**: the controller exposes semantic commands, not key codes; targets and gestures belong in the GUI layer.
- **Shared apps**: model one app with several builds (see `org.exile.sample.player`).
- **Universal Launcher**: the store consumes only `DeviceProfile` and the catalog; it needs no launcher API except `IPackageBackend::launch`.

## 4. Suggested next integration steps

1. Re-run the repository audit against the real Exile OS tree and map `IPackageBackend` onto its installer service (privileged, with TOCTOU-safe hand-off of verified bytes).
2. Plug in an HTTPS `ITransport` and make the catalog signed with expiry.
3. Port the front-end to the real GUI toolkit using `src/ui` as the state/logic layer.
4. Replace the dev trust file with the OS root of trust.
