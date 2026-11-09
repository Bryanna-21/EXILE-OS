#!/usr/bin/env bash
# OMNI-Kernel Batch 11 verification.
#
# Steps: required files; toolchain pin check; fmt; clippy; Batch 11 tests (debug, release); IPC and ABI conformance checkers with their self-tests; Batch 9 and 10 tests (regression); Batch 7 and Batch 8 hosted tests (--locked, so their lock files stay untouched); Batch 8's own verify
# script; Zig regression of Batches 2 to 6 (one suite at a time, counts reported per batch); hygiene; protected paths. Package
# integrity (manifest.json, SHA256SUMS) is checked by verify_package.sh at the root of the delivery archive, not here.
#
# Exit status: 0 every executed check passed (SKIPPED checks are never counted as passed); 1 a check FAILED;
# 2 a REQUIRED tool is missing.
#
# Environment:
#   OMNI_RUSTC_EXPECTED   pinned rustc (default 1.91.1); a mismatch is a WARN, not a pass of the pin
#   ZIG                   zig executable (default: zig on PATH); Zig 0.15.2 is the required version
#   CARGO_TARGET_DIR      build directory (default: a temporary directory)
#   OMNI11_PHASE=pre       skip the manifest and SHA256SUMS checks (used while the manifest is being generated)
#   OMNI11_SKIP_B8_VERIFY=1  do not run Batch 8's verify script (it repeats the Zig regression)
#   OMNI11_KERNEL_SYNTAX="x86_64 aarch64"  also run the Kernel/ static syntax check (minutes per architecture; never a build or test)
#   OMNI11_ZIG_CACHE       directory for Zig caches (default: a temporary directory); reuse it to avoid recompiling
#   OMNI_BASE_REF         optional git ref; only the Batch 11 directory and the CI workflow may differ from it

set -u
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT" || exit 1
EXP="$(cd .. && pwd)"

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

echo "== OMNI-Kernel Batch 11 verification =="
echo "root: ."

# ---- 1. required files -------------------------------------------------------
required=(
  Cargo.toml Cargo.lock src/lib.rs src/caps.rs src/ipc.rs src/sys.rs src/status.rs
  tests/caps_unit.rs tests/ipc_e2e.rs
  arch/aarch64-virt/boot.zig arch/aarch64-virt/link.ld
  scripts/verify.sh scripts/check_ipc_conformance.py scripts/build_package.py scripts/qemu_x86_q35.py
  scripts/qemu_aarch64_virt.sh scripts/build_aarch64.sh scripts/kernel_syntax_check.sh
  docs/GAP_MATRIX.md docs/ARCHITECTURE_AND_MIGRATION.md docs/IPC_DESIGN.md docs/THREAT_MODEL_AND_SECURITY_REVIEW.md
  docs/COMPONENT_DISPOSITION.md docs/USERLAND_INTEGRATION.md docs/SUPPORT_MATRIX.md docs/BUILD_AND_TOOLCHAIN.md
  docs/VERIFICATION_REPORT.md
  ../OMNI-Kernel-Batch-10/Cargo.toml ../OMNI-Kernel-Batch-10/toolchain.pin ../OMNI-Kernel-Batch-10/scripts/check_toolchain.py
  ../OMNI-Kernel-Batch-7/Cargo.toml ../OMNI-Kernel-Batch-7/kernel/lib.rs
  ../OMNI-Kernel-Batch-8/Cargo.toml ../OMNI-Kernel-Batch-8/src/lib.rs
  ../OMNI-Kernel-Batch-9/Cargo.toml ../OMNI-Kernel-Batch-9/src/lib.rs ../OMNI-Kernel-Batch-9/scripts/check_abi_conformance.py
  ../OMNI-Kernel-Batch-6/build.zig
)
missing=0
for f in "${required[@]}"; do
  if [ ! -f "$f" ]; then printf 'FAIL    missing required file: %s\n' "$f"; missing=$((missing + 1)); fi
done
if [ "$missing" -eq 0 ]; then ok "all ${#required[@]} required files present"; else fail=$((fail + 1)); fi

# ---- 2. toolchain ------------------------------------------------------------
need=0
for t in rustc cargo python3; do
  command -v "$t" >/dev/null 2>&1 || { echo "TOOL-MISSING  $t"; need=1; }
