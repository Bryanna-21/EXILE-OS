# Non-kernel system audit

Baseline: `origin/main` at `d0b61526ad2378fe3bf801bf2c94834bad8b3220`. Scope: everything outside `Kernel/` and `ExileKernel/`.

## How to read this document

* Every finding names a repository path. A finding that rests on something I could not run says so.
* **Verified by running** means I built or executed it for this audit (section 2). **Read** means I read the source or build files only. **Not verified** means neither.
* Status values: **Works** (exists and has evidence it works), **Incomplete** (exists, known gaps), **Prototype** (exists, not wired into the OS or not production-grade), **Missing** (nothing found), **Duplicate**.
* Older audits in this directory (for example `REQUIREMENTS_GAP_MATRIX.md`) were written against commit `18a3c70` and are partly out of date. Where they disagree with this document, this document reflects the current tree.

## 1. Limits of this audit

* The full OS build, QEMU boot and CI results were **not** reproduced. The Lagom configure step downloads a CA bundle and the network proxy in the audit environment refused it, so `ctest` could not be run through the real build system.
* The ExileBoot signing tool and the verifier tests were compiled directly with `g++ 13.3` against the repository's `AK/` and `LibCrypto` sources. The 12 verifier cases were run against a local stand-in for `LibTest` (macros `TEST_CASE`, `EXPECT_EQ`, `EXPECT_NE` only), so they validate the logic but not the real harness.
* The EFI prekernel integration (`Kernel/EFIPrekernel/BootVerification.cpp`) was **not** compiled for any target.
* Line counts in section 3 are `.cpp`, `.h`, `.ipc` and `.gml` lines under each directory, a size indicator and not a quality measure.

## 2. Boot and trust: verification of the known audit items

| Item | Result | Evidence |
|---|---|---|
| ExileBootManifest is registered | **Still true.** The host tool is registered in the host build. | `Meta/Lagom/Tools/CMakeLists.txt` (`add_subdirectory(ExileBootManifest)`), `Meta/Lagom/Tools/ExileBootManifest/CMakeLists.txt` (`lagom_tool(ExileBootManifest ...)`) |
| `TestBootGuardian.cpp` runs through `exile_test` and CTest | **Still true by reading the build files.** Registered for the host build (`ExileBoot` is in `TEST_DIRECTORIES`, `Meta/Lagom/CMakeLists.txt`) and for the target build (`Tests/CMakeLists.txt`). The host CI job runs `ctest` (`.github/workflows/lagom-template.yml`, step "Test"). I did not observe a CI run. | `Tests/ExileBoot/CMakeLists.txt` |
| Standalone `test-boot-integrity.sh` was not demonstrated in CI | **Confirmed, and worse: it was wired into nothing.** Before this change the only references to the script were its own header and `Documentation/BootIntegrity.md`. No CTest entry, no workflow step. | `grep -rn test-boot-integrity` over the tree |
| Signing-tool trust-anchor validation gap | **Confirmed.** `ExileBootManifest sign` never compared the seed with the trust anchor. The prekernel build only checked that both key files are 32 bytes (`Kernel/EFIPrekernel/CMakeLists.txt`, key-size loop). A seed that does not belong to `BOOT_TRUST_ANCHOR_PUBKEY` built successfully and could only fail at boot. All-zero keys were accepted by `sign` and `verify`. | `Meta/Tools/ExileBootManifest.cpp` before this change; reproduced against the old binary (7 new checks fail against it) |

Baseline before any change, by running: the signing tool and the 24-check tamper script passed (24/24) and the 12 verifier cases passed (12/12, stand-in harness).

### What the boot path is, and is not

* **Format and verifier**: `Kernel/Boot/Exile/BootManifest.h` (128-byte v2 manifest, strict field-by-field parse), `Kernel/Boot/Exile/BootGuardian.{h,cpp}` (strict parse, key-id lookup, Ed25519 over a domain-separated message, size check, SHA-256). Fails closed on no trust anchor. Status: **Works** on a development host.
* **Host tool**: `Meta/Tools/ExileBootManifest.cpp` (`keygen`, `sign`, `verify`). **Works**.
* **Prekernel hook**: `Kernel/EFIPrekernel/BootVerification.cpp` calls the verifier and panics on failure. Compiled only with `ENABLE_BOOT_VERIFICATION=ON` (default OFF, `Meta/CMake/exile_options.cmake`). **Prototype**: never compiled for the target or booted in this audit, and `Documentation/BootIntegrity.md` says the same.
* **Not present**: a tamper boot test in QEMU, anything covering the initrd or userland images, rollback protection, key revocation or rotation (the verifier accepts several anchors, which is the only rotation mechanism), measured boot. `Documentation/BootIntegrity.md` lists these limits.
* **Also**: `Kernel/Prekernel/` (non-EFI) has no verification hook; `Documentation/BootIntegrity.md` scopes the feature to EFI boot only.
* **Bootloader paths**: `Meta/build-image-{grub,grub-uefi,limine,extlinux,uefi,qemu,raspberry-pi}.sh`. None of them involves the signed manifest; only the EFI prekernel path would.

