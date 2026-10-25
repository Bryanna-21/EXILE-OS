# Exile OS — Application Audit (read-only)

Status: **proposed draft**. Does not overwrite the six existing documents in this folder. No source change in the OS tree was committed; one optional patch is supplied separately (`patches/`).

## 0. Method and honesty statement

* Evidence = source reading, grep, a mechanical inventory script (`inv.py` → 86 app-like targets), and a **host-only** build of the Exile Store core (configure OK, 0 warnings, ctest 2/2).
* **NOT done:** full OS cross-build, image creation, QEMU boot, running any GUI app, the native Exile Store target, the Jakt Weather app, hardware. Therefore **no app is "functionally verified"** in the sense of the audit prompt. Every status below is one of: *source-verified defect*, *suspected*, or *inventory-level only*.
* Deep reads were done for about 15 apps (listed in §3). The rest are **inventory-level only; deeper review pending** (sub-agent deep audits failed with rate-limit 429 twice).
* Scope of facts: repo `Bryanna-21/EXILE-OS` anonymous clone; nothing pushed.

## 1. Application inventory

Registered in `Userland/Applications/CMakeLists.txt`: Applications 53, Games 13, Demos 11, DevTools 4, Applets 6 (87 entries; the script found 86 target rows). Launcher metadata: 72 `.af` files in `Base/res/apps` (categories: Demos 8, Utilities 13, Games 13, Settings 13, Development 8, Office 4, Media 3, Graphics 3, Demos/Screensaver 3, Internet 4). Apps without `.af`: About, CrashReporter, Debugger, Escalator, ExileStore, Run, Settings, Welcome, plus applets (Audio, ClipboardHistory, Keymap, Network, ResourceGraph, WorkspacePicker). Most are expected exceptions; **ExileStore is not**.

Columns: *Src* present in source; *Reg* registered in CMake; *.af* launcher entry; *Comp* has a build-component option. "Built / in image / launchable / verified" = **not verified** for all rows.

