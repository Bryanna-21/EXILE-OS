# Non-kernel system audit: addendum

Adds to, and corrects, `NON_KERNEL_SYSTEM_AUDIT.md`. The underlying audit snapshot is historical: `origin/main` at `d0b61526`. This integration branch starts from `origin/main` at `c8edf3d0`; findings below have not all been independently revalidated against that newer baseline. Evidence labels as in the audit: **Verified by running**, **Read**, **Not verified**.

## Corrections to the first audit

| Audit statement | Correction | Evidence |
|---|---|---|
| "Archive manager: Missing" | Command-line archive tools exist; a graphical archive manager is still missing. | `Userland/Utilities/{zip,unzip,tar,gzip}.cpp` |
| "no system log service" | Correct for a log *service*, but a kernel log reader exists. | `Userland/Utilities/dmesg.cpp`; `Userland/Libraries/LibC/syslog.h` |
| (not mentioned) | The host (Lagom) build fails to compile `LibCore` on Linux on current `main`; see finding 1 below. | Verified by running |

## New finding 1: the host build is broken, so host tests cannot run in CI

* `Userland/Libraries/LibCore/Process.cpp:532` calls `Process::get_name(*pid)`, but the only declaration is `static ErrorOr<String> get_name();` (`Process.h:105`). Compile error: too many arguments.
* `Userland/Libraries/LibCore/SystemServerTakeover.cpp:41` calls `System::unsetenv(...)`, which is declared nowhere in `LibCore` (`grep unsetenv` finds only the call).
* Both errors are signature errors independent of the compiler version. They block every Lagom target that links `LibCore`, including `TestBootGuardian`. The host CI job (`.github/workflows/lagom-template.yml`) cannot pass while they exist, so no host test, including `ExileBootTamperTests`, is currently executed by CI.
* Status: **confirmed, unrelated to boot**. Not fixed here (outside scope; the correct `get_name(pid)` semantics are an owner decision). Recommended action: owners of `LibCore` fix both call sites; then re-run the real `ctest` command in the completion report.

## The requested findings

| # | Finding | Evidence and path | Current status | Recommended action |
|---|---|---|---|---|
| 1 | XFiles and FileManager duplicate each other | Both added at `Userland/Applications/CMakeLists.txt:6` and `:19`; `diff -r` shows 158 differing lines | Confirmed, both built | Product decision on naming; retire one |
| 2 | Browser and Bridgeline duplicate each other | `Userland/Applications/Bridgeline` is built (`CMakeLists.txt:5`); `Userland/Applications/Browser` is in no build file (searched all `CMakeLists.txt` and `*.cmake`); 87 differing lines | Confirmed, but only Bridgeline is built; `Browser` is an orphan source copy | Owners decide whether to remove the orphan |
| 3 | Installer missing | No installer in `Userland/`, `Meta/`, or boot dirs; `Documentation/BareMetalInstallation.md` describes manual `dd` | Confirmed | Phase 7 |
| 4 | ISO target missing | No `grub-mkrescue`, `xorriso`, `mkisofs` or `.iso` in `Meta/*.sh`, `Meta/CMake`, root or `Kernel/` CMake; `xorriso` only in `.github/workflows/euk.yml` | Confirmed | Phase 7 |
| 5 | Recovery environment missing | No recovery code or docs found (search over `Userland`, `Meta`, `Kernel/Boot`, `Kernel/EFIPrekernel`) | Confirmed | Phase 7 |
| 6 | Wi-Fi support missing | `Documentation/BareMetalInstallation.md`: "There is no WiFi support"; no `802.11`/`wlan`/`WiFi` in `Kernel/`, `Userland/Services`, `Userland/Applications` | Confirmed | Blocked on drivers |
| 7 | Firewall missing | No `firewall`/`netfilter`/`nftables` in `Kernel/Net`, `Userland/Services`, `Userland/Utilities` | Confirmed | Needs a kernel packet-filter hook first |
| 8 | System logging missing | No log service in `Userland/Services`; utilities `dmesg`, `logout`, `nologin` only | Confirmed (service); kernel log reader exists | Phase 2 |
| 9 | Time synchronisation missing | Only the one-shot `Userland/Utilities/ntpquery.cpp` | Confirmed | Phase 2 |
| 10 | Update service missing | No updater in `Userland/Services` or `Userland/Applications/Settings` (searched `updater`, `UpdateService`, `self-update`) | Confirmed | Phase 6 |
| 11 | Archive manager missing | See corrections: CLI tools exist, no GUI | Partly confirmed | Phase 5 |
| 12 | Exile Store limitations | `Userland/Applications/ExileStore/docs/SECURITY.md` lists nine, including no sandbox enforcement, no privileged installer, TOCTOU, unsigned catalog, no HTTPS | Confirmed (read) | See roadmap phase 6 |
| 13 | BootGuardian and manifest integration | Verifier and tool: **Verified by running** (12 verifier cases and 53 tamper checks pass through the real `ctest`, see the completion report). Prekernel hook: compiled only with `ENABLE_BOOT_VERIFICATION=ON`, **not compiled** here | Verifier works; integration unproven on target | Build the prekernel with verification and run `Tests/ExileBoot/qemu/run-qemu-boot-tests.sh` |
| 14 | Trust-anchor enforcement | Before: none. Now: `ExileBootManifest sign --anchor` and `check-keys` enforce it in the tool; `Meta/CMake/exile_boot_verification_checks.cmake` runs `check-keys` before `EFIPrekernel` is built, without changing `Kernel/` | Enforced in the tool (**verified by running**); the CMake hook was tested only in a mock project because the OS build is blocked | Review the hook; optionally also apply the `Kernel/` patch in `PREKERNEL-APPROVAL-PATCH/` |