## 3. Component inventory

Sizes are the line counts described in section 1. "Tests" counts files under `Tests/` whose path contains the component name, or a test directory named for it; it is a rough guide.

### 3.1 Boot and trust

| Component | Path | Status | Notes |
|---|---|---|---|
| Boot manifest format | `Kernel/Boot/Exile/BootManifest.h` | Works | 128 bytes, strict |
| Verifier ("BootGuardian") | `Kernel/Boot/Exile/BootGuardian.{h,cpp}` | Works (host) | 12 unit cases |
| Signing tool | `Meta/Tools/ExileBootManifest.cpp` | Works | now 44 tamper checks |
| EFI prekernel hook | `Kernel/EFIPrekernel/BootVerification.cpp`, `TrustAnchors.cpp.in` | Prototype | opt-in, not target-built here |
| Boot images | `Meta/build-image-*.sh` | Incomplete | seven variants, manual `dd` install per `Documentation/BareMetalInstallation.md` |
| Boot docs | `Documentation/BootIntegrity.md` | Works | honest about limits |

### 3.2 System services (`Userland/Services/`)

| Service | Lines | Status | Notes |
|---|---|---|---|
| SystemServer (init and service manager) | 699 | Incomplete | Reads `Base/etc/SystemServer.ini`; supports `KeepAlive`, lazy start, per-mode start, a restart-attempt counter (`Service.h`: `m_keep_alive`, `m_lazy`, `m_restart_attempts`). No dependency graph found; no documented restart-policy contract; no tests |
| LoginServer | 307 | Incomplete | Session start; not verified |
| ConfigServer | 476 | Works (read) | Config IPC |
| NotificationServer | 441 | Works (read) | Desktop notifications |
| WindowServer | 16578 | Works (inherited) | Compositor and window manager |
| Taskbar | 2260 | Works (inherited) | Taskbar |
| LaunchServer | 697 | Works (inherited) | Application launching by handler |
| AudioServer | 927 | Works (inherited) | |
| NetworkServer / DHCPClient / LookupServer | 105 / 834 / 869 | Incomplete | IPv4 DHCP and DNS lookup; see networking gaps |
| DeviceMapper | 500 | Incomplete | Device nodes |
| CrashDaemon | 48 | Works (inherited) | Coredump collection |
| FileSystemAccessServer, FileOperation, Clipboard | 240 / 376 / 210 | Works (inherited) | |
| RequestServer, WebContent, WebWorker, WebDriver, ImageDecoder | | Works (inherited) | Browser engine support |
| SSHServer, TelnetServer, WebServer, EchoServer, SpiceAgent, SQLServer | | Works (inherited) | Not core to the boot path |

Missing as services (nothing found by path or content search): a system log service (only `Userland/Libraries/LibC/syslog.h`), a health or diagnostics monitor, a time-synchronisation service (only the one-shot tool `Userland/Utilities/ntpquery.cpp`), a firewall or packet filter, a Wi-Fi manager, a connection monitor, an update service, a storage monitor or disk-error reporter, a unified session and permission service. `Base/etc/` holds flat configuration files: `SystemServer.ini`, `Network.ini`, `fstab`, `passwd`, `shadow`, `group`, `timezone`.

### 3.3 Storage

| Area | Path | Status |
|---|---|---|
| Filesystem drivers | `Kernel/FileSystem/` | In the kernel; out of scope |
| Trash (library) | `Userland/Libraries/LibFileSystem/Trash.{h,cpp}`, `Tests/LibFileSystem/TestTrash.cpp` | Works (tested in host build) |
| Trash (UI) | `Userland/Applications/XFiles/TrashActions.{h,cpp}`, `FileUtils.cpp` | Works (read): ordinary Delete moves to Trash and never falls back to permanent deletion (`delete_paths`, `FileUtils.cpp:23-27`); permanent delete and Empty Trash are separate menu actions (`main.cpp`, Trash menu) |
| Partition editor | `Userland/Applications/PartitionEditor` | Prototype (273 lines) |
| Space analysis | `Userland/Applications/SpaceAnalyzer` | Works (inherited) |
| Mount management, disk-error reporting, storage monitoring, secure storage | | Missing |

