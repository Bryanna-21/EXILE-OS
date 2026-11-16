#!/usr/bin/env bash
# run-boot-trust-host-checks.sh [--out DIR]   (milestones M1 / M1b, host side; M2 prerequisites)
# Builds the Exile boot-manifest tool and TestBootGuardian directly with the host clang++ (no Lagom configure, which
# needs network downloads), then runs: the 12-case TestBootGuardian, the 53-check tamper suite, the SHA-256 guardian
# cross-check, the key-check harness (M1b) and the 6-scenario fixture matrix. Last line: RESULT <PASS|FAIL|BLOCKED>.
# These are HOST results. Nothing here boots Kernel.efi: M2 (QEMU+OVMF boot) is reported separately as BLOCKED/NOT RUN
# when qemu-system-x86_64, OVMF or a built Kernel.efi is missing.
set -u

# SerenityOS AK/LibCore host tests require a native Linux environment.
# MSYS2/MinGW is not Linux even when Bash and GNU tools are available.
if [ "$(uname -s 2>/dev/null)" != Linux ]; then
    echo "RESULT BLOCKED step=host-platform reason=Linux required; use GitHub Actions"
    exit 3
fi

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"; ROOT="$(cd "$HERE/../../.." && pwd)"
OUT="$ROOT/Build/m2-host"; CXX="${CXX:-clang++}"
while [ $# -gt 0 ]; do case "$1" in --out) OUT="$2"; shift 2 ;; -h|--help) sed -n 2,9p "${BASH_SOURCE[0]}"; exit 0 ;; *) echo "RESULT USAGE step=args reason=$1"; exit 1 ;; esac; done
mkdir -p "$OUT/tool" "$OUT/gen/AK"; N=(); S=(); W=()
rec() { N+=("$1"); S+=("$2"); W+=("$3"); }
for t in "$CXX" python3 cmake bash; do command -v "$t" >/dev/null || { echo "RESULT BLOCKED step=host-tools reason=missing $t"; exit 2; }; done

python3 -I - "$ROOT/AK/Debug.h.in" "$OUT/gen/AK/Debug.h" <<'PY'
import re, sys
s = open(sys.argv[1]).read(); s = re.sub(r'cmakedefine01 (\w+)', r'define \1 0', s); open(sys.argv[2], "w").write(s)
PY
INC=(-I"$ROOT" -I"$OUT/gen" -I"$ROOT/Userland/Libraries" -I"$ROOT/Userland")
CRY=("$ROOT/Kernel/Boot/Exile/BootGuardian.cpp" "$ROOT/Userland/Libraries/LibCrypto/Curves/Curve25519.cpp" "$ROOT/Userland/Libraries/LibCrypto/Curves/Ed25519.cpp" "$ROOT/Userland/Libraries/LibCrypto/Hash/SHA2.cpp")
CORE=(); for f in ArgsParser File System Directory MappedFile Environment Resource DirIterator ResourceImplementation ResourceImplementationFile StandardPaths DirectoryEntry Version; do
    [ -f "$ROOT/Userland/Libraries/LibCore/$f.cpp" ] && CORE+=("$ROOT/Userland/Libraries/LibCore/$f.cpp"); done

"$CXX" -std=c++23 -O1 -DAK_OS_LINUX "${INC[@]}" "$ROOT/Meta/Tools/ExileBootManifest.cpp" "${CRY[@]}" "$ROOT"/AK/*.cpp -o "$OUT/tool/ExileBootManifest" >"$OUT/tool-compile.log" 2>&1
[ $? -eq 0 ] && rec "build ExileBootManifest" PASS "host $CXX, 5 sources + AK" || rec "build ExileBootManifest" FAIL "see tool-compile.log"
"$CXX" -std=c++23 -O1 -DAK_OS_LINUX "${INC[@]}" "$ROOT/Tests/ExileBoot/TestBootGuardian.cpp" "${CRY[@]}" "$ROOT/Userland/Libraries/LibTest/TestSuite.cpp" "$ROOT/Userland/Libraries/LibTest/TestMain.cpp" "${CORE[@]}" "$ROOT"/AK/*.cpp -o "$OUT/tool/TestBootGuardian" >"$OUT/test-compile.log" 2>&1
[ $? -eq 0 ] && rec "build TestBootGuardian" PASS "LibTest + minimal LibCore" || rec "build TestBootGuardian" FAIL "see test-compile.log"

T="$OUT/tool/ExileBootManifest"
if [ -x "$OUT/tool/TestBootGuardian" ]; then ( cd "$OUT/tool" && ./TestBootGuardian ) >"$OUT/test-boot-guardian.log" 2>&1; rc=$?
    [ $rc -eq 0 ] && grep -q "All 12 tests passed" "$OUT/test-boot-guardian.log" && rec "TestBootGuardian (12 cases)" PASS "$(tail -n1 "$OUT/test-boot-guardian.log")" || rec "TestBootGuardian (12 cases)" FAIL "rc=$rc"
else rec "TestBootGuardian (12 cases)" BLOCKED "not built"; fi
if [ -x "$T" ]; then
    bash "$ROOT/Tests/ExileBoot/test-boot-integrity.sh" "$T" >"$OUT/tamper-tests.log" 2>&1; rc=$?
    [ $rc -eq 0 ] && grep -q "passed: 53, failed: 0" "$OUT/tamper-tests.log" && rec "ExileBootTamperTests (53 checks)" PASS "passed: 53" || rec "ExileBootTamperTests (53 checks)" FAIL "rc=$rc, see tamper-tests.log"
    bash "$ROOT/Tests/ExileBoot/test-key-checks.sh" "$T" >"$OUT/key-checks.log" 2>&1; rc=$?
    [ $rc -eq 0 ] && rec "M1b key checks (harness + tool)" PASS "$(tail -n1 "$OUT/key-checks.log")" || rec "M1b key checks (harness + tool)" FAIL "rc=$rc"
    FX="$OUT/fixtures"; rm -rf "$FX"; mkdir -p "$FX"
    bash "$ROOT/Tests/ExileBoot/qemu/make-fixtures.sh" "$T" "$FX" >"$OUT/make-fixtures.log" 2>&1 && bash "$ROOT/Tests/ExileBoot/qemu/check-fixture-matrix.sh" "$T" "$FX" >"$OUT/fixture-matrix.log" 2>&1
    rc=$?; [ $rc -eq 0 ] && rec "fixture matrix (6 scenarios)" PASS "$(tail -n1 "$OUT/fixture-matrix.log")" || rec "fixture matrix (6 scenarios)" FAIL "rc=$rc"
else for n in "ExileBootTamperTests (53 checks)" "M1b key checks (harness + tool)" "fixture matrix (6 scenarios)"; do rec "$n" BLOCKED "tool not built"; done; fi
bash "$ROOT/Tests/ExileBoot/host-guardian-check.sh" >"$OUT/host-guardian-check.log" 2>&1; rc=$?
[ $rc -eq 0 ] && rec "SHA-256 guardian cross-check" PASS "$(tail -n1 "$OUT/host-guardian-check.log")" || rec "SHA-256 guardian cross-check" FAIL "rc=$rc"
# M2: needs QEMU, OVMF and a built Kernel.efi
EFI="${EXILE_KERNEL_EFI:-$ROOT/Build/x86_64/Kernel/EFIPrekernel/Kernel.efi}"; OVMF="${OVMF_CODE:-/usr/share/OVMF/OVMF_CODE_4M.fd}"
miss=""; command -v qemu-system-x86_64 >/dev/null || miss="$miss qemu-system-x86_64"; [ -f "$OVMF" ] || miss="$miss OVMF($OVMF)"; [ -f "$EFI" ] || miss="$miss Kernel.efi(ENABLE_BOOT_VERIFICATION=ON build)"
if [ -n "$miss" ]; then rec "M2 QEMU+OVMF boot (3 cases)" BLOCKED "missing:$miss"
else bash "$ROOT/Tests/ExileBoot/qemu/run-qemu-boot-tests.sh" "$EFI" "$OVMF" >"$OUT/qemu-boot-tests.log" 2>&1; rc=$?; [ $rc -eq 0 ] && rec "M2 QEMU+OVMF boot (3 cases)" PASS "$(tail -n1 "$OUT/qemu-boot-tests.log")" || rec "M2 QEMU+OVMF boot (3 cases)" FAIL "rc=$rc"; fi

printf '%-34s %-8s %s\n' stage state detail | tee "$OUT/summary.txt"; fail=0; block=0
for i in "${!N[@]}"; do printf '%-34s %-8s %s\n' "${N[$i]}" "${S[$i]}" "${W[$i]}" | tee -a "$OUT/summary.txt"; [ "${S[$i]}" = FAIL ] && fail=1; [ "${S[$i]}" = BLOCKED ] && block=1; done
[ $fail -eq 1 ] && { echo "RESULT FAIL step=boot-trust reason=see $OUT/summary.txt"; exit 4; }
[ $block -eq 1 ] && { echo "RESULT BLOCKED step=boot-trust reason=host checks pass; see $OUT/summary.txt"; exit 3; }
echo "RESULT PASS step=boot-trust reason=all stages PASS"; exit 0