done
if [ "$need" -ne 0 ]; then
  echo "Build and test steps were NOT run. This is not a test failure."
  printf '\nSummary: %d passed, %d failed, %d skipped, %d warnings; exit 2 (required tool missing)\n' "$pass" "$fail" "$skip" "$warn"
  exit 2
fi
actual="$(rustc --version | awk '{print $2}')"
echo "rustc: $(rustc --version)"
echo "cargo: $(cargo --version)"
if [ "$actual" = "$EXPECTED" ]; then ok "rustc is the pinned $EXPECTED"
else warned "rustc is $actual, not the pinned $EXPECTED; results are NOT evidence for the pinned compiler"; fi
python3 -I ../OMNI-Kernel-Batch-10/scripts/check_toolchain.py >"$TMP/toolchain.log" 2>&1; tc=$?
sed 's/^/        /' "$TMP/toolchain.log"
if [ "$tc" -eq 0 ]; then ok "toolchain matches Batch 10 toolchain.pin"
elif [ "${OMNI11_REQUIRE_PINNED:-}" = 1 ]; then bad "toolchain does not match toolchain.pin (OMNI11_REQUIRE_PINNED=1)"
else warned "toolchain does not match toolchain.pin (set OMNI11_REQUIRE_PINNED=1 to make this fatal)"; fi
if command -v "$ZIG" >/dev/null 2>&1 || [ -x "$ZIG" ]; then
  zv="$("$ZIG" version 2>&1)"; echo "zig:   $zv"
  if [ "$zv" = "0.15.2" ]; then ok "zig is the required 0.15.2"; else warned "zig is $zv, not the required 0.15.2"; fi
fi
if [ "$(uname -m)" != "x86_64" ] || [ "$(uname -s)" != "Linux" ]; then
  echo "NOTE: Batches 7 to 9 run real x86-64 assembly in hosted tests; this host is $(uname -s)/$(uname -m)."
fi

# ---- 3. formatting and lints -------------------------------------------------
if cargo fmt --check >"$TMP/fmt.log" 2>&1; then ok "cargo fmt --check"; else bad "cargo fmt --check"; cat "$TMP/fmt.log"; fi
if cargo clippy --version >/dev/null 2>&1; then
  if cargo clippy --offline --all-targets -- -D warnings >"$TMP/clippy.log" 2>&1; then ok "cargo clippy --all-targets -D warnings (Batch 11 crate)"
  else bad "cargo clippy"; cat "$TMP/clippy.log"; fi
else skipped "cargo clippy (not installed)"; fi

# ---- 4. Batch 10 tests --------------------------------------------------------
count_passed() { grep -E '^test result: ok\. [0-9]+ passed' "$1" | sed -E 's/.* ([0-9]+) passed.*/\1/' | awk '{s+=$1} END{print s+0}'; }
count_ignored() { grep -E '^test result: ' "$1" | sed -E 's/.* ([0-9]+) ignored.*/\1/' | awk '{s+=$1} END{print s+0}'; }
for profile in debug release; do
  flag=""; [ "$profile" = release ] && flag="--release"
  if timeout 900 cargo test --offline --locked $flag >"$TMP/b11-$profile.log" 2>&1; then
    ok "Batch 11 cargo test ($profile): $(count_passed "$TMP/b11-$profile.log") passed, $(count_ignored "$TMP/b11-$profile.log") ignored"
    grep -E '^test result: |Running ' "$TMP/b11-$profile.log" | sed 's/^ *//'
  else bad "Batch 11 cargo test ($profile) (output follows)"; cat "$TMP/b11-$profile.log"; fi
done

# ---- 5. conformance checkers ---------------------------------------------------
for c in "check_ipc_conformance.py:scripts/check_ipc_conformance.py" "ABI conformance:../OMNI-Kernel-Batch-9/scripts/check_abi_conformance.py"; do
  name="${c%%:*}"; path="${c#*:}"; [ "$name" = check_ipc_conformance.py ] && name="IPC conformance"
  if python3 -I "$path" >"$TMP/c.log" 2>&1; then ok "$name: $(tail -n1 "$TMP/c.log")"; else bad "$name"; cat "$TMP/c.log"; fi
  if python3 -I "$path" --self-test >"$TMP/cs.log" 2>&1; then ok "$name self-test: $(tail -n1 "$TMP/cs.log")"; else bad "$name self-test"; cat "$TMP/cs.log"; fi
done

