# REMOVAL_CANDIDATES.md (PROPOSED DRAFT; nothing was deleted)

> **Status update (branch `fix/boot-integrity-and-ci`, based on `72c3049`; changes uncommitted pending review).**
> This audit was written against `18a3c70`. Verified since then, by running code on a development host: the boot guardian now implements Ed25519-authenticated verification (12 unit tests, 24 tamper checks pass); the Store host build passes 2/2 tests; the repo style checks pass. `.tmp/` is no longer tracked. **Not verified:** any full-OS build, QEMU boot, CI result, or the prekernel target compile. Verified boot is opt-in and not proven end to end; see `Documentation/BootIntegrity.md`. Items below not mentioned here remain proposals.

Evidence: tracked-file listing, `git grep` for consumers, file-type inspection. "Consumers" = other tracked files referencing the path.

| Candidate | Tracked | Generated | Consumers | CI/release dependency | Risk if removed | Verdict |
|---|---|---|---|---|---|---|
| `.tmp/` (22 files: 3 Windows PE32+ `.exe`, COFF `.o`, `gcc*-temp-wrapper.sh`, `ninja-test/`, `test-kernel.bin/.manifest`, `win-temp/direct.exe`) | Yes (not in `.gitignore`) | Yes, Windows/MSYS2 compiler experiments | None found outside `.tmp/` itself | None found | Wrapper scripts hard-code `C:/Users/USER/Downloads/EXILE-OS/.tmp/win-temp`; losing them loses only the experiments | **Safe after your confirmation**, then add `.tmp/` to `.gitignore` |
| `.github/workflows/dev-container.yml.disabled`, `manpages.yml.disabled`, `wasm.yml.disabled`, `Meta/toot-commits.js.disabled` | Yes | No | None | Disabled | Lose upstream reference | **Conditional**: keep until Exile decides whether to revive docs/wasm CI |
| `.github/workflows/discord.yml`, `social-media.yml`, `nightly.yml` | Yes | No | None | Gated on `refs/heads/master`; default branch is `main`, so they never fire (nightly shows "Skipped") | Upstream notification plumbing needing `DISCORD_WEBHOOK`/`MASTODON_ACCESS_TOKEN` secrets | **Conditional**: fix the branch name or drop; nightly is worth keeping once fixed |
| `.github/CODEOWNERS` | Yes | No | GitHub review routing | Active | Lists upstream individuals (@timschumi, @alimpfard, @trflynn89, @GMTA, @BertalanD) who do not own this repo | **Not safe as-is**: replace owners, do not just delete |
| `Ladybird/` (127 files) | Yes | No | `Documentation/BuildInstructionsLadybird.md`, Lagom CMake | Lagom CI jobs | Host browser frontends; unrelated to OS images | **Conditional / optional**; keep until product scope decided |
| `Ports/` (1,063 files, 367 recipes) | Yes | No | Image build optional | Not required for core CI | Large optional surface, no license metadata in recipes | **Not recommended** to remove blindly; prune per-port later |
| `Toolchain/Stubs/*/*.so` (12 files) | Yes | Stubs | Cross-build | Needed for toolchain bootstrap | Breaks build | **Not recommended** |
| `Userland/Libraries/LibCompress/BrotliDictionary.cpp.dict.bin` | Yes | Data | LibCompress | Core | Breaks Brotli | **Not recommended** |
| `Base/home/anon/Documents/3D Models/*.obj`, `zip/archive.zip`, `Videos/test-webm.webm`, big test images | Yes | No | Demo/test payload | Some tests/apps | Minor | **Conditional**; trim only with a review of tests |
| `Userland/Applications/ExileStore/docs/AUDIT.md` | Yes | No | None | None | Stale: claims no repo existed | **Rewrite**, do not delete |
| `Userland/Applications/ExileStore/NOTICE` | Yes | No | None | None | Says "no license has been chosen" while `native_main.cpp` carries `BSD-2-Clause` | **Fix**, do not delete |
| Leftover `SERENITY_*` identifiers (`SERENITY_QEMU_CPU` in `Meta/run.py`, `x86_64-pc-serenity` triple, docs) | Yes | No | Scripts/docs | CI behaviour | Renaming blindly can break toolchain triple | **Migrate deliberately** (see below) |

## Not a candidate: a false alarm worth noting
`"Base/res/fonts/CJKBiángRegular36.font"` shows quoted in `git ls-files` only because of non-ASCII path quoting (`core.quotepath`). It is a real upstream font, not a stray file.

## Rename drift that can break behaviour (fix, not delete)
- `Meta/run.py` reads `SERENITY_QEMU_CPU`, but CI sets `EXILE_QEMU_CPU="max,vmx=off"`: the CI QEMU CPU setting is silently ignored.
- CI coverage jobs set `SERENITY_DISK_SIZE_BYTES`, but `Meta/build-image-qemu.sh` reads `EXILE_DISK_SIZE_BYTES`.
- `Documentation/RunningTests.md` and `RunningOnRaspberryPi.md` still document `SERENITY_RUN` / `SERENITY_KERNEL_CMDLINE`; scripts use `EXILE_*`.
- README Store build command `cmake -S . -B build` now configures the native target; the standalone host build is `CMakeLists.host.cmake`.
