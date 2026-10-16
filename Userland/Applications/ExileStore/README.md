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
| `src/app/` | `Engine`: thread-safe wrapper around the core (immutable snapshots + one install worker) |
| `src/server/` | Loopback HTTP server, JSON API and request-security checks for the web UI |
| `web/` | **The universal UI**: one responsive page (phone, tablet, desktop, TV) compiled into the binary |
| `src/main.cpp` | Terminal front-end and CLI (`exile-store`), plus `serve` for the web UI |
| `tools/expkg_tool.cpp` | Developer tool: build `.expkg` dev packages, sign, generate the sample catalog |
| `tests/` | 176 unit tests + a CLI smoke test; browser-driven checks were run by hand (see below) |
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

## The universal UI (phones, tablets, PCs, TVs and anything with a browser)

```sh
./build/exile-store --catalog data/catalog/catalog.json serve        # prints a link; default port 7878
./build/exile-store --device tv --arch aarch64 serve                 # a TV's view of the same catalog
```

Open the printed `http://127.0.0.1:PORT/#t=...` link in any browser or web view **on the same device**. One page adapts itself:

| Device | Layout | Input |
|---|---|---|
| Phone (< 600 px) | bottom navigation, single column | touch, 48 px+ targets |
| Tablet (600 to 1023 px) | left rail, two or three columns | touch |
| PC (1024 px and up) | labelled sidebar, grid | mouse and keyboard (`/` searches) |
| TV (device class `tv`, a TV browser, or `#t=...&ui=tv`) | large type, dark, labelled sidebar | D-pad / remote: arrow keys move focus spatially, Enter selects, Back/Esc returns |

Same four destinations everywhere (Home, Explore, Search, Library) plus the app page with one obvious action. Apps that cannot run on the device are listed but marked, and never offer Install. A catalog build whose `devices` is `["any"]` is a **universal app** (shown as "Works on all devices"); it still has to pass the architecture, OS, channel and signature checks.

How "universal" works here: the store is one executable. The UI is HTML/CSS/JS compiled into it, so any OS that has a browser or web view can show it; the native pieces (catalog, trust, installer) stay in C++. A device whose OS has no browser or web view cannot show this UI, and a shell for it (a launcher that opens the link in a web view) does not exist in this repository.

Limits you should know: the service listens on **127.0.0.1 only**. It is *not* a way to browse the store from another device on your network, on purpose. Opening it from a phone while it runs on a PC needs a deliberate, authenticated remote mode that is not built. See `docs/SECURITY.md`, "Local web service".

## Look and feel

- **Icon**: an original "X" mark (blue bar under a green arrow) in `branding/`; see `branding/README.md` for files and usage rules.
- **Loading**: first load of a tab shows a short branded splash where the mark draws itself (about 0.9 s, once per tab, skipped when the user prefers reduced motion). After that every screen shows layout-matched skeleton placeholders instead of a spinner, new content eases in, installs show four named steps (Check, Download, Verify, Install) with a progress bar, and a finished install draws a check mark. All motion is replaced by static states under `prefers-reduced-motion`.

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