# ---- 6. Batch 7 to 10 regression (hosted) ------------------------------
for b in 7 8 9 10; do
  if timeout 900 cargo test --offline --locked --manifest-path "../OMNI-Kernel-Batch-$b/Cargo.toml" --target-dir "$CARGO_TARGET_DIR/b$b" >"$TMP/b$b.log" 2>&1; then
    ok "Batch $b hosted tests (debug): $(count_passed "$TMP/b$b.log") passed, $(count_ignored "$TMP/b$b.log") ignored"
  else bad "Batch $b hosted tests (output follows)"; tail -n 60 "$TMP/b$b.log"; fi
done
if [ "${OMNI11_SKIP_B8_VERIFY:-}" = 1 ]; then
  skipped "Batch 8 verify.sh (OMNI11_SKIP_B8_VERIFY=1)"
else
  if ( cd ../OMNI-Kernel-Batch-8 && CARGO_TARGET_DIR="$CARGO_TARGET_DIR/b8v" bash scripts/verify.sh ) >"$TMP/b8verify.log" 2>&1; then
    ok "Batch 8 verify.sh: $(grep -E '^Summary:' "$TMP/b8verify.log" | tail -n1)"
    grep -E '^SKIPPED|^WARN' "$TMP/b8verify.log" | sed 's/^/        /'
  else bad "Batch 8 verify.sh (tail follows)"; tail -n 40 "$TMP/b8verify.log"; fi
fi

# ---- 7. Zig regression, Batches 2 to 6, one suite at a time -------------------
if command -v "$ZIG" >/dev/null 2>&1 || [ -x "$ZIG" ]; then
  for b in 2 3 4 5 6; do
    if ( cd "../OMNI-Kernel-Batch-$b" && "$ZIG" build test --summary all \
          --cache-dir "${OMNI11_ZIG_CACHE:-$TMP}/zc$b" --global-cache-dir "${OMNI11_ZIG_CACHE:-$TMP}/zg" ) >"$TMP/zig$b.log" 2>&1; then
      ok "Batch $b Zig tests: $(grep -E 'Build Summary' "$TMP/zig$b.log" | tail -n1)"
    else bad "Batch $b Zig tests (tail follows)"; tail -n 40 "$TMP/zig$b.log"; fi
  done
else
  skipped "Batches 2 to 6 Zig regression (zig not found; set ZIG=/path/to/zig)"
fi

# ---- 7b. Kernel/ static syntax check (opt-in: it takes minutes per architecture) ---
# OMNI11_KERNEL_SYNTAX="x86_64 aarch64" enables it. Static only: never counted as a build or a test.
if [ -n "${OMNI11_KERNEL_SYNTAX:-}" ]; then
  REPO_ROOT="$(cd "$ROOT/../../.." && pwd)"
  for a in $OMNI11_KERNEL_SYNTAX; do
    KARCH="$a" bash scripts/kernel_syntax_check.sh "$REPO_ROOT" </dev/null >"$TMP/ks.log" 2>&1; rc=$?
    last="$(tail -n1 "$TMP/ks.log")"
    case $rc in
      0) ok "Kernel/ syntax check ($a): $last" ;;
      3) skipped "Kernel/ syntax check ($a): $last" ;;
      *) bad "Kernel/ syntax check ($a): $last"; grep -E '^FAIL' -A3 "$TMP/ks.log" | head -n 20 ;;
    esac
  done
else skipped "Kernel/ syntax check (set OMNI11_KERNEL_SYNTAX=\"x86_64 aarch64\" to run)"; fi

# ---- 8. hygiene ------------------------------------------------
if python3 -I - "$ROOT" <<'PY'
# No machine-specific absolute paths, AI-session links, generated-by watermarks or banned colour terms in the package.
import os, re, sys
root = sys.argv[1]
pats = {
    "absolute home path": re.compile("/" + "home/|/" + "Users/|/" + "root/|/" + "tmp/claude"),
    "session link": re.compile("claude" + r"\.ai/code|session" + "_0"),
    "generated-by watermark": re.compile("Generated " + "with|Co-" + "Authored"),
    "purple": re.compile(r"\bpurple\b|\bviolet\b", re.I),
}
bad = 0
for d, _, fs in os.walk(root):
    for f in fs:
        p = os.path.join(d, f)
        rel = os.path.relpath(p, root).replace(os.sep, "/")
        if rel == "scripts/verify.sh":
            continue
        try:
            t = open(p, encoding="utf-8").read()
        except UnicodeDecodeError:
            continue
        for name, rx in pats.items():
            if name == "purple" and not rel.endswith((".md", ".json", ".txt")):
                continue
            m = rx.search(t)
            if m:
                print(f"  {name} in {rel}: {m.group(0)!r}"); bad += 1
