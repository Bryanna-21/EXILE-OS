#!/usr/bin/env bash
# OMNI-Kernel Batch 2 verification script.
#
# Runs, in order:
#   1. required-file check
#   2. toolchain check (Zig present, version matches the pinned version)
#   3. formatting check      (zig fmt --check)
#   4. build + run the hosted test target (zig build test)
#   5. manifest integrity check (if manifest.json exists and python3 or sha256sum is available)
#
# Exit status:
#   0  every executed check passed (optional checks that could not run are reported as SKIPPED)
#   1  at least one check FAILED (a real defect: missing file, format error, build/test failure, hash mismatch)
#   2  a REQUIRED tool is missing, so build/test could not be attempted (not a test failure)
#
# Environment:
#   ZIG                                  path to the zig executable (default: zig on PATH)
#   OMNI_ZIG_EXPECTED                    expected `zig version` (default: 0.15.2)
#   OMNI_ALLOW_ZIG_VERSION_MISMATCH=1    downgrade a version mismatch from FAIL to WARN
#
# The script never reports success for a step it did not run.

set -u
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT" || exit 1

ZIG="${ZIG:-zig}"
EXPECTED="${OMNI_ZIG_EXPECTED:-0.15.2}"

pass=0
fail=0
skip=0
warn=0

ok() { printf 'PASS    %s\n' "$1"; pass=$((pass + 1)); }
bad() { printf 'FAIL    %s\n' "$1"; fail=$((fail + 1)); }
skipped() { printf 'SKIPPED %s\n' "$1"; skip=$((skip + 1)); }
warned() { printf 'WARN    %s\n' "$1"; warn=$((warn + 1)); }

echo "== OMNI-Kernel Batch 2 verification =="
echo "root: $ROOT"

# ---- 1. required files -------------------------------------------------------
required=(
  README.md REPORT.md build.zig build.zig.zon
  src/core/error.zig src/core/ids.zig src/core/capabilities.zig
  src/core/resources.zig src/core/memory.zig src/core/ipc.zig
  src/arch/interface.zig src/arch/x86_64/README.md src/arch/aarch64/README.md
  src/tests.zig
  docs/IMPLEMENTATION-PLAN.md docs/CORE-INTERFACES.md docs/TEST-PLAN.md docs/SECURITY-REVIEW.md
  scripts/verify.sh
)
missing=0
for f in "${required[@]}"; do
  if [ ! -f "$f" ]; then
    printf 'FAIL    missing required file: %s\n' "$f"
    missing=$((missing + 1))
  fi
done
if [ "$missing" -eq 0 ]; then ok "all ${#required[@]} required files present"; else fail=$((fail + 1)); fi

# ---- 2. toolchain ------------------------------------------------------------
if ! command -v "$ZIG" >/dev/null 2>&1 && [ ! -x "$ZIG" ]; then
  echo "TOOL-MISSING  required tool not found: '$ZIG' (set ZIG=/path/to/zig, Zig $EXPECTED expected)"
  echo "Build and test steps were NOT run. This is not a test failure."
  printf '\nSummary: %d passed, %d failed, %d skipped, %d warnings; exit 2 (required tool missing)\n' "$pass" "$fail" "$skip" "$warn"
  exit 2
fi

actual="$("$ZIG" version 2>&1)"
echo "zig: $ZIG ($actual)"
if [ "$actual" = "$EXPECTED" ]; then
  ok "zig version is $EXPECTED"
elif [ "${OMNI_ALLOW_ZIG_VERSION_MISMATCH:-0}" = "1" ]; then
  warned "zig version is '$actual', expected '$EXPECTED' (mismatch allowed by OMNI_ALLOW_ZIG_VERSION_MISMATCH=1)"
else
  bad "zig version is '$actual', expected '$EXPECTED'"
fi

# ---- 3. formatting -----------------------------------------------------------
fmt_out="$(mktemp)"
fmt_err="$(mktemp)"
trap 'rm -f "$test_log" "$fmt_out" "$fmt_err"' EXIT
if "$ZIG" fmt --check build.zig build.zig.zon src >"$fmt_out" 2>"$fmt_err"; then
  ok "zig fmt --check"
  if [ -s "$fmt_err" ]; then
    # A Debug-built Zig compiler may print allocator diagnostics about ITSELF on stderr.
    # They are not formatting findings; the exit status above is what counts.
    echo "        (note: zig wrote $(wc -l <"$fmt_err") lines to stderr; exit status was 0)"
  fi
else
  bad "zig fmt --check reported problems (run: $ZIG fmt build.zig build.zig.zon src)"
  cat "$fmt_out" "$fmt_err"
fi

# ---- 4. build and test -------------------------------------------------------
test_log="$(mktemp)"
trap 'rm -f "$test_log" "$fmt_out" "$fmt_err"' EXIT
if "$ZIG" build test --summary all >"$test_log" 2>&1; then
  ok "zig build test"
  cat "$test_log"
else
  bad "zig build test (output follows)"
  cat "$test_log"
fi

# ---- 5. manifest -------------------------------------------------------------
if [ -f manifest.json ]; then
  if command -v python3 >/dev/null 2>&1; then
    if python3 -I - "$ROOT" <<'PY'
import hashlib, json, os, sys
root = sys.argv[1]
m = json.load(open(os.path.join(root, "manifest.json"), encoding="utf-8"))
bad = 0
listed = set()
for e in m["files"]:
    p = e["path"]
    listed.add(p)
    path = os.path.join(root, p)
    if not os.path.isfile(path):
        print("  missing:", p); bad += 1; continue
    data = open(path, "rb").read()
    if hashlib.sha256(data).hexdigest() != e["sha256"]:
        print("  sha256 mismatch:", p); bad += 1
    if len(data) != e["bytes"]:
        print("  size mismatch:", p); bad += 1
actual = set()
for d, _, fs in os.walk(root):
    for f in fs:
        rel = os.path.relpath(os.path.join(d, f), root).replace(os.sep, "/")
        if rel != "manifest.json" and not rel.startswith((".zig-cache/", "zig-out/")):
            actual.add(rel)
for extra in sorted(actual - listed):
    print("  not in manifest:", extra); bad += 1
sys.exit(1 if bad else 0)
PY
    then ok "manifest.json matches package files (checked with python3)"
    else bad "manifest.json does not match the package files"
    fi
  else
    skipped "manifest check (python3 not available; optional tool)"
  fi
else
  skipped "manifest check (manifest.json not present in this tree)"
fi

printf '\nSummary: %d passed, %d failed, %d skipped, %d warnings\n' "$pass" "$fail" "$skip" "$warn"
if [ "$fail" -ne 0 ]; then
  echo "RESULT: FAILED"
  exit 1
fi
echo "RESULT: all executed checks passed"
exit 0
