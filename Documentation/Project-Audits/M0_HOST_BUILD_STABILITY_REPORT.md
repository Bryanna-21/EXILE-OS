# M0 Host Build Stability Report

Status: **the Linux Lagom build completes and the three target test suites pass locally. The full ctest run has 19 failing tests, which are attributed below with differing levels of evidence. GitHub Actions has NOT been observed.** Nothing here concerns the OS image, the prekernel or booting.

## 1. Baseline

- The original pre-merge baseline was `4b2520ed` (PR #8, the Phase One boot-integrity completion package). It did not contain the Phase One closure fixes.
- Phase One closure commit `d9c674a8` was subsequently merged into `main` by merge commit `686eecaf` (PR #9). The merge commit has `4b2520ed` as its first parent and `d9c674a8` as its second parent. The M0 patch in this report is applied to the post-merge `main` state, so it does not need to carry PR #9 again.
- The original Linux build investigation used the pre-merge baseline. The current M0 integration target is `686eecaf`; these are distinct points in repository history.

## 2. Historical failures, reproduced against the pre-merge baseline

Historical reproduction on a clean worktree of the pre-merge baseline `4b2520ed` (the first parent of merge commit `686eecaf`); this is not a claim that the failures remain on current `main`. Command: `cmake -S Meta/Lagom -B <dir> -DBUILD_LAGOM=ON <offline flags>`; `ninja -k 0 lib/liblagom-core.so.0.0.0`; exit 1:

    Userland/Libraries/LibCore/SystemServerTakeover.cpp:41:11: error: no member named 'unsetenv' in namespace 'Core::System'
    Userland/Libraries/LibCore/Process.cpp:532:56: error: too many arguments to function call, expected 0, have 1

Both failures were present on the pre-merge baseline 4b2520ed and were addressed by Phase One closure PR #9, merged into main at 686eecaf. Clang 20.1.2.

## 3. Root causes and fixes

| # | Problem | Root cause | Fix | Where |
|---|---|---|---|---|
| 1 | `System::unsetenv` undeclared | Wrong owner; `Core::Environment::unset` is the API | use it | PR #9 |
| 2 | `Process::get_name(pid_t)` undeclared | Overload never declared or implemented | declared, implemented (Linux `/proc/<pid>/cmdline`; other platforms return `"???"`) | PR #9 |
| 3 | `get_process_pid` never found a live process | `stat` check inverted | corrected | PR #9 |
| 4 | **New:** pid file with `0` or a negative number accepted | `kill(0, 0)` / `kill(-1, 0)` address a process group / all processes, so they look alive | non-positive pid is `EINVAL` | `Process.cpp` |
| 5 | **New:** `LookupServer` does not compile | `UDPServer.h` uses `SocketHandle` (defined in `LibCore/System.h`) without including it; `LibCore` only compiled because `UDPServer.cpp` includes `System.h` first | add `#include <LibCore/System.h>` | `UDPServer.h` |
| 6 | **New:** `LibWebView` does not compile with `ENABLE_PUBLIC_SUFFIX_DOWNLOAD=OFF` | CMake defines `ENABLE_PUBLIC_SUFFIX` as `0`/`1`, code tested `#if defined(...)` (always true), so the missing generated header was included | `#if ENABLE_PUBLIC_SUFFIX` | `URL.cpp` |

Items 5 and 6 were found by the first full `ninja -k 0` of `Build/lagom` (3 failed targets, all of them these). No warnings were silenced for them.

## 4. Tests

`Tests/LibCore/TestLibCoreIPCProcess.cpp` now has 12 cases. Changes against PR #9:
- Removed the fork-then-assume-the-pid-is-not-reused pattern. A dead or nonexistent pid is now `0x7fffffff`, above every platform's pid maximum, so the result does not depend on pid reuse.
- Added: empty, `0`, `-1`, `-12345`, `2147483648`, `12 34`, `0x10` pid files must all return `EINVAL`; a directory at the pid path is an error (works for root and non-root); taking over a socket that was not passed is an error; identity matching against a real live child with a known `argv[0]` (match and mismatch), with the child always killed and reaped and a bounded wait for `exec`.
- Scratch directories are removed by the destructor, including a directory placed at the pid path. No leaked scratch directory or child process after a clean run.
- Mutation check: removing the `<= 0` guard makes the rejection test fail (`lhs=2 rhs=22`); restoring it gives 12/12.
- Not tested: a permission-denied pid file (root bypasses it, so it cannot be deterministic here) and `EPERM` from `kill`. Linux-only cases do not run on macOS, where `get_name(pid)` returns `"???"`.

## 5. Files changed relative to PR #9 (`d9c674a8`)

- `Userland/Libraries/LibCore/Process.cpp` (guard)
- `Userland/Libraries/LibCore/UDPServer.h` (include)
- `Userland/Libraries/LibWebView/URL.cpp` (macro test)
- `Tests/LibCore/TestLibCoreIPCProcess.cpp` (tests)
- `.github/workflows/lagom-template.yml` (the named test step now runs even if the main `Test` step failed: `!cancelled() &&`)
- `Documentation/Project-Audits/M0_HOST_BUILD_STABILITY_REPORT.md` (this file)

`Kernel/` and `ExileKernel/`: no changes (`git diff origin/main -- Kernel ExileKernel` is empty).

## 6. Commands and results

Environment: Linux x86_64 (2 CPUs), clang/clang++ 20.1.2, CMake 3.28.3, Ninja 1.11.1, running as root. The sandbox proxy blocks the dataset hosts, so every download was switched off (flags below). That is a limitation of this environment, not a repository requirement; CI has network access.

    cmake -S Meta/Lagom -B Build/lagom                                     # exit 1 here: CA bundle download, curl.se -> proxy 403
    cmake -S Meta/Lagom -B Build/lagom -G Ninja -DBUILD_LAGOM=ON \
      -DCMAKE_BUILD_TYPE=Release -DCMAKE_C_COMPILER=clang-20 -DCMAKE_CXX_COMPILER=clang++-20 \
      -DENABLE_NETWORK_DOWNLOADS=OFF -DENABLE_CACERT_DOWNLOAD=OFF -DENABLE_UNICODE_DATABASE_DOWNLOAD=OFF \
      -DENABLE_TIME_ZONE_DATABASE_DOWNLOAD=OFF -DENABLE_PUBLIC_SUFFIX_DOWNLOAD=OFF -DENABLE_CLDR_DOWNLOAD=OFF \
      -DENABLE_PNP_IDS_DOWNLOAD=OFF -DENABLE_ADOBE_ICC_PROFILES_DOWNLOAD=OFF -DENABLE_COMMONMARK_SPEC_DOWNLOAD=OFF \
      -DENABLE_LAGOM_LADYBIRD=OFF -DENABLE_GUI_TARGETS=OFF -DCMAKE_CXX_FLAGS=-Wno-error=unused-parameter   # exit 0
    cmake --build Build/lagom              # first run: 3 failed targets (section 3, 5 and 6); after the fixes: exit 0, 3599 targets
    ctest --test-dir Build/lagom --output-on-failure --timeout 900 -j2    # exit 8: 236 of 255 passed, 19 failed (below)
    ctest --test-dir Build/lagom -R 'TestBootGuardian|ExileBootTamperTests|TestLibCoreIPCProcess' --no-tests=error   # exit 0: 3/3

Note: the literal command in the milestone brief, without `-DBUILD_LAGOM=ON`, configures host tools only (no LibCore tests, no ctest suite). CI and `Documentation/RunningTests.md` use `-DBUILD_LAGOM=ON`.

`-Wno-error=unused-parameter` is needed only because `LibUnicode/CharacterTypes.cpp` has unused parameters on the no-data code path (pre-existing, not fixed here).

## 7. The 19 failing tests (full ctest, downloads disabled)

Evidence level A = the failure message points at missing generated data; B = consistent with the environment but not proven; both are unverified against a network-enabled build.

| Test | Observed | Likely cause | Level |
|---|---|---|---|
| TestEmoji, TestIDNA, TestSegmentation, TestUnicodeNormalization, TestUnicodeCharacterTypes, TestString, TestMetrics, TestSegmenter, BenchmarkSegmenter, TestLocale, TestDateTimeFormat, TestLibCoreDateTime, JS, Regex | wrong or empty Unicode/locale/time-zone results (`to_unicode_lowercase(0x3a9)` wrong, `is_unicode(code_point)` fails, time zone `UTC` instead of `GMT`, 0 segmentation boundaries) | Unicode, CLDR and time-zone data disabled | A |
| TestWebViewURL | wrong effective-TLD results | public suffix data disabled (stub returns false) | A |
| TestTLSHandshake | `open: No such file or directory` | CA bundle not downloaded | A |
| TestSFTPStat | `no_permission` case | running as uid 0, so permission checks do not apply | B |
| TestPDF | fixtures found but `Renderer::render` fails | probably the disabled Adobe ICC profiles | B |
| Shell-builtin-redir | `'time' stderr output not redirected correctly`, no `/usr/bin/time` | environment | B |

None of these touch code changed by this branch. They were not compared against a pre-change baseline (the baseline does not compile), so a failure that existed before cannot be separated from an environment effect. They must be re-run with downloads enabled; that is what the GitHub Actions run is for.

## 8. CI

`lagom-template.yml` already configures with `-DBUILD_LAGOM=ON`, builds, and runs full `ctest --timeout 1800` (Linux and macOS, plus a fuzzer leg). The named step added in PR #9 runs the three required suites by name with `--timeout 600 --no-tests=error`; this branch makes it run even when the main test step failed so a boot-manifest regression is not hidden. No coverage was removed.

Open concerns, not changed:
- `ci.yml` gates the Lagom job on `github.repository == 'Bryanna-21/EXILE-OS'`. The remote URL is lower-case (`exile-os`). If the repository's real name does not match that string exactly, the job is skipped, and a skipped job is not a green host build. Check the Actions run.
- `ExileBootTamperTests` has not been run on macOS.

**Evidence from CI: none yet.** The acceptance criterion "GitHub Actions reports a green host CI job" is not met until a run on this branch is seen.

## 9. Remaining limitations

- Full ctest not green here (section 7); needs a network-enabled run.
- `Process::get_name(pid)` is Linux-only; the single-instance check is degraded elsewhere.
- Everything in `Phase One closure` still applies: no OS image, no `Kernel.efi`, no QEMU boot of the prekernel.