sys.exit(1 if bad else 0)
PY
then ok "no absolute paths, session links, watermarks or banned colour terms in the package"; else bad "forbidden content found in the package"; fi

if [ -d target ] || [ -d .zig-cache ] || [ -d zig-out ] || [ -d __pycache__ ] || [ -d scripts/__pycache__ ] || [ -d tests/__pycache__ ]; then
  bad "build artifact or cache directory inside the package"
else ok "no build artifact or cache directories in the package"; fi

if command -v git >/dev/null 2>&1 && git -C "$ROOT" rev-parse --is-inside-work-tree >/dev/null 2>&1; then
  REPO="$(git -C "$ROOT" rev-parse --show-toplevel)"
  if git -C "$REPO" diff --check >/dev/null 2>&1; then ok "git diff --check"; else bad "git diff --check reported whitespace problems"; fi
  # whitespace of untracked package files, which git diff does not see
  if grep -rnE '[[:blank:]]+$' --include='*.rs' --include='*.md' --include='*.py' --include='*.sh' --include='*.toml' . >"$TMP/ws.log" 2>&1; then
    bad "trailing whitespace in package files"; head -n 10 "$TMP/ws.log"
  else ok "no trailing whitespace in package files"; fi
  prot=(Kernel ExileKernel Documentation Meta/Experiments/OMNI-Kernel-Batch-2 Meta/Experiments/OMNI-Kernel-Batch-3
        Meta/Experiments/OMNI-Kernel-Batch-4 Meta/Experiments/OMNI-Kernel-Batch-5 Meta/Experiments/OMNI-Kernel-Batch-6
        Meta/Experiments/OMNI-Kernel-Batch-7 Meta/Experiments/OMNI-Kernel-Batch-8 Meta/Experiments/OMNI-Kernel-Batch-9 Meta/Experiments/OMNI-Kernel-Batch-10)
  dirty="$(git -C "$REPO" status --porcelain -- "${prot[@]}" 2>/dev/null)"
  if [ -z "$dirty" ]; then ok "protected paths (Kernel/, ExileKernel/, Documentation/, Batches 2-10) have no local modifications"
  else bad "protected paths modified:"; echo "$dirty"; fi
  # Nothing may differ from HEAD outside Batch 11 and the CI workflow.
  other="$(git -C "$REPO" status --porcelain --untracked-files=normal | grep -v -E ' (Meta/Experiments/OMNI-Kernel-Batch-11/?|\.github/workflows/?)' || true)"
  if [ -z "$other" ]; then ok "the work tree differs from HEAD only under Meta/Experiments/OMNI-Kernel-Batch-11/ and .github/workflows/"
  else bad "unexpected changes outside Batch 11 and the CI workflow:"; echo "$other"; fi
  if [ -n "${OMNI_BASE_REF:-}" ]; then
    if git -C "$REPO" rev-parse --verify -q "$OMNI_BASE_REF^{commit}" >/dev/null; then
      outside="$(git -C "$REPO" diff --name-only "$OMNI_BASE_REF" | grep -v -E '^(Meta/Experiments/OMNI-Kernel-Batch-11/|\.github/workflows/omni-kernel\.yml)' || true)"
      if [ -z "$outside" ]; then ok "tracked files equal $OMNI_BASE_REF outside Batch 11 and the CI workflow"
      else bad "tracked files differ from $OMNI_BASE_REF:"; echo "$outside"; fi
    else bad "OMNI_BASE_REF '$OMNI_BASE_REF' is not a commit"; fi
  else skipped "comparison with a base ref (set OMNI_BASE_REF to enable)"; fi
else
  skipped "git checks (not inside a git work tree)"
fi

printf '\nSummary: %d passed, %d failed, %d skipped, %d warnings\n' "$pass" "$fail" "$skip" "$warn"
if [ "$fail" -ne 0 ]; then echo "RESULT: FAILED"; exit 1; fi
echo "RESULT: all executed checks passed"
exit 0
