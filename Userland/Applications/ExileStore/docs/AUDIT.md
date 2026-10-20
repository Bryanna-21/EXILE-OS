# Repository audit

> **Superseded in part.** The findings below describe the state when the Store was first written, before it was
> merged into the Exile OS repository. It is now part of that repository (`Userland/Applications/ExileStore`),
> built natively through `CMakeLists.txt` and, standalone, through `CMakeLists.host.cmake`. The repository audit
> is kept in `Documentation/Project-Audits/`. The integration gaps listed in `INTEGRATION.md` and `SECURITY.md`
> (privileged installer, enforced permissions, HTTPS transport, catalog freshness, signing-key rotation, rollback
> protection) are still open. The original text follows unchanged.

## Findings

The brief says to inspect the existing Exile OS repository first. **There was no repository to inspect.**

| Item | Finding |
|---|---|
| Repository root / branch / commit / working tree | None. The workspace (`/home/claude`) was not a git repository (`git rev-parse` failed) and contained only tool caches and dotfiles. |
| README, contribution guide, licenses, third-party notices | None present. |
| Serenity-derived sources, GUI toolkit (LibGUI/LibGfx), `AK`, `LibCore`, `LibIPC` | **Not present**, so none of it was inspected, linked, or reused. |
| Existing package manager, signature verification, dependency resolver, update service | Not present / not inspectable. |
| Existing apps, settings UI, icons, themes, app metadata formats | Not present. |
| Prior Exile Store prototype | None found. |
| Build/test commands of the project | None to inherit. |

Toolchain available in the workspace: GCC 13.3, CMake 3.28, OpenSSL 3.0.13 (headers), no libcurl headers, clang-format present.

## What that means for the work

The brief says not to invent APIs that don't exist and to reuse project conventions. With no project to follow, I did the following instead of guessing at Serenity internals:

1. Built Exile Store as a **self-contained C++20/CMake project** with the only dependency being OpenSSL (for Ed25519).
2. Kept all store logic behind **small interfaces** (`IPackageBackend`, `ITransport`, `ICatalogSource`, `ISignatureVerifier`) so the real Exile OS services can be plugged in without touching the logic.
3. Wrote a **text front-end** driven by pure renderers and a navigation controller. A LibGUI front-end should replace only the I/O loop in `main.cpp`.
4. Did **not** claim integration with anything that does not exist here. The missing pieces are enumerated in `INTEGRATION.md`.
5. No upstream code was copied, so there is no upstream attribution to preserve. See `NOTICE`.

## What needs a human with the real tree

- Drop `src/core` in as a library (it is dependency-light and should port to `AK` types with mechanical changes if the project wants that; I did not guess at it).
- Decide the repository's formatting rules; the code here uses 4-space indent and ~120 columns but no `.clang-format` is enforced because there is no house style to match.
- Choose the license (see `NOTICE`).
- Re-do this audit against the real repository: find the real installer, signature infrastructure and GUI toolkit, then implement the adapters listed in `INTEGRATION.md`.
