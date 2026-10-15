# Architecture

```
        +-----------------------------------------------------------+
        |  Presentation   src/ui/        render.* controller.* text.*|   <- replaceable by a GUI
        +----------------------------+------------------------------+
                                     |  (state in, pages/commands out)
        +----------------------------v------------------------------+
        |  Store logic    src/core/                                  |
        |   catalog + validation   search   compatibility   appstate |
        |   installer (pipeline)   library  trust   transport        |
        +--------+-----------+-----------+-------------+-------------+
                 |           |           |             |
           ICatalogSource  ITransport  ISignatureVerifier  IPackageBackend
           (file today)    (local;     (OpenSSL Ed25519)   (dev prefix;
                           https gap)                       OS service = gap)
```

Responsibilities map one-to-one onto the brief's list:

| Responsibility | Code |
|---|---|
| Presentation layer | `ui/render.*`, `ui/controller.*`, `ui/text.*`, `main.cpp` |
| Catalog client | `core/catalogclient.*` (offline fallback, cache never poisoned by bad data) |
| Search and filtering | `core/search.*` |
| Compatibility resolver | `core/device.*` |
| Package manager adapter | `core/backend.h` (`IPackageBackend`), `LocalPrefixBackend` |
| Trust and verification | `core/trust.*` |
| Local library | `core/library.*` |
| Update service | `core/appstate.*` (`findUpdates`) |
| Device capability provider | `core/device.*` (`DeviceProfile`, `detectHostDevice`) |
| Optional remote catalog | proposed in `INTEGRATION.md`; the store works with a local file today |

## Data model

`core/model.h`. **App identity is separate from builds**: an `AppEntry` (name, publisher, categories, permissions, release notes, ...) owns one or more `PackageBuild`s, each with its own device classes, architectures, minimum OS, `kind` (`native` / `compat-layer` / `web`), URL, SHA-256, sizes, signature metadata, channel, dependencies, and withdrawn flag. Required fields from the brief (`appId`, `supportedDevices`, `supportedArchitectures`, `minimumOsVersion`, `packages`, ...) are present; `supportedDevices` etc. are derived views over the builds.

Catalog metadata is **untrusted input**: `core/catalog.cpp` validates every field (identifier grammar, version syntax, digest format, URL scheme, control characters, size limits, rating consistency). A malformed document yields no catalog; a malformed app is skipped and reported while the rest still load. Ratings exist only if the catalog supplies a real average and a positive count; the store never fabricates ratings, download counts or reviews, and has no popularity sort for that reason.

## Compatibility

`resolveCompatibility(app, device)` picks the best build (native > compatibility layer > web; exact architecture before `any`) or returns a **specific, user-facing reason**: wrong device class (naming the supported ones), wrong processor, OS too old (with both versions), channel not enabled, release withdrawn (with the publisher's reason). The UI never shows an incompatible app as installable; the installer refuses it too, so the rule does not depend on the UI.

## Install pipeline

`core/installer.*`. Each stage is an explicit state with enforced transitions (`isValidTransition`):

```
Queued -> CheckingCompatibility -> CheckingDependencies -> CheckingSpace -> Downloading
       -> Verifying -> Installing -> Succeeded          (any stage -> Failed / Cancelled,
                                                         except Installing -> Cancelled)
```

1. **Compatibility** (and install-vs-update validity; updates never downgrade).
2. **Dependencies** checked against installed apps with minimum versions. Missing ones are reported by name; nothing is auto-installed behind the user's back.
3. **Storage** check with a 5% margin before any download.
4. **Download** to a content-addressed `<sha256>.part` file. Interrupted/cancelled transfers keep the partial file and **resume**; a stale oversized partial is discarded.
5. **Integrity**: size and SHA-256. A corrupt *resumed* file is deleted and retried once from scratch; a corrupt fresh download fails and is deleted.
6. **Trust**: Ed25519 signature over a canonical payload binding appId, version, package id, digest and size, verified against the separate trust store, with revocation and a per-key publisher restriction.
7. **Install** through `IPackageBackend`. Success is claimed **only after the backend's own `isInstalled` confirms it**; only then is the library updated.

Cancellation is cooperative and safe: allowed until the backend starts working. Installs run on a worker thread (`runAsync`) with a thread-safe progress snapshot so a UI never blocks.

## Local development backend

`LocalPrefixBackend` unpacks `.expkg` files into a user-owned prefix with: path validation before anything is written, symlink refusal, a staging directory, atomic swap with a `previous` rollback copy, crash recovery (`recoverInterrupted`), and a reserved receipt file that packages cannot forge. It runs unprivileged and is a stand-in for the real system installer (see `INTEGRATION.md`).

## Multi-device design

- One catalog, per-build device/architecture targeting, so a shared app with separate desktop and mobile builds is a single listing (see `org.exile.sample.player`).
- The device is a runtime `DeviceProfile` (class, architecture, OS version, enabled channels). `--device mobile --arch aarch64` demonstrates a phone's view of the same catalog with the same code.
- Layout is chosen by width: **sidebar** at 72+ columns, **bottom navigation bar** below it; all content wraps to the available width (tested from 40 to 140 columns, no overflow).
- Resizing a terminal is **not** a claim of mobile support. Real mobile support needs a mobile OS build, drivers, packages for that architecture, input services, and a touch GUI front-end, none of which exist in this repository.
- Keyboard-first on desktop (number selection, single-key commands, `/` search). Touch targets belong to the future GUI front-end; the controller's command set (`Install`, `Update`, `Uninstall`, ...) is input-agnostic.

## Visual identity

Black/white base, green for primary actions and success, blue for headings. No purple. Color is opt-in (`--color`) and a test asserts no purple/magenta escape codes are ever emitted.
