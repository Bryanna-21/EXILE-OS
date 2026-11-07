#!/usr/bin/env bash
# OMNI-Kernel Batch 9 verification.
#
# Steps: required files; toolchain; fmt; clippy; Batch 9 tests (debug, release); ABI conformance checker and its
# self-test; Batch 7 and Batch 8 hosted tests (--locked, so their lock files stay untouched); Batch 8's own verify
# script; Zig regression of Batches 2 to 6 (one suite at a time, counts reported per batch); manifest and checksum
# integrity; hygiene; protected paths.
#
# Exit status: 0 every executed check passed (SKIPPED checks are never counted as passed); 1 a check FAILED;
# 2 a REQUIRED tool is missing.
#
# Environment:
#   OMNI_RUSTC_EXPECTED   pinned rustc (default 1.91.1); a mismatch is a WARN, not a pass of the pin
#   ZIG                   zig executable (default: zig on PATH); Zig 0.15.2 is the required version
#   CARGO_TARGET_DIR      build directory (default: a temporary directory)
#   OMNI9_PHASE=pre       skip the manifest and SHA256SUMS checks (used while the manifest is being generated)
#   OMNI9_SKIP_B8_VERIFY=1  do not run Batch 8's verify script (it repeats the Zig regression)
#   OMNI9_ZIG_CACHE       directory for Zig caches (default: a temporary directory); reuse it to avoid recompiling
#   OMNI_BASE_REF         optional git ref; only the Batch 8 and Batch 9 directories may differ from it

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

echo "== OMNI-Kernel Batch 9 verification =="
echo "root: ."

# ---- 1. required files -------------------------------------------------------
required=(
  README.md APPLY_BATCH_9.md Cargo.toml Cargo.lock src/lib.rs
  tests/common/mod.rs tests/e2e_memory.rs tests/e2e_syscall_sched.rs tests/e2e_conformance.rs
  scripts/verify.sh scripts/check_abi_conformance.py
  docs/BATCH_1_TO_8_INTEGRATION_AUDIT.md docs/FINAL_KERNEL_ARCHITECTURE.md docs/BATCH_9_VERIFICATION_REPORT.md
  docs/BATCH_9_SECURITY_REVIEW.md docs/OMNI_KERNEL_PROGRAMME_COMPLETION_REPORT.md docs/SUPPORT_MATRIX.md docs/PROPOSALS.md
  ../OMNI-Kernel-Batch-7/Cargo.toml ../OMNI-Kernel-Batch-7/kernel/lib.rs
  ../OMNI-Kernel-Batch-8/Cargo.toml ../OMNI-Kernel-Batch-8/src/lib.rs
  ../OMNI-Kernel-Batch-6/build.zig
)
if [ "${OMNI9_PHASE:-}" != pre ]; then required+=(manifest.json SHA256SUMS); fi
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
  if cargo clippy --offline --all-targets -- -D warnings >"$TMP/clippy.log" 2>&1; then ok "cargo clippy --all-targets -D warnings (Batch 9 crate)"
  else bad "cargo clippy"; cat "$TMP/clippy.log"; fi
else skipped "cargo clippy (not installed)"; fi

# ---- 4. Batch 9 tests --------------------------------------------------------
count_passed() { grep -E '^test result: ok\. [0-9]+ passed' "$1" | sed -E 's/.* ([0-9]+) passed.*/\1/' | awk '{s+=$1} END{print s+0}'; }
count_ignored() { grep -E '^test result: ' "$1" | sed -E 's/.* ([0-9]+) ignored.*/\1/' | awk '{s+=$1} END{print s+0}'; }
for profile in debug release; do
  flag=""; [ "$profile" = release ] && flag="--release"
  if timeout 900 cargo test --offline --locked $flag >"$TMP/b9-$profile.log" 2>&1; then
    ok "Batch 9 cargo test ($profile): $(count_passed "$TMP/b9-$profile.log") passed, $(count_ignored "$TMP/b9-$profile.log") ignored"
    grep -E '^test result: |Running ' "$TMP/b9-$profile.log" | sed 's/^ *//'
  else bad "Batch 9 cargo test ($profile) (output follows)"; cat "$TMP/b9-$profile.log"; fi
done