### 3.4 Networking

| Area | Evidence | Status |
|---|---|---|
| Wired and TCP/IP stack | `Kernel/Net/` | In the kernel; out of scope. `Documentation/BareMetalInstallation.md` lists e1000 and Realtek 8168 only |
| Network settings UI | `Userland/Applications/NetworkSettings` (399 lines) | Incomplete (adapter settings) |
| Wi-Fi | `Documentation/BareMetalInstallation.md` states "There is no WiFi support"; a search for `802.11`, `wlan` and `WiFi` in `Kernel/`, `Userland/Services`, `Userland/Applications` finds nothing relevant | Missing, and blocked on drivers |
| DNS | `Userland/Services/LookupServer` | Works (inherited) |
| Firewall, connection monitoring, diagnostics service | none found | Missing |

### 3.5 Desktop and applications

Roadmap applications from the mission, against the tree:

| Roadmap item | In tree | Status |
|---|---|---|
| XFiles (file manager) | `Userland/Applications/XFiles` (4944 lines) **and** `Userland/Applications/FileManager` (4753 lines) | **Duplicate, both built**: both are added in `Userland/Applications/CMakeLists.txt` (lines 6 and 19). `diff -r` shows 158 differing lines; XFiles adds `TrashActions` and prefixed GML files. A fork, not two independent apps |
| Bridgeline (browser) | `Userland/Applications/Bridgeline` is built (`Userland/Applications/CMakeLists.txt`, line 5). `Userland/Applications/Browser` (same size, 4962 lines) is **not** in that file and no other build file references it | **Duplicate source tree**: an unbuilt copy. `diff -r` shows 87 differing lines. `Ladybird/` and the Web services provide the engine. It is not listed in `REMOVAL_CANDIDATES.md`; whether to remove it is a decision for the owners, and nothing is deleted here |
| Audio (native audio application) | `Userland/Applications/SoundPlayer` (1912 lines), `Userland/Applications/Piano`, `Userland/Services/AudioServer` | No application named Audio. SoundPlayer is the nearest existing implementation; do not create a second one without a decision |
| Exile Store | `Userland/Applications/ExileStore` (12446 lines, 10 test files, docs) | **Prototype**, see 3.6 |
| Document viewer | `PDFViewer` (1604), `Presenter` (936) | PDF works (inherited); Office document viewing: nothing found |
| Image viewer | `ImageViewer` (984) | Works (inherited) |
| Archive manager | none found as an application (archive support exists as a Properties tab and libraries) | Missing |
| Calculator, Calendar, Terminal, System monitor, Settings | `Calculator`, `Calendar`, `Terminal`, `SystemMonitor`, `Settings` | Present (inherited); not individually verified |
| Accessibility | `Userland/Applications/Magnifier` (423) | Incomplete: no screen reader or other accessibility service found |
| Desktop shell, launcher | `Taskbar`, `Assistant`, `Run`, `Welcome` | Inherited; Exile identity work not verified |
| Display server / compositor | `WindowServer` | Works (inherited) |

Branding (black, white, green, blue, no purple) was not re-audited here; see `BRANDING_PALETTE_AUDIT.md`.

### 3.6 Packages and updates

