# Exile OS: Non-Kernel Integration and Release Readiness

Date: 2026-11-09. Kernel untouched. Nothing committed or pushed.

## 1. Non-kernel subsystem inventory
Exile-specific: Bridgeline, XFiles, ExileStore (Userland/Applications), Meta/Exile/IDENTITY.md,
Meta/CMake/exile_*.cmake, Meta/exile.sh, Tests/ExileStore, .github/workflows/exile-template.yml.
Inherited Serenity: Taskbar, Settings (stock launcher grid), LaunchServer, accounts, networking,
audio/display, installer/recovery tooling. Status per area was assessed by source reading only unless noted.
Existing audits in Documentation/Project-Audits are partly stale (rename drift already fixed).

## 2. Full OS build report
BLOCKED in this environment. No cross toolchain, no QEMU, no xorriso, no network (downloads 403).
Not run: full OS build, ISO, QEMU boot, desktop/service checks. No boot evidence exists.
Finding: documented host compilers (Clang 17-19) fail -Werror on clang 18.1.3 at AK/Time.h:159
(invalid-constexpr). Worked around only for static checks with -Wno-invalid-constexpr; no toolchain change made.

## 3. ISO and QEMU
NOT RUN (blocked as above).

## 4. Ranked outstanding blockers
1. Full cross build + ISO + QEMU boot never executed here; needs a Linux host with the toolchain.
2. Clang 18 documentation/build claim conflicts with Time.h constexpr error (owner decision).
3. 61 app sources not statically checkable (need generated LibWeb/LibGL/WindowServer/RequestServer headers),
   including Bridgeline.
4. Meta/Experiments (LivingBattlefield) fails style lint; kernel-adjacent Python lint failures (left untouched).
5. Owner decisions: keep Browser vs Bridgeline, XFiles vs FileManager.

## 5. Implemented fixes and verification
- XFiles/TrashActions.cpp: compile error (deleted rvalue ByteString::view) fixed. Verified: host clang
  syntax check failed before, passes after.
- Taskbar QuickLaunchWidget: default entries pointed at nonexistent Browser.af; now Bridgeline.af / XFiles.af.
- Meta/lint-app-launchers.py (+ --self-test, 4 mutations detected) wired into Meta/lint-ci.sh; rc=0 after fix, rc=1 before.
- Base/res/apps/Bridgeline.af: executable bit removed (flagged by lint-executable-resources.sh).
- Exile Store host build and tests ran and passed (host build only).

## 6. Application integration report
Static type-check of Userland/Applications with host clang: 146 sources needing generated headers were
rechecked with IPC/GML headers generated: 85 OK, 61 still not checkable, 0 FAIL. Combined with the
earlier pass, the only real failure found was TrashActions.cpp (fixed). Not runtime tested.

## 7. Release readiness
NOT READY to claim a release: build, boot and runtime behaviour are unverified here.
Incident disclosure: an earlier run of Meta/lint-ci.sh in the working tree auto-reformatted ~102 tracked
files (including Kernel/). I restored them with git checkout and re-applied only my intended changes;
Kernel/ shows no modification.