| App | Root | Reg | .af | LOC | TODO | pledge/unveil | Icon(s) |
|---|---|---|---|---|---|---|---|
| Audio | Applets | Y | N | 259 | 0 | 2/3 |  |
| ClipboardHistory | Applets | Y | N | 420 | 0 | 2/3 |  |
| Keymap | Applets | Y | N | 202 | 0 | 2/0 |  |
| Network | Applets | Y | N | 183 | 0 | 1/5 |  |
| ResourceGraph | Applets | Y | N | 305 | 0 | 2/6 |  |
| WorkspacePicker | Applets | Y | N | 251 | 0 | 2/3 |  |
| 3DFileViewer | Applications | Y | Y | 948 | 0 | 1/4 | app-3d-file-viewer |
| About | Applications | Y | N | 26 | 0 | 2/2 | ladyball |
| AnalogClock | Applications | Y | Y | 316 | 0 | 1/3 | app-analog-clock |
| Assistant | Applications | Y | Y | 754 | 1 | 1/0 | app-assistant |
| Browser | Applications | Y | Y | 4962 | 13 | 2/19 | app-browser |
| BrowserSettings | Applications | Y | Y | 1011 | 2 | 1/5 | app-browser |
| Calculator | Applications | Y | Y | 1238 | 2 | 2/2 | app-calculator |
| Calendar | Applications | Y | Y | 1522 | 1 | 2/6 | app-calendar |
| CalendarSettings | Applications | Y | Y | 270 | 0 | 2/2 | app-calendar |
| CertificateSettings | Applications | Y | Y | 345 | 0 | 1/6 | certificate |
| CharacterMap | Applications | Y | Y | 579 | 2 | 2/2 | app-character-map |
| ClockSettings | Applications | Y | Y | 549 | 1 | 2/4 | app-analog-clock |
| CrashReporter | Applications | Y | N | 507 | 0 | 1/0 | app-crash-reporter |
| Debugger | Applications | Y | N | 445 | 2 | 1/0 |  |
| DisplaySettings | Applications | Y | Y | 2373 | 1 | 1/0 | app-display-settings |
| Escalator | Applications | Y | N | 338 | 1 | 3/0 | app-escalator |
| ExileStore | Applications | Y | N | 12982 | 0 | 1/3 | app-help |
| FileManager | Applications | Y | Y | 4753 | 9 | 2/0 | app-file-manager |
| FontEditor | Applications | Y | Y | 2751 | 1 | 1/3 | app-font-editor |
| GamesSettings | Applications | Y | Y | 810 | 0 | 1/5 | games |
| Help | Applications | Y | Y | 836 | 1 | 1/6 | app-help |
| HexEditor | Applications | Y | Y | 4078 | 13 | 1/3 | app-hex-editor |
| ImageViewer | Applications | Y | Y | 984 | 4 | 1/6 | app-image-viewer |
| KeyboardMapper | Applications | Y | Y | 679 | 0 | 2/0 | app-keyboard-mapper |
| KeyboardSettings | Applications | Y | Y | 643 | 0 | 2/7 | app-keyboard-settings |
| Magnifier | Applications | Y | Y | 423 | 0 | 1/3 | app-magnifier |
| Mail | Applications | Y | Y | 1218 | 9 | 1/6 | app-mail |
| MailSettings | Applications | Y | Y | 291 | 0 | 2/2 | app-mail |
| Maps | Applications | Y | Y | 1076 | 2 | 1/6 | app-maps |
| MapsSettings | Applications | Y | Y | 243 | 0 | 1/3 | app-maps |
| MouseSettings | Applications | Y | Y | 1105 | 3 | 2/0 | app-mouse |
| NetworkSettings | Applications | Y | Y | 399 | 1 | 2/7 | network |
| PDFViewer | Applications | Y | Y | 1604 | 0 | 1/3 | app-pdf-viewer |
| PartitionEditor | Applications | Y | Y | 273 | 2 | 2/6 | app-partition-editor |
| Piano | Applications | Y | Y | 2534 | 12 | 1/0 | app-piano |
| PixelPaint | Applications | Y | Y | 16302 | 22 | 1/7 | app-pixel-paint |
| Presenter | Applications | Y | Y | 936 | 5 | 1/0 | app-presenter |
| Run | Applications | Y | N | 379 | 3 | 1/0 | app-run |
| Screenshot | Applications | Y | Y | 267 | 1 | 1/0 | app-screenshot |
| Settings | Applications | Y | N | 147 | 0 | 2/0 | app-settings |
| SoundPlayer | Applications | Y | Y | 1912 | 11 | 1/0 | app-sound-player |
| SpaceAnalyzer | Applications | Y | Y | 1060 | 2 | 0/0 | app-space-analyzer |
| Spreadsheet | Applications | Y | Y | 8088 | 17 | 1/6 | app-spreadsheet |
| SystemMonitor | Applications | Y | Y | 2996 | 6 | 1/15 | app-system-monitor |
| Terminal | Applications | Y | Y | 468 | 0 | 2/10 | app-terminal |
| TerminalSettings | Applications | Y | Y | 705 | 0 | 2/2 | app-terminal |
| TextEditor | Applications | Y | Y | 1460 | 2 | 1/5 | app-text-editor |
| ThemeEditor | Applications | Y | Y | 1391 | 0 | 2/0 | app-theme-editor |
| UsersSettings | Applications | Y | Y | 932 | 0 | 1/5 | app-user-settings |
| VideoPlayer | Applications | Y | Y | 815 | 1 | 0/3 | app-video-player |
| Weather | Applications | Y | Y | 260 | 0 | 0/0 | app-weather |
| Welcome | Applications | Y | N | 336 | 0 | 1/7 | app-welcome |
| CatDog | Demos | Y | Y | 536 | 1 | 2/4 | app-catdog |
| Eyes | Demos | Y | Y | 262 | 0 | 1/3 | app-eyes |
| Gradient | Demos | Y | Y | 111 | 0 | 2/2 | app-gradient |
| LibGfxDemo | Demos | Y | Y | 213 | 0 | 1/2 | app-libgfx-demo |
| LibGfxScaleDemo | Demos | Y | Y | 133 | 0 | 1/2 | app-libgfx-demo |
| Mandelbrot | Demos | Y | Y | 523 | 1 | 1/3 | app-mandelbrot |
| ModelGallery | Demos | Y | Y | 302 | 1 | 2/0 | app-model-gallery |
| Screensaver | Demos | Y | Y | 111 | 0 | 2/0 | app-screensaver |
| Starfield | Demos | Y | Y | 185 | 0 | 2/0 | app-starfield |
| Tubes | Demos | Y | Y | 505 | 0 | 2/0 | app-tubes |
| WidgetGallery | Demos | Y | Y | 1364 | 1 | 1/4 | app-widget-gallery |
| GMLPlayground | DevTools | Y | Y | 507 | 1 | 1/4 | app-gml-playground |
| HackStudio | DevTools | Y | Y | 10376 | 13 | 5/4 | app-hack-studio |
| Profiler | DevTools | Y | Y | 4370 | 7 | 0/0 | app-profiler |
| SQLStudio | DevTools | Y | Y | 864 | 0 | 0/0 | app-sql-studio |
| BrickGame | Games | Y | Y | 855 | 2 | 2/3 | app-brickgame |
| Chess | Games | Y | Y | 2050 | 7 | 1/7 | app-chess |
| ColorLines | Games | Y | Y | 1076 | 0 | 2/3 | app-colorlines |
| FlappyBug | Games | Y | Y | 424 | 0 | 2/3 | app-flappybug |
| Flood | Games | Y | Y | 749 | 1 | 2/3 | app-flood |
| GameOfLife | Games | Y | Y | 1072 | 0 | 2/3 | app-gameoflife |
| Hearts | Games | Y | Y | 1697 | 2 | 2/4 | app-hearts |
| MasterWord | Games | Y | Y | 603 | 1 | 2/4 | app-masterword |
| Minesweeper | Games | Y | Y | 1192 | 1 | 2/3 | app-minesweeper |
| Snake | Games | Y | Y | 984 | 0 | 2/3 | app-snake |
| Solitaire | Games | Y | Y | 1294 | 0 | 2/3 | app-solitaire |
| Spider | Games | Y | Y | 958 | 0 | 2/3 | app-spider |
| TwentyFourtyEight | Games | Y | Y | 1218 | 0 | 2/3 | app-2048 |

## 2. Build and runtime verification matrix

| Level | Result |
|---|---|
| Present in source | all 86 rows: yes |
| Registered in CMake | per table |
| Launcher entry | 72 `.af`; gaps above |
| Built (OS cross-build) | **not verified** (no toolchain run) |
| In image | **not verified** |
| Launchable | **not verified**; ExileStore provably has no launcher entry |
| Functionally verified | **none**; only Exile Store host core (unit tests + CLI smoke) |

## 3. Per-app assessment

Apps with real reads are first; everything else follows with generated inventory-level entries. Structure: purpose / build evidence / working / broken or incomplete / missing / UI-a11y / reliability-security / recommendation / priority-effort / tests.