* `Userland/Applications/ExileStore` implements catalog loading and validation, a compatibility resolver, Ed25519 trust verification (OpenSSL), an `.expkg` development package format, an installer pipeline, and a loopback web UI. Its own `docs/SECURITY.md` states that it is **not production-secure** and lists nine limits, among them: no sandbox or permission enforcement, no privileged installer, a verification-to-install TOCTOU window, an unsigned catalog (so a signed old vulnerable version can be served for a fresh install), no HTTPS transport, and an unprotected local trust store. Its tests are a host build (`tools/host-build.sh`); the repository-level `Tests/ExileStore/TestCatalogLoader.cpp` is registered for the OS and host builds. Not re-run here.
* `Ports/` holds 371 entries inherited from the upstream port system. That is the only other install mechanism found and it is a build-time system, not a package manager.
* Missing: a package format agreed with the OS (as opposed to the Store's dev format), dependency resolution beyond the Store's exact-id model, signed repositories, update management and rollback, an installer service.

### 3.7 Installation and release

| Area | Evidence | Status |
|---|---|---|
| Image generation | `Meta/build-image-*.sh` (see 3.1) | Works for developers, per the CI step `ninja install && ninja qemu-image` (`.github/workflows/exile-template.yml`) |
| Bootable ISO | no ISO target in `Meta/` or the root build; the only `xorriso` use is the EUK workflow (`.github/workflows/euk.yml`) | Missing |
| OS installer, first-boot setup, recovery environment, hardware compatibility tests | none found; `Documentation/BareMetalInstallation.md` describes a manual `dd` of a built image and states pre-built install images do not exist | Missing |
| VM boot testing | CI runs the self-test in QEMU (`exile-template.yml`, step "Run On-Target Tests", `system_mode=self-test`) and checks `Failed: 0` | Works in principle (not observed here); does not test boot verification |
| Release signing, reproducible builds | none found | Missing |

### 3.8 Build, CI and tests

* `.github/workflows/ci.yml` runs the OS build matrix (GNU and Clang on x86_64, plus aarch64 and riscv64 builds) and the host Lagom build with `ctest`. `euk.yml` covers the separate ExileKernel.
* `Tests/` has about 45 subdirectories, mostly inherited library tests. Userland services and most applications have no tests: in the table in 3.2, only ExileStore, Maps, Spreadsheet and a few others have test files.

## 4. Duplicated or inconsistent

1. XFiles and FileManager both build and install (section 3.5); both are near copies. Bridgeline is built while `Userland/Applications/Browser` is an unbuilt near copy. No removal is made here: decide the product naming first, then retire the inherited name.
2. Boot verification exists in three disconnected places: verifier (`Kernel/Boot/Exile`), host tool (`Meta/Tools`), prekernel hook (`Kernel/EFIPrekernel`). They share sources by design, and the build wiring for the tool and the anchor was inconsistent until the Phase One change.
3. Two trust systems are unrelated: boot manifests (Ed25519, raw 32-byte keys, compiled-in anchors) and Store packages (Ed25519 via OpenSSL, local JSON trust file). That is intentional separation, but a future update service will have to bridge them.
4. Older audits in this directory describe the boot guardian as unsigned and uncalled; they predate the current tree.

## 5. Dependencies on unfinished kernel functionality

* **Application isolation and permissions** (Store permission enforcement, per-app sandboxes): needs kernel enforcement; the existing pledge/unveil model is inherited, and nothing in the userland can replace a missing kernel primitive.
* **Verified boot beyond the kernel image** (initrd, userland, rollback counters): needs prekernel and firmware support, including a monotonic counter facility.
* **Wi-Fi, mobile data, Bluetooth**: need drivers.
* **Disk error reporting, SMART, storage health**: need kernel interfaces that this audit did not find.
* **Secure storage and encryption**: need filesystem and key-storage support.
* **Time synchronisation**: can be a userland service using the existing clock-setting interface; not verified which syscall.

## 6. What can be implemented independently now

1. Boot-trust hardening in host tooling and tests: signing-tool validation, tamper tests in CTest (done in Phase One, section 7 of the report).
2. A tamper **boot** test in QEMU, once the prekernel can be built with verification enabled (needs the target toolchain and a small kernel-side build patch).
3. A signed-update **design** and the Store's missing pieces that are pure userland (signed catalog with freshness, privileged installer service interface, TOCTOU-safe fd hand-off).
4. SystemServer: documented restart-policy contract and tests, plus a dependency declaration format.
5. Logging and health services over existing IPC conventions (`.ipc` files).
6. Time-sync service on top of `ntpquery`.
7. Settings, installer UI and first-boot flow once an installer service exists.
8. Resolving the XFiles/FileManager duplication and the unbuilt `Browser` copy, after a product decision.

## 7. Risks

* **Overclaiming boot security.** The feature is opt-in and unproven on a target. Mitigation: keep `Documentation/BootIntegrity.md` as the single statement of status; do not market verified boot yet.
* **Key handling.** The signing seed is a raw file whose only protection is the operator. Mitigation in this change: tool refuses mismatched and all-zero keys. Remaining: the build does not forbid a seed inside the source tree (proposed patch in the Phase One report).
* **Dual maintenance of forked apps** (section 4).
* **A second trust root.** If an update service is built without a decision on how it chains to the boot anchor, two roots will diverge.
* **Environment gaps.** I could not run the full CI-equivalent here; findings that depend on it are marked.

## 8. Recommended build sequence

1. Phase One: boot and trust foundation (done in this change set; see `PHASE_ONE_REPORT.md`).
2. Land the prekernel build patch, then add a QEMU tamper boot test (needs target toolchain and kernel-owner review).
3. Phase Two services: SystemServer contract and tests, logging, health, time.
4. Phase Three: storage monitoring and mount management over kernel interfaces that exist; networking userland (network manager, DNS, diagnostics) over the existing stack. Wi-Fi waits on drivers.
5. Decide naming for the duplicated applications, then consolidate.
6. Package and update design with the Store's open security items, bridging to the boot trust root.
7. Installer, recovery environment, ISO and release signing, after the services and update design exist.
