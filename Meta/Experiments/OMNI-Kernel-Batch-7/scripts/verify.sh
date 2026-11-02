#!/usr/bin/env bash
# OMNI-Kernel Batch 7 verification script.
#
# Runs, in order:
#   1. required-file check
#   2. toolchain check (rustc and cargo present; version compared with the pinned one)
#   3. formatting (cargo fmt --check)
#   4. lints (cargo clippy -D warnings; SKIPPED if clippy is not installed)
#   5. tests: debug and release (these execute the real context-switch assembly on the host)
#   6. bare-metal build for x86_64-unknown-none (SKIPPED if that target is not installed)
#   7. regression: the Batch 6 Zig suite, which also builds the Batch 2-5 suites (SKIPPED without zig)
#   8. whitespace, manifest (including unlisted files), no build artifacts, protected paths unmodified
#
# Exit status:
#   0  every executed check passed (checks that could not run are reported as SKIPPED, never as passed)
#   1  at least one check FAILED
#   2  a REQUIRED tool (rustc, cargo) is missing, so nothing could be built or tested
#
# Environment:
#   OMNI_RUSTC_EXPECTED              pinned rustc version (default: 1.91.1, ExileKernel's pin)
#                                   (a mismatch is only a WARN: the crate needs edition 2024, rustc >= 1.88, and is
#                                   verified on the host target)
#   ZIG                              zig executable for the regression step (default: zig on PATH)
#   OMNI_BASE_REF                    optional git ref to diff against (only this package may change)
#   CARGO_TARGET_DIR                 build output directory (default: a temporary directory)

set -u
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT" || exit 1

EXPECTED="${OMNI_RUSTC_EXPECTED:-1.91.1}"
ZIG="${ZIG:-zig}"
pass=0; fail=0; skip=0; warn=0
ok() { printf 'PASS    %s\n' "$1"; pass=$((pass + 1)); }
bad() { printf 'FAIL    %s\n' "$1"; fail=$((fail + 1)); }
skipped() { printf 'SKIPPED %s\n' "$1"; skip=$((skip + 1)); }
warned() { printf 'WARN    %s\n' "$1"; warn=$((warn + 1)); }

TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$TMP/target}"

echo "== OMNI-Kernel Batch 7 verification =="
echo "root: $ROOT"

# ---- 1. required files -------------------------------------------------------
required=(
  README.md REPORT.md EXILE-OS-STATUS.md manifest.json Cargo.toml Cargo.lock
  kernel/lib.rs kernel/context.rs kernel/sched.rs kernel/syscall.rs kernel/uaccess.rs
  kernel/msr.rs kernel/sync.rs kernel/status.rs kernel/tests.rs
  kernel/arch/x86_64/context.S kernel/arch/x86_64/syscall_entry.S
  docs/ARCHITECTURE.md docs/SYSCALL-INTEGRATION.md docs/SCHEDULER-INTEGRATION.md
  docs/CONTEXT-SWITCHING.md docs/SECURITY-ANALYSIS.md docs/TEST-PLAN.md
  scripts/verify.sh
  ../OMNI-Kernel-Batch-6/build.zig ../OMNI-Kernel-Batch-6/src/tests.zig
)
missing=0
for f in "${required[@]}"; do
  if [ ! -f "$f" ]; then printf 'FAIL    missing required file: %s\n' "$f"; missing=$((missing + 1)); fi
done
if [ "$missing" -eq 0 ]; then ok "all ${#required[@]} required files present"; else fail=$((fail + 1)); fi

# ---- 2. toolchain ------------------------------------------------------------
if ! command -v rustc >/dev/null 2>&1 || ! command -v cargo >/dev/null 2>&1; then
  echo "TOOL-MISSING  rustc and cargo are required (Rust >= 1.88, edition 2024; pin is $EXPECTED)"
  echo "Build and test steps were NOT run. This is not a test failure."
  printf '\nSummary: %d passed, %d failed, %d skipped, %d warnings; exit 2 (required tool missing)\n' "$pass" "$fail" "$skip" "$warn"
  exit 2
fi
actual="$(rustc --version | awk '{print $2}')"
echo "rustc: $(rustc --version)"
echo "cargo: $(cargo --version)"
if [ "$actual" = "$EXPECTED" ]; then ok "rustc is the pinned $EXPECTED"
else warned "rustc is $actual, not the pinned $EXPECTED (ExileKernel/scripts/build.py would refuse it); tests ran on the host target only"; fi
if [ "$(uname -m)" != "x86_64" ] || [ "$(uname -s)" != "Linux" ]; then
  echo "NOTE: the hosted context-switch tests need Linux on x86-64; this host is $(uname -s)/$(uname -m)."
fi

# ---- 3. formatting -----------------------------------------------------------
if cargo fmt --check >"$TMP/fmt.log" 2>&1; then ok "cargo fmt --check"; else bad "cargo fmt --check"; cat "$TMP/fmt.log"; fi

# ---- 4. lints ----------------------------------------------------------------
if cargo clippy --version >/dev/null 2>&1; then
  if cargo clippy --offline --all-targets -- -D warnings >"$TMP/clippy.log" 2>&1; then ok "cargo clippy --all-targets -D warnings"
  else bad "cargo clippy"; cat "$TMP/clippy.log"; fi
else
  skipped "cargo clippy (clippy not installed)"
fi

# ---- 5. tests ----------------------------------------------------------------
for profile in debug release; do
  flag=""; [ "$profile" = release ] && flag="--release"
  if timeout 600 cargo test --offline $flag >"$TMP/test-$profile.log" 2>&1; then
    n="$(grep -E '^test result: ok\. [0-9]+ passed' "$TMP/test-$profile.log" | head -1 | sed -E 's/.* ([0-9]+) passed.*/\1/')"
    ok "cargo test ($profile): ${n:-?} passed"
  else
    bad "cargo test ($profile) (output follows)"; cat "$TMP/test-$profile.log"
  fi