### ExileStore — evidence: Source-read + HOST build/tests only
* Working (source-level): Native app is a read-only catalog browser (223 lines). Install/launch disabled and honestly labelled. Host tree (CLI/core, 2/2 ctest pass, 0 warnings) is a separate, much larger codebase not wired into the OS image.
* Broken / incomplete: (1) use-after-free: ItemListModel keeps `Container const&`, model was built over a lambda-local Vector (VERIFIED in source). (2) refresh_list overwrites load_catalog error with "0 of 0" (VERIFIED). (3) install dest `share/exile-store` vs app opens `/usr/share/exile-store` (SUSPECTED, high confidence; prefix is Build/Root with DATAROOTDIR=res). (4) No .af entry, nothing references it: not launchable by a user (VERIFIED by grep). (5) placeholder icon app-help.
* Recommendation: **FIX (P0 for 1,2; P1 for 3,4). Patch provided; native compile NOT verified.**

#### ExileStore remediation (branch `exilestore/complete-native-store`; validated results only)

| # | Finding | Status | How it was checked |
|---|---|---|---|
| 1 | Model use-after-free | **Fixed in source** | Replaced `ItemListModel` over a lambda-local `Vector` with `CatalogListModel`, which owns its strings. Regression coverage addresses result/filter lifetime. The GUI model has not been executed in the OS runtime. |
| 2 | Load error masked by "0 of 0" | **Fixed in source; tests added** | `status_text()` distinguishes a failed load from an empty catalog. Regression tests cover failed loads, empty catalogs, and malformed input; the standalone CatalogLoader tests passed 11/11 in a manual build. CMake/CI registration remains unverified, and mutation testing has not been performed. |
| 3 | Install path mismatch | **Confirmed and fixed** | Traced: `CMAKE_INSTALL_PREFIX=<build>/Root` (CMakeLists.txt:176), `Meta/build-root-filesystem.sh` rsyncs `Root/` onto the image root, so `share/exile-store` landed at `/share/exile-store`, not `/usr/share/exile-store`. Destination is now `usr/share/exile-store`. Verified by running the real `install()` rules in a stub harness and merging `Base/` + staged `Root/`: `/usr/share/exile-store/catalog/catalog.json` exists and parses; no stray `/share/exile-store`. (The audit's `DATAROOTDIR=res` explanation was not the cause.) |
| 4 | No `.af` entry | **Fixed** | `Base/res/apps/ExileStore.af` (`Executable=/bin/ExileStore`, Utilities). Staged install produces `/bin/ExileStore`, matching the descriptor. Not launched from a running desktop. |
| 5 | Placeholder icon | **Fixed** | `Base/res/icons/16x16/exile-store.png` and `32x32/exile-store.png` generated from the project's own `branding/icon-1024.png`; `exile_app(... ICON exile-store)`. |
| 6 | **New:** native source did not compile | **Fixed (syntax/type level)** | Host `clang++ -fsyntax-only` against the real in-tree headers reported 16 errors in the committed `native_main.cpp` (C-string literals and `ByteString` passed to `Label::set_text(String)`, invalid `AK::Literals`, range-for over `JsonArray`). After the rewrite the same check reports 0 errors. This is **not** a cross-build: link, runtime and visual behaviour are unverified. |

Commands and results: host core `tools/host-build.sh` ctest 2/2 passed (unchanged code); `Tests/ExileStore/TestCatalogLoader.cpp` 11/11 passed (clang 18 build, and GCC 13 with ASan+UBSan); `git diff --check` clean; `Meta/check-style.py` exit 0; `clang-format-18` clean on the new C++ files. The Lagom configure could not run here (repo needs a C++26 compiler; sandbox has GCC 13/clang 18), so the test was built by hand against in-tree `AK`/`LibTest` sources; registration in `Tests/ExileStore/CMakeLists.txt`, `Tests/CMakeLists.txt` and `Meta/Lagom/CMakeLists.txt` is therefore unexercised in CI.

Still **not** done (blocked by missing OS infrastructure, not faked): installing, updating, uninstalling and launching packages; installed-state tracking; signature/checksum verification in the native app. There is no privileged package-installer service, package database or launcher API in the repo (`docs/INTEGRATION.md` section 1), the only installer (`src/core/installer.cpp`) is a host-side dev backend with an unprivileged local prefix, and it needs OpenSSL. Smallest next step: implement an OS-side `IPackageBackend` (privileged installer service) and use `LibCrypto::Curves::Ed25519` for verification, then link the host core into the native target.

### Calculator — evidence: Source-read
* Working (source-level): BigFraction arithmetic; guards sqrt(neg), 1/0 (m_has_error).
* Broken / incomplete: 2 FIXMEs (port to GML).
* Recommendation: **KEEP.**

### Piano — evidence: Source-read (partial)
* Working (source-level): Keyboard/roll, WAV export.
* Broken / incomplete: No project save/load; no close-request guard => unsaved work lost silently (VERIFIED). Sampler "We don't actually load the sample" (SamplerWidget.cpp:58). 12 TODO/FIXME.
* Recommendation: **FIX/IMPROVE (P1): add close guard + project file; label Sampler as incomplete.**

### Mail — evidence: Source-read (partial)
* Working (source-level): IMAP client; TLS default on.
* Broken / incomplete: Password read from plaintext Config (MailWidget.cpp:150 FIXME). Plaintext IMAP selectable by user. No secret store.
* Recommendation: **FIX (P1 security): needs a credential store service (NEW CAPABILITY REQUIRED).**

### PartitionEditor — evidence: Source-read
* Working (source-level): Opens devices read-only (PartitionModel.cpp:70); viewer only.
* Broken / incomplete: Name implies editing; it does not edit.
* Recommendation: **KEEP; consider rename/clarify (IMPROVE, P3).**

### About — evidence: Source-read
* Working (source-level): Shows Exile OS.
* Broken / incomplete: Uses `ladyball` (Ladybird) icon; version string falls back to "Version 1.0" off-Serenity.
* Recommendation: **FIX (P2 branding).**

### Welcome — evidence: Source-read (grep)
* Working (source-level): not assessed
* Broken / incomplete: Banner draws text "Serenity" (WelcomeWidget.cpp:128-129).
* Recommendation: **FIX (P2 branding).**

### Maps — evidence: Source-read (grep)
* Working (source-level): not assessed
* Broken / incomplete: Fetches https://usermap.serenityos.org/people.json (UsersMapWidget.cpp:25,80): outbound call to upstream service on use.
* Recommendation: **FIX/INTEGRATE (P2): disable or point to Exile service.**

### Presenter — evidence: Grep only
* Working (source-level): Viewer.
* Broken / incomplete: No close/save hooks (viewer; acceptable).
* Recommendation: **KEEP (unverified).**

### Run — evidence: Source-read (grep)
* Working (source-level): not assessed
* Broken / incomplete: History load/save failures unhandled (RunWindow.cpp:36,137,179).
* Recommendation: **FIX (P3).**

### FileManager — evidence: Grep of TODO/FIXME only
* Working (source-level): not assessed
* Broken / incomplete: 9 TODO/FIXME; FileOperationProgressWidget:135 "Communicate more with the user about errors".
* Recommendation: **IMPROVE (P2); deep review pending.**

### Browser — evidence: Grep of TODO/FIXME only
* Working (source-level): not assessed
* Broken / incomplete: History entry selection not reimplemented (Tab.cpp:148,156,1047; HistoryModel.h:17); BrowserWindow.cpp:715 ColorScheme TODO.
* Recommendation: **IMPROVE (P2); deep review pending.**

### HackStudio — evidence: Grep only
* Working (source-level): not assessed
* Broken / incomplete: project_is_serenity() checks path ends `Source/serenity`; build-root-filesystem.sh:203-206 clones there while Meta/exile.sh:44 documents ~/Source/exile; HackStudioWidget.cpp:1110 uses /usr/src/exile. Naming inconsistent (currently works). Notification text says "Serenity repository" (main.cpp:130).
* Recommendation: **REFACTOR naming (P3).**

### Escalator — evidence: Grep only
* Working (source-level): GUI sudo.
* Broken / incomplete: pledge includes proc exec id; not deeply audited.
* Recommendation: **Security review required (P1).**

### VideoPlayer — evidence: Grep only
* Working (source-level): not assessed
* Broken / incomplete: 0 pledge calls, 3 unveil in inventory; no file association for video.
* Recommendation: **INTEGRATE (P2): add LaunchServer handlers; review pledge.**

### Weather — evidence: Inventory only
* Working (source-level): Jakt app.
* Broken / incomplete: 0 pledge calls; Jakt build not verified.
* Recommendation: **Review (P2).**

### Inventory-level entries (no deep review)

* **3DFileViewer** (`Userland/Applications/3DFileViewer`, 948 LOC): built/runtime unverified. Flags: 4 'serenity' mentions; no man page found; no tests found. Recommendation: KEEP pending deeper review. Features, UX and accessibility: *not assessed*.
* **AnalogClock** (`Userland/Applications/AnalogClock`, 316 LOC): built/runtime unverified. Flags: 1 'serenity' mentions; no man page found; no tests found. Recommendation: KEEP pending deeper review. Features, UX and accessibility: *not assessed*.
* **Assistant** (`Userland/Applications/Assistant`, 754 LOC): built/runtime unverified. Flags: no unveil call found; 2 'serenity' mentions; no man page found; no tests found. Recommendation: KEEP pending deeper review. Features, UX and accessibility: *not assessed*.
* **Audio** (`Userland/Applets/Audio`, 259 LOC): built/runtime unverified. Flags: no .af launcher entry; 4 'serenity' mentions; no man page found; no tests found. Recommendation: KEEP pending deeper review. Features, UX and accessibility: *not assessed*.
* **BrickGame** (`Userland/Games/BrickGame`, 855 LOC): built/runtime unverified. Flags: 4 'serenity' mentions; no tests found. Recommendation: KEEP pending deeper review. Features, UX and accessibility: *not assessed*.
* **BrowserSettings** (`Userland/Applications/BrowserSettings`, 1011 LOC): built/runtime unverified. Flags: 7 'serenity' mentions; no man page found; no tests found. Recommendation: KEEP pending deeper review. Features, UX and accessibility: *not assessed*.
* **Calendar** (`Userland/Applications/Calendar`, 1522 LOC): built/runtime unverified. Flags: 11 'serenity' mentions; no man page found; no tests found. Recommendation: KEEP pending deeper review. Features, UX and accessibility: *not assessed*.
* **CalendarSettings** (`Userland/Applications/CalendarSettings`, 270 LOC): built/runtime unverified. Flags: 3 'serenity' mentions; no man page found; no tests found. Recommendation: KEEP pending deeper review. Features, UX and accessibility: *not assessed*.
* **CatDog** (`Userland/Demos/CatDog`, 536 LOC): built/runtime unverified. Flags: 5 'serenity' mentions; no man page found; no tests found. Recommendation: KEEP pending deeper review. Features, UX and accessibility: *not assessed*.
* **CertificateSettings** (`Userland/Applications/CertificateSettings`, 345 LOC): built/runtime unverified. Flags: 1 'serenity' mentions; no man page found; no tests found. Recommendation: KEEP pending deeper review. Features, UX and accessibility: *not assessed*.
* **CharacterMap** (`Userland/Applications/CharacterMap`, 579 LOC): built/runtime unverified. Flags: 12 'serenity' mentions; no man page found; no tests found. Recommendation: KEEP pending deeper review. Features, UX and accessibility: *not assessed*.
* **Chess** (`Userland/Games/Chess`, 2050 LOC): built/runtime unverified. Flags: 18 'serenity' mentions; no tests found. Recommendation: KEEP pending deeper review. Features, UX and accessibility: *not assessed*.
* **ClipboardHistory** (`Userland/Applets/ClipboardHistory`, 420 LOC): built/runtime unverified. Flags: no .af launcher entry; 7 'serenity' mentions; no man page found; no tests found. Recommendation: KEEP pending deeper review. Features, UX and accessibility: *not assessed*.
* **ClockSettings** (`Userland/Applications/ClockSettings`, 549 LOC): built/runtime unverified. Flags: 5 'serenity' mentions; no man page found; no tests found. Recommendation: KEEP pending deeper review. Features, UX and accessibility: *not assessed*.
* **ColorLines** (`Userland/Games/ColorLines`, 1076 LOC): built/runtime unverified. Flags: 8 'serenity' mentions; no tests found. Recommendation: KEEP pending deeper review. Features, UX and accessibility: *not assessed*.
* **CrashReporter** (`Userland/Applications/CrashReporter`, 507 LOC): built/runtime unverified. Flags: no .af launcher entry; no unveil call found; 4 'serenity' mentions; no man page found; no tests found. Recommendation: KEEP pending deeper review. Features, UX and accessibility: *not assessed*.
* **Debugger** (`Userland/Applications/Debugger`, 445 LOC): built/runtime unverified. Flags: no .af launcher entry; no unveil call found; 2 'serenity' mentions; no man page found; no tests found. Recommendation: KEEP pending deeper review. Features, UX and accessibility: *not assessed*.
* **DisplaySettings** (`Userland/Applications/DisplaySettings`, 2373 LOC): built/runtime unverified. Flags: no unveil call found; 18 'serenity' mentions; no man page found; no tests found. Recommendation: KEEP pending deeper review. Features, UX and accessibility: *not assessed*.
* **Eyes** (`Userland/Demos/Eyes`, 262 LOC): built/runtime unverified. Flags: 6 'serenity' mentions; no man page found; no tests found. Recommendation: KEEP pending deeper review. Features, UX and accessibility: *not assessed*.
* **FlappyBug** (`Userland/Games/FlappyBug`, 424 LOC): built/runtime unverified. Flags: 1 'serenity' mentions; no tests found. Recommendation: KEEP pending deeper review. Features, UX and accessibility: *not assessed*.
* **Flood** (`Userland/Games/Flood`, 749 LOC): built/runtime unverified. Flags: 10 'serenity' mentions; no tests found. Recommendation: KEEP pending deeper review. Features, UX and accessibility: *not assessed*.
* **FontEditor** (`Userland/Applications/FontEditor`, 2751 LOC): built/runtime unverified. Flags: 14 'serenity' mentions; no man page found; no tests found. Recommendation: KEEP pending deeper review. Features, UX and accessibility: *not assessed*.
* **GMLPlayground** (`Userland/DevTools/GMLPlayground`, 507 LOC): built/runtime unverified. Flags: 12 'serenity' mentions; no man page found; no tests found. Recommendation: KEEP pending deeper review. Features, UX and accessibility: *not assessed*.
* **GameOfLife** (`Userland/Games/GameOfLife`, 1072 LOC): built/runtime unverified. Flags: 10 'serenity' mentions; no tests found. Recommendation: KEEP pending deeper review. Features, UX and accessibility: *not assessed*.
* **GamesSettings** (`Userland/Applications/GamesSettings`, 810 LOC): built/runtime unverified. Flags: 8 'serenity' mentions; no man page found; no tests found. Recommendation: KEEP pending deeper review. Features, UX and accessibility: *not assessed*.
* **Gradient** (`Userland/Demos/Gradient`, 111 LOC): built/runtime unverified. Flags: 2 'serenity' mentions; no man page found; no tests found. Recommendation: KEEP pending deeper review. Features, UX and accessibility: *not assessed*.
* **Hearts** (`Userland/Games/Hearts`, 1697 LOC): built/runtime unverified. Flags: 16 'serenity' mentions; no tests found. Recommendation: KEEP pending deeper review. Features, UX and accessibility: *not assessed*.
* **Help** (`Userland/Applications/Help`, 836 LOC): built/runtime unverified. Flags: 14 'serenity' mentions; no man page found; no tests found. Recommendation: KEEP pending deeper review. Features, UX and accessibility: *not assessed*.
* **HexEditor** (`Userland/Applications/HexEditor`, 4078 LOC): built/runtime unverified. Flags: 13 TODO/FIXME; 34 'serenity' mentions; no man page found; no tests found. Recommendation: KEEP pending deeper review. Features, UX and accessibility: *not assessed*.
* **ImageViewer** (`Userland/Applications/ImageViewer`, 984 LOC): built/runtime unverified. Flags: 10 'serenity' mentions; no man page found; no tests found. Recommendation: KEEP pending deeper review. Features, UX and accessibility: *not assessed*.
* **KeyboardMapper** (`Userland/Applications/KeyboardMapper`, 679 LOC): built/runtime unverified. Flags: no unveil call found; 8 'serenity' mentions; no man page found; no tests found. Recommendation: KEEP pending deeper review. Features, UX and accessibility: *not assessed*.
* **KeyboardSettings** (`Userland/Applications/KeyboardSettings`, 643 LOC): built/runtime unverified. Flags: 5 'serenity' mentions; no man page found; no tests found. Recommendation: KEEP pending deeper review. Features, UX and accessibility: *not assessed*.
* **Keymap** (`Userland/Applets/Keymap`, 202 LOC): built/runtime unverified. Flags: no .af launcher entry; no unveil call found; 7 'serenity' mentions; no man page found; no tests found. Recommendation: KEEP pending deeper review. Features, UX and accessibility: *not assessed*.
* **LibGfxDemo** (`Userland/Demos/LibGfxDemo`, 213 LOC): built/runtime unverified. Flags: 3 'serenity' mentions; no man page found; no tests found. Recommendation: KEEP pending deeper review. Features, UX and accessibility: *not assessed*.
* **LibGfxScaleDemo** (`Userland/Demos/LibGfxScaleDemo`, 133 LOC): built/runtime unverified. Flags: 2 'serenity' mentions; no man page found; no tests found. Recommendation: KEEP pending deeper review. Features, UX and accessibility: *not assessed*.
* **Magnifier** (`Userland/Applications/Magnifier`, 423 LOC): built/runtime unverified. Flags: 3 'serenity' mentions; no man page found; no tests found. Recommendation: KEEP pending deeper review. Features, UX and accessibility: *not assessed*.
* **MailSettings** (`Userland/Applications/MailSettings`, 291 LOC): built/runtime unverified. Flags: 7 'serenity' mentions; no man page found; no tests found. Recommendation: KEEP pending deeper review. Features, UX and accessibility: *not assessed*.
* **Mandelbrot** (`Userland/Demos/Mandelbrot`, 523 LOC): built/runtime unverified. Flags: 3 'serenity' mentions; no man page found; no tests found. Recommendation: KEEP pending deeper review. Features, UX and accessibility: *not assessed*.
* **MapsSettings** (`Userland/Applications/MapsSettings`, 243 LOC): built/runtime unverified. Flags: 1 'serenity' mentions; no man page found; no tests found. Recommendation: KEEP pending deeper review. Features, UX and accessibility: *not assessed*.
* **MasterWord** (`Userland/Games/MasterWord`, 603 LOC): built/runtime unverified. Flags: 2 'serenity' mentions; no tests found. Recommendation: KEEP pending deeper review. Features, UX and accessibility: *not assessed*.
* **Minesweeper** (`Userland/Games/Minesweeper`, 1192 LOC): built/runtime unverified. Flags: 10 'serenity' mentions; no tests found. Recommendation: KEEP pending deeper review. Features, UX and accessibility: *not assessed*.
* **ModelGallery** (`Userland/Demos/ModelGallery`, 302 LOC): built/runtime unverified. Flags: no unveil call found; 1 'serenity' mentions; no man page found; no tests found. Recommendation: KEEP pending deeper review. Features, UX and accessibility: *not assessed*.
* **MouseSettings** (`Userland/Applications/MouseSettings`, 1105 LOC): built/runtime unverified. Flags: no unveil call found; 13 'serenity' mentions; no man page found; no tests found. Recommendation: KEEP pending deeper review. Features, UX and accessibility: *not assessed*.
* **Network** (`Userland/Applets/Network`, 183 LOC): built/runtime unverified. Flags: no .af launcher entry; 3 'serenity' mentions; no tests found. Recommendation: KEEP pending deeper review. Features, UX and accessibility: *not assessed*.
* **NetworkSettings** (`Userland/Applications/NetworkSettings`, 399 LOC): built/runtime unverified. Flags: 1 'serenity' mentions; no man page found; no tests found. Recommendation: KEEP pending deeper review. Features, UX and accessibility: *not assessed*.
* **PDFViewer** (`Userland/Applications/PDFViewer`, 1604 LOC): built/runtime unverified. Flags: 18 'serenity' mentions; no man page found; no tests found. Recommendation: KEEP pending deeper review. Features, UX and accessibility: *not assessed*.
* **PixelPaint** (`Userland/Applications/PixelPaint`, 16302 LOC): built/runtime unverified. Flags: 22 TODO/FIXME; 176 'serenity' mentions; no man page found; no tests found. Recommendation: KEEP pending deeper review. Features, UX and accessibility: *not assessed*.
* **Profiler** (`Userland/DevTools/Profiler`, 4370 LOC): built/runtime unverified. Flags: no pledge call found; no unveil call found; 49 'serenity' mentions; no man page found; no tests found. Recommendation: KEEP pending deeper review. Features, UX and accessibility: *not assessed*.
* **ResourceGraph** (`Userland/Applets/ResourceGraph`, 305 LOC): built/runtime unverified. Flags: no .af launcher entry; 4 'serenity' mentions; no man page found; no tests found. Recommendation: KEEP pending deeper review. Features, UX and accessibility: *not assessed*.
* **SQLStudio** (`Userland/DevTools/SQLStudio`, 864 LOC): built/runtime unverified. Flags: no pledge call found; no unveil call found; 4 'serenity' mentions; no man page found; no tests found. Recommendation: KEEP pending deeper review. Features, UX and accessibility: *not assessed*.
* **Screensaver** (`Userland/Demos/Screensaver`, 111 LOC): built/runtime unverified. Flags: no unveil call found; 2 'serenity' mentions; no man page found; no tests found. Recommendation: KEEP pending deeper review. Features, UX and accessibility: *not assessed*.
* **Screenshot** (`Userland/Applications/Screenshot`, 267 LOC): built/runtime unverified. Flags: no unveil call found; 1 'serenity' mentions; no man page found. Recommendation: KEEP pending deeper review. Features, UX and accessibility: *not assessed*.
* **Settings** (`Userland/Applications/Settings`, 147 LOC): built/runtime unverified. Flags: no .af launcher entry; no unveil call found; 2 'serenity' mentions; no man page found; no tests found. Recommendation: KEEP pending deeper review. Features, UX and accessibility: *not assessed*.
* **Snake** (`Userland/Games/Snake`, 984 LOC): built/runtime unverified. Flags: 31 'serenity' mentions; no tests found. Recommendation: KEEP pending deeper review. Features, UX and accessibility: *not assessed*.
* **Solitaire** (`Userland/Games/Solitaire`, 1294 LOC): built/runtime unverified. Flags: 8 'serenity' mentions; no tests found. Recommendation: KEEP pending deeper review. Features, UX and accessibility: *not assessed*.
* **SoundPlayer** (`Userland/Applications/SoundPlayer`, 1912 LOC): built/runtime unverified. Flags: no unveil call found; 11 TODO/FIXME; 25 'serenity' mentions; no man page found; no tests found. Recommendation: KEEP pending deeper review. Features, UX and accessibility: *not assessed*.
* **SpaceAnalyzer** (`Userland/Applications/SpaceAnalyzer`, 1060 LOC): built/runtime unverified. Flags: no pledge call found; no unveil call found; 13 'serenity' mentions; no man page found; no tests found. Recommendation: KEEP pending deeper review. Features, UX and accessibility: *not assessed*.
* **Spider** (`Userland/Games/Spider`, 958 LOC): built/runtime unverified. Flags: 7 'serenity' mentions; no tests found. Recommendation: KEEP pending deeper review. Features, UX and accessibility: *not assessed*.
* **Spreadsheet** (`Userland/Applications/Spreadsheet`, 8088 LOC): built/runtime unverified. Flags: 17 TODO/FIXME; 48 'serenity' mentions; no man page found. Recommendation: KEEP pending deeper review. Features, UX and accessibility: *not assessed*.
* **Starfield** (`Userland/Demos/Starfield`, 185 LOC): built/runtime unverified. Flags: no unveil call found; 2 'serenity' mentions; no man page found; no tests found. Recommendation: KEEP pending deeper review. Features, UX and accessibility: *not assessed*.
* **SystemMonitor** (`Userland/Applications/SystemMonitor`, 2996 LOC): built/runtime unverified. Flags: 39 'serenity' mentions; no man page found; no tests found. Recommendation: KEEP pending deeper review. Features, UX and accessibility: *not assessed*.
* **Terminal** (`Userland/Applications/Terminal`, 468 LOC): built/runtime unverified. Flags: 2 'serenity' mentions; no man page found; no tests found. Recommendation: KEEP pending deeper review. Features, UX and accessibility: *not assessed*.
* **TerminalSettings** (`Userland/Applications/TerminalSettings`, 705 LOC): built/runtime unverified. Flags: 7 'serenity' mentions; no man page found; no tests found. Recommendation: KEEP pending deeper review. Features, UX and accessibility: *not assessed*.
* **TextEditor** (`Userland/Applications/TextEditor`, 1460 LOC): built/runtime unverified. Flags: 8 'serenity' mentions; no man page found; no tests found. Recommendation: KEEP pending deeper review. Features, UX and accessibility: *not assessed*.
* **ThemeEditor** (`Userland/Applications/ThemeEditor`, 1391 LOC): built/runtime unverified. Flags: no unveil call found; 14 'serenity' mentions; no man page found; no tests found. Recommendation: KEEP pending deeper review. Features, UX and accessibility: *not assessed*.
* **Tubes** (`Userland/Demos/Tubes`, 505 LOC): built/runtime unverified. Flags: no unveil call found; 1 'serenity' mentions; no man page found; no tests found. Recommendation: KEEP pending deeper review. Features, UX and accessibility: *not assessed*.
* **TwentyFourtyEight** (`Userland/Games/TwentyFourtyEight`, 1218 LOC): built/runtime unverified. Flags: 10 'serenity' mentions; no man page found; no tests found. Recommendation: KEEP pending deeper review. Features, UX and accessibility: *not assessed*.
* **UsersSettings** (`Userland/Applications/UsersSettings`, 932 LOC): built/runtime unverified. Flags: 1 'serenity' mentions; no man page found; no tests found. Recommendation: KEEP pending deeper review. Features, UX and accessibility: *not assessed*.
* **WidgetGallery** (`Userland/Demos/WidgetGallery`, 1364 LOC): built/runtime unverified. Flags: 8 'serenity' mentions; no man page found; no tests found. Recommendation: KEEP pending deeper review. Features, UX and accessibility: *not assessed*.
* **WorkspacePicker** (`Userland/Applets/WorkspacePicker`, 251 LOC): built/runtime unverified. Flags: no .af launcher entry; 7 'serenity' mentions; no man page found; no tests found. Recommendation: KEEP pending deeper review. Features, UX and accessibility: *not assessed*.

## 4. Missing, incomplete, broken, duplicated (verified unless marked)

* Exile Store native: use-after-free, error masking, suspected install-path mismatch, unreachable from launcher, placeholder icon.
* Piano: no save, no close guard, sampler stub.
* Mail: plaintext password in config.
* File associations (`Base/etc/LaunchServer.ini`): handlers exist for images, html, audio, txt, font, sheets, csv, gml, pdf, profile, pp, directory; **missing**: video (VideoPlayer unmapped), 3D models (3DFileViewer), md, archives, midi (Piano), `mailto`. Unknown/binary files fall back to TextEditor.
* Category oddity: "Demos/Screensaver" used by 3 apps.
* CI: `discord.yml` and `nightly.yml` gated on `refs/heads/master`, default branch is `main` => never fire. Template comments say `master`; disabled workflows still use `SERENITY_*` env.
* Stale existing doc: REMOVAL_CANDIDATES claims `SERENITY_QEMU_CPU` is read while CI sets `EXILE_QEMU_CPU`; `Meta/run.py` actually reads `EXILE_*` with `SERENITY_*` fallback.
* Stale Store docs: `docs/AUDIT.md`, `NOTICE` partly outdated.

## 5. Per-app additions (proposed)

ExileStore: launcher `.af`, real icon, LaunchServer `exile-store:` protocol, verified-install flow via host core. Piano: project save/load + close guard. Mail: credential storage service. VideoPlayer/3DFileViewer/Archive: LaunchServer handlers. FileManager: richer error reporting. Run: error handling for history. Browser: history selection.

## 6. Removal, deprecation, merge candidates — **owner approval required; nothing is to be auto-removed**

| Candidate | Reason | Risk |
|---|---|---|
| LibGfxDemo / LibGfxScaleDemo / Gradient | overlapping graphics demos | low; useful as library tests |
| Screensaver / Starfield / Tubes | overlapping screensaver-type demos; odd category | low |
| ModelGallery vs 3DFileViewer | overlap | medium |
| BrickGame vs Snake / FlappyBug | overlapping casual games | low |
| ImageViewer vs PixelPaint viewing | overlap | medium: different use cases |
| SoundPlayer vs Piano | different purposes; **not** a merge, listed only for review | — |

Demos and dev tools are not recommended for removal merely for being so.

## 7. Security, reliability, accessibility, performance

* Security (verified): Mail plaintext password; Maps calls an upstream host; Escalator and VideoPlayer need pledge/unveil review; apps with 0 pledge calls: Weather, VideoPlayer, SpaceAnalyzer, Profiler, SQLStudio (inventory count, may use other mechanisms).
* Reliability (verified): Store UAF; Piano data loss. Close guards exist (grep counts) for TextEditor, HexEditor, PixelPaint, Spreadsheet, FontEditor, ThemeEditor, HackStudio, SQLStudio, Calendar; depth not read.
* Accessibility and performance: **not assessed** (needs running UI).
* Branding: Welcome banner "Serenity"; Chess PGN tag "SerenityOS Chess"; mime `image/x-serenityos` (PixelPaint, ClipboardHistory); man pages mention Serenity (About, Browser, Calculator, Calendar, FontEditor, Mail, Assistant, GMLPlayground, Help, MouseSettings); `Base/res/html/misc/*.html` 29 mentions; About uses Ladybird icon. ABI names (`serenity_main`, `AK_OS_SERENITY`, icon sections) are intentionally kept.
* Palette: purple/magenta hues in 29 of 37 theme/colour-scheme files (Plum, Contrast, Pumpkin, Dark worst). Terminal ANSI "magenta" is expected and separate. Identity is black/white/green/blue.

## 8. Shared-library and OS-integration improvements

1. A credential store service (Mail, Store, future ARC AI keys). 2. Shared close-guard/dirty-state helper to standardise unsaved-work prompts. 3. LaunchServer: add missing handlers and Exile protocols. 4. Theme pass: purple → Exile palette tokens; keep ANSI palette. 5. Crash diagnostics: CrashDaemon/CrashReporter already exist; standardise the report UI. 6. Notifications through the existing NotificationServer. 7. Unified shortcuts/settings pages (not audited).

## 9. ARC AI map

Facts: no ARC AI service, API or reference exists in the repo (grep). `Userland/Applications/Assistant` is launcher search (providers App, Calculator, File, Terminal, URL), not AI.
Proposal (design only, nothing invented): one shared `ArcAIServer` over LibIPC, per-app permission via an FileSystemAccessServer-style prompt; approvals bound to a single action and its arguments, never standing; no-provider/offline state shown as unavailable, apps fully functional; no decorative AI buttons. Candidate uses once the service exists: TextEditor/HackStudio explain/summarise (selection only), Terminal command explanation (never auto-run), Mail draft assist, Assistant natural-language query. Provider, keys and data policy: owner decision.

## 10. Roadmap

* **P0 (S):** Store UAF + error masking: **done on branch `exilestore/complete-native-store`** (unit-tested; native cross-build still unverified).
* **P1:** Store launcher entry + icon + install-path (S) **done on the same branch (staged-install verified)**; Store installer backend (L, blocked: no OS installer service); Piano close guard (S) and project file (M); Mail credential storage (L); Escalator security review (M); CI branch gating (S).
* **P2:** Branding leftovers (M); theme palette (M); LaunchServer handlers (S); Maps upstream call (S); About icon (S); FileManager/Browser FIXMEs (M each).
* **P3:** HackStudio naming (S); Run error handling (S); PartitionEditor naming (S); category cleanup (S); candidates in §6 after approval.
* **ARC AI:** design (M) → service (L) → per-app integration (M each).

## 11. Test plan and acceptance criteria

* Build: full cross-build per app group in CI; each app launches from `.af` and exits cleanly.
* Store: host ctest stays 2/2; native app opens catalog from the installed path; missing catalog shows the specific error; ASan run with list refresh/search 100×.
* Piano: closing with unsaved work prompts; project round-trips.
* LaunchServer: opening each handled type launches the right app.
* Branding: grep gate for user-visible "Serenity" outside allow-listed ABI names; theme hue check.
* Mail: no password in plain Config after fix.
* Accessibility: keyboard-only pass per app (manual, after boot).

## 12. Unverifiable items here

Full OS build and image; QEMU boot; any runtime/GUI behaviour; native ExileStore compile of the supplied patch; Jakt Weather; hardware/audio/network; accessibility; performance; whether install-path mismatch actually occurs; the Escalator/UsersSettings privilege logic in depth; apps listed as inventory-level only.
