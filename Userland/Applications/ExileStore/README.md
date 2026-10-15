# Exile Store

The application marketplace for the Exile ecosystem, written in C++20.

> **Status: development build, not production-secure.** Everything described as "working" below runs and is tested. Everything else is clearly listed as a gap in [`docs/INTEGRATION.md`](docs/INTEGRATION.md) and [`docs/SECURITY.md`](docs/SECURITY.md). Please read those before treating an install as safe.

*Make installing software as easy as finding a song.*

## What it is

A store that lets a user open it, find an app, see whether it works on their device, understand its permissions and publisher, install it, and keep it updated, without knowing about package formats or the command line.

This repository contains:

| Part | What it does |
|---|---|
| `src/core/` | UI-independent store logic: catalog model + validation, compatibility resolver, search, trust/signature verification, package transport, install pipeline, local library, update detection |
| `src/ui/` | Pure renderers (state in, text out), responsive layout (sidebar vs bottom navigation), and a navigation controller |
| `src/main.cpp` | Terminal front-end and CLI (`exile-store`) |
| `tools/expkg_tool.cpp` | Developer tool: build `.expkg` dev packages, sign, generate the sample catalog |
| `tests/` | 153 unit tests + a CLI smoke test |
| `data/` | A small **clearly-labelled sample catalog** (signed, development only) |

The core library has no UI dependency, so a graphical front-end for Exile OS (or the future mobile/tablet/TV stores) can reuse it unchanged.

## Build and test

Requirements: a C++20 compiler, CMake 3.16+, OpenSSL (for Ed25519 verification; without it the store builds but **refuses all installs**).

```sh
cmake -S . -B build
cmake --build build -j
cd build && ctest --output-on-failure
```

Regenerate the sample catalog (new throw-away signing key each time):

```sh
./build/expkg-tool devcatalog data     # writes data/catalog and data/trust
```

## Try it

```sh
./build/exile-store --catalog data/catalog/catalog.json            # interactive store
./build/exile-store --catalog data/catalog/catalog.json status     # environment + security posture
./build/exile-store --catalog data/catalog/catalog.json search music
./build/exile-store --catalog data/catalog/catalog.json install org.exile.sample.notes
./build/exile-store --catalog data/catalog/catalog.json --device mobile --arch aarch64 home
```

Inside the interactive store: `h` home, `e` explore, `l` library, `/ words` search, a number opens an item, then `i` install, `u` update, `x` uninstall, `b` back, `?` help, `q` quit. `--color` enables the black/white/green/blue theme; `--width N` (40 to 400) exercises the responsive layouts.

The sample apps are development fixtures that show every state: compatible, phone-only (browsable but **not installable** on desktop, with an explanation), needs-newer-OS, beta-only (hidden unless `--beta`), web app, compatibility-layer app, dependency chain, and a package signed by an untrusted key (refused).

State lives in `~/.local/share/exile-store` (override with `--data-dir`).

## Documents

- [`docs/AUDIT.md`](docs/AUDIT.md): repository audit findings (and why it is what it is)
- [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md): layers, data model, install pipeline, multi-device rules
- [`docs/SECURITY.md`](docs/SECURITY.md): what is enforced, what is not, known limitations
- [`docs/INTEGRATION.md`](docs/INTEGRATION.md): missing Exile OS services, the interfaces to plug them into, proposed catalog backend, mobile/tablet/TV extension points
- [`NOTICE`](NOTICE): attribution and licensing status
