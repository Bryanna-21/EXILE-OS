#!/usr/bin/env bash
# M1b host test: drives the real Meta/CMake/exile_boot_verification_checks.cmake from a tiny harness project and the
# real ExileBootManifest tool. Proves the seed-in-source-tree refusal and the seed/anchor mismatch refusal. It does
# NOT configure or build the target OS (EFIPrekernel); that part of M1b still needs the cross toolchain.
# Usage: test-key-checks.sh /path/to/ExileBootManifest
set -u
TOOL="${1:-}"; [ -x "$TOOL" ] || { echo "usage: $0 /path/to/ExileBootManifest" >&2; exit 1; }
command -v cmake >/dev/null || { echo "cmake not found" >&2; exit 1; }
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
W="$(mktemp -d)"; trap 'rm -rf "$W"' EXIT
pass=0; fail=0
ok()  { pass=$((pass+1)); echo "PASS: $1"; }
bad() { fail=$((fail+1)); echo "FAIL: $1"; }
"$TOOL" keygen "$W/a.seed" "$W/a.pub" >/dev/null && "$TOOL" keygen "$W/b.seed" "$W/b.pub" >/dev/null || { echo "keygen failed"; exit 1; }
FAKE_SRC="$W/src"; mkdir -p "$FAKE_SRC"; cp "$W/a.seed" "$FAKE_SRC/in-tree.seed"
head -c 31 /dev/zero > "$W/short.key"

harness() { # dir seed pub  -> configure result in $?
    local d="$W/h-$1"; rm -rf "$d"; mkdir -p "$d"
    cat > "$d/CMakeLists.txt" <<CM
cmake_minimum_required(VERSION 3.25)
project(harness NONE)
set(ENABLE_BOOT_VERIFICATION ON)
set(ExileOS_SOURCE_DIR "$FAKE_SRC")
set(BOOT_SIGNING_SEED "$2")
set(BOOT_TRUST_ANCHOR_PUBKEY "$3")
add_custom_target(EFIPrekernel)
add_executable(Lagom::ExileBootManifest IMPORTED)
set_target_properties(Lagom::ExileBootManifest PROPERTIES IMPORTED_LOCATION "$TOOL")
include("$ROOT/Meta/CMake/exile_boot_verification_checks.cmake")
CM
    cmake -S "$d" -B "$d/b" >"$d/configure.log" 2>&1
}

harness inree "$FAKE_SRC/in-tree.seed" "$W/a.pub"
[ $? -ne 0 ] && grep -q "must not be inside the source tree" "$W/h-inree/configure.log" && ok "seed inside the source tree fails at configure" || bad "seed inside the source tree fails at configure"
harness good "$W/a.seed" "$W/a.pub"
[ $? -eq 0 ] && ok "seed outside the tree configures" || bad "seed outside the tree configures"
cmake --build "$W/h-good/b" --target ExileBootKeyCheck >"$W/h-good/build.log" 2>&1
[ $? -eq 0 ] && ok "matching seed and anchor pass ExileBootKeyCheck" || bad "matching seed and anchor pass ExileBootKeyCheck"
harness mismatch "$W/a.seed" "$W/b.pub"
[ $? -eq 0 ] && ok "mismatched pair still configures (the check is a build step)" || bad "mismatched pair configure"
cmake --build "$W/h-mismatch/b" --target ExileBootKeyCheck >"$W/h-mismatch/build.log" 2>&1
[ $? -ne 0 ] && ok "mismatched seed and anchor fail ExileBootKeyCheck" || bad "mismatched seed and anchor fail ExileBootKeyCheck"
"$TOOL" check-keys "$W/a.seed" "$W/b.pub" >/dev/null 2>&1; [ $? -ne 0 ] && ok "tool: check-keys rejects a mismatch" || bad "tool: check-keys rejects a mismatch"
"$TOOL" check-keys "$W/a.seed" "$W/short.key" >/dev/null 2>&1; [ $? -ne 0 ] && ok "tool: check-keys rejects a 31-byte anchor" || bad "tool: check-keys rejects a 31-byte anchor"
"$TOOL" check-keys "$W/a.seed" "$W/a.pub" >/dev/null 2>&1; [ $? -eq 0 ] && ok "tool: check-keys accepts a matching pair" || bad "tool: check-keys accepts a matching pair"
echo; echo "passed: $pass, failed: $fail"; [ "$fail" -eq 0 ]