# ---- 5. ABI conformance ------------------------------------------------------
if python3 -I scripts/check_abi_conformance.py >"$TMP/abi.log" 2>&1; then ok "ABI conformance: $(tail -n1 "$TMP/abi.log")"
else bad "ABI conformance"; cat "$TMP/abi.log"; fi
if python3 -I scripts/check_abi_conformance.py --self-test >"$TMP/abi-self.log" 2>&1; then ok "ABI conformance self-test: $(tail -n1 "$TMP/abi-self.log")"
else bad "ABI conformance self-test"; cat "$TMP/abi-self.log"; fi

# ---- 6. Batch 7 and Batch 8 regression (hosted) ------------------------------
for b in 7 8; do
  if timeout 900 cargo test --offline --locked --manifest-path "../OMNI-Kernel-Batch-$b/Cargo.toml" --target-dir "$CARGO_TARGET_DIR/b$b" >"$TMP/b$b.log" 2>&1; then
    ok "Batch $b hosted tests (debug): $(count_passed "$TMP/b$b.log") passed, $(count_ignored "$TMP/b$b.log") ignored"
  else bad "Batch $b hosted tests (output follows)"; tail -n 60 "$TMP/b$b.log"; fi
done
if [ "${OMNI9_SKIP_B8_VERIFY:-}" = 1 ]; then
  skipped "Batch 8 verify.sh (OMNI9_SKIP_B8_VERIFY=1)"
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
          --cache-dir "${OMNI9_ZIG_CACHE:-$TMP}/zc$b" --global-cache-dir "${OMNI9_ZIG_CACHE:-$TMP}/zg" ) >"$TMP/zig$b.log" 2>&1; then
      ok "Batch $b Zig tests: $(grep -E 'Build Summary' "$TMP/zig$b.log" | tail -n1)"
    else bad "Batch $b Zig tests (tail follows)"; tail -n 40 "$TMP/zig$b.log"; fi
  done
else
  skipped "Batches 2 to 6 Zig regression (zig not found; set ZIG=/path/to/zig)"
fi

# ---- 8. integrity and hygiene ------------------------------------------------
if [ "${OMNI9_PHASE:-}" = pre ]; then
  skipped "manifest.json and SHA256SUMS checks (OMNI9_PHASE=pre)"
else
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
        if rel not in ("manifest.json", "SHA256SUMS"): actual.add(rel)
for extra in sorted(actual - listed): print("  not in manifest:", extra); bad += 1
for self_listed in ("manifest.json", "SHA256SUMS"):
    if self_listed in listed: print("  manifest lists", self_listed); bad += 1
sums = {}
for line in open(os.path.join(root, "SHA256SUMS"), encoding="utf-8"):
    h, _, p = line.rstrip("\n").partition("  ")
    sums[p] = h
want = {e["path"]: e["sha256"] for e in m["files"]}
want["manifest.json"] = hashlib.sha256(open(os.path.join(root, "manifest.json"), "rb").read()).hexdigest()
if sums != want: print("  SHA256SUMS differs from manifest (plus manifest.json)"); bad += 1
sys.exit(1 if bad else 0)
PY
  then ok "manifest.json and SHA256SUMS match the package files (no missing, mismatched or unlisted files)"
  else bad "manifest.json or SHA256SUMS does not match the package files"; fi
fi

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
        Meta/Experiments/OMNI-Kernel-Batch-7)
  dirty="$(git -C "$REPO" status --porcelain -- "${prot[@]}" 2>/dev/null)"
  if [ -z "$dirty" ]; then ok "protected paths (Kernel/, ExileKernel/, Documentation/, Batches 2-7) have no local modifications"
  else bad "protected paths modified:"; echo "$dirty"; fi
  # Batch 8 may be present as staged dependency; nothing else may differ outside Batch 9.
  other="$(git -C "$REPO" status --porcelain --untracked-files=normal | grep -v -E ' Meta/Experiments/OMNI-Kernel-Batch-(8|9)/?' || true)"
  if [ -z "$other" ]; then ok "the work tree differs from HEAD only under Meta/Experiments/OMNI-Kernel-Batch-8/ and -9/"
  else bad "unexpected changes outside Batch 8 and Batch 9:"; echo "$other"; fi
  if [ -n "${OMNI_BASE_REF:-}" ]; then
    if git -C "$REPO" rev-parse --verify -q "$OMNI_BASE_REF^{commit}" >/dev/null; then
      outside="$(git -C "$REPO" diff --name-only "$OMNI_BASE_REF" | grep -v -E '^Meta/Experiments/OMNI-Kernel-Batch-(8|9)/' || true)"
      if [ -z "$outside" ]; then ok "tracked files equal $OMNI_BASE_REF outside Batch 8 and Batch 9"
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