done

# ---- 6. bare-metal build -------------------------------------------------------
sysroot="$(rustc --print sysroot)"
if [ -d "$sysroot/lib/rustlib/x86_64-unknown-none" ]; then
  if cargo build --offline --lib --target x86_64-unknown-none >"$TMP/bare.log" 2>&1; then ok "cargo build --target x86_64-unknown-none"
  else bad "cargo build --target x86_64-unknown-none"; cat "$TMP/bare.log"; fi
else
  skipped "bare-metal build (target x86_64-unknown-none is not installed for this rustc)"
fi

# ---- 7. regression: Batches 2-6 --------------------------------------------------
if command -v "$ZIG" >/dev/null 2>&1 || [ -x "$ZIG" ]; then
  if ( cd ../OMNI-Kernel-Batch-6 && "$ZIG" build test --summary all \
        --cache-dir "$TMP/zig-cache" --global-cache-dir "$TMP/zig-global" ) >"$TMP/zig.log" 2>&1; then
    ok "Batch 6 regression (zig $("$ZIG" version 2>&1)): $(grep -E 'Build Summary' "$TMP/zig.log" | tail -1)"
  else
    bad "Batch 6 regression (output follows)"; cat "$TMP/zig.log"
  fi
else
  skipped "Batch 2-6 regression (zig not found; set ZIG=/path/to/zig)"
fi

# ---- 8. repository hygiene ---------------------------------------------------------
if command -v git >/dev/null 2>&1 && git -C "$ROOT" rev-parse --is-inside-work-tree >/dev/null 2>&1; then
  if git -C "$ROOT" diff --check HEAD -- . >/dev/null 2>&1 || git -C "$ROOT" diff --check >/dev/null 2>&1; then ok "git diff --check"
  else bad "git diff --check reported whitespace problems"; fi
else
  skipped "git diff --check (not inside a git work tree)"
fi

if command -v python3 >/dev/null 2>&1; then
  if python3 -I - "$ROOT" <<'PY'
import hashlib, json, os, sys
root = sys.argv[1]
m = json.load(open(os.path.join(root, "manifest.json"), encoding="utf-8"))
bad = 0
listed = set()
for e in m["files"]:
    p = e["path"]; listed.add(p)
    path = os.path.join(root, p)
    if not os.path.isfile(path):
        print("  missing:", p); bad += 1; continue
    data = open(path, "rb").read()
    if hashlib.sha256(data).hexdigest() != e["sha256"]: print("  sha256 mismatch:", p); bad += 1
    if len(data) != e["bytes"]: print("  size mismatch:", p); bad += 1
actual = set()
for d, _, fs in os.walk(root):
    for f in fs:
        rel = os.path.relpath(os.path.join(d, f), root).replace(os.sep, "/")
        if rel != "manifest.json": actual.add(rel)
for extra in sorted(actual - listed): print("  not in manifest:", extra); bad += 1
sys.exit(1 if bad else 0)
PY
  then ok "manifest.json matches package files"; else bad "manifest.json does not match the package files"; fi
else
  skipped "manifest check (python3 not available)"
fi

if [ -d target ] || [ -d .zig-cache ] || [ -d zig-out ]; then bad "build artifact directory inside the package (target/, .zig-cache/ or zig-out/)"
else ok "no build artifact directories in the package"; fi

if command -v git >/dev/null 2>&1 && git -C "$ROOT" rev-parse --is-inside-work-tree >/dev/null 2>&1; then
  REPO="$(git -C "$ROOT" rev-parse --show-toplevel)"
  prot=(Kernel ExileKernel Documentation/OMNI-Kernel Meta/Experiments/OMNI-Kernel-Batch-2 Meta/Experiments/OMNI-Kernel-Batch-3 Meta/Experiments/OMNI-Kernel-Batch-4 Meta/Experiments/OMNI-Kernel-Batch-5 Meta/Experiments/OMNI-Kernel-Batch-6)
  dirty="$(git -C "$REPO" status --porcelain -- "${prot[@]}" 2>/dev/null)"
  if [ -z "$dirty" ]; then ok "Kernel/, ExileKernel/ and earlier batches have no local modifications"
  else bad "protected paths modified:"; echo "$dirty"; fi
  if [ -n "${OMNI_BASE_REF:-}" ]; then
    if git -C "$REPO" rev-parse --verify -q "$OMNI_BASE_REF^{commit}" >/dev/null; then
      outside="$(git -C "$REPO" diff --name-only "$OMNI_BASE_REF" HEAD | grep -v '^Meta/Experiments/OMNI-Kernel-Batch-7/' || true)"
      if [ -z "$outside" ]; then ok "diff against $OMNI_BASE_REF touches only Meta/Experiments/OMNI-Kernel-Batch-7/"
      else bad "diff against $OMNI_BASE_REF touches other paths:"; echo "$outside"; fi
    else bad "OMNI_BASE_REF '$OMNI_BASE_REF' is not a commit"; fi
  else
    skipped "diff against a base ref (set OMNI_BASE_REF to enable)"
  fi
else
  skipped "protected-path checks (not inside a git work tree)"
fi

printf '\nSummary: %d passed, %d failed, %d skipped, %d warnings\n' "$pass" "$fail" "$skip" "$warn"
if [ "$fail" -ne 0 ]; then echo "RESULT: FAILED"; exit 1; fi
echo "RESULT: all executed checks passed"
exit 0
