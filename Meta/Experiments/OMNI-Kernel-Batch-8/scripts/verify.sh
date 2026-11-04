#!/usr/bin/env bash
# OMNI-Kernel Batch 8 verification script.
#
# Runs, in order:
#   1. required-file check
#   2. toolchain check (rustc, cargo, gcc, ld, objdump, objcopy, python3)
#   3. formatting (cargo fmt --check, plus rustfmt --check on the bare-metal kernel source)
#   4. lints (cargo clippy -D warnings; SKIPPED if clippy is not installed)
#   5. hosted tests, debug and release
#   6. kernel image builds for every variant, static image checks and assembly checks,
#      and a two-build reproducibility comparison
#   7. QEMU boot tests (scripts/qemu_test.py): reported NOT RUN / SKIPPED when QEMU or GRUB is
#      missing. A skipped boot test is never a pass.
#   8. regression: the Batch 6 Zig suite (which also builds the Batch 2-5 suites)
#   9. hygiene: whitespace, manifest (including unlisted files), no build artifacts, no
#      machine-specific paths or attribution text in the package, protected paths unmodified
#
# Exit status:
#   0  every executed check passed (checks that could not run are SKIPPED, never counted as passed)
#   1  at least one check FAILED
#   2  a REQUIRED tool is missing, so nothing could be built or tested
#
# Environment:
#   OMNI_RUSTC_EXPECTED  pinned rustc version (default 1.91.1); a mismatch is a WARN
#   ZIG                  zig executable for the regression step (default: zig on PATH)
#   OMNI_BASE_REF        optional git ref; only this package may differ from it
#   CARGO_TARGET_DIR     hosted-test build directory (default: a temporary directory)
#   OMNI8_QEMU, OMNI8_GRUB_MKRESCUE   see scripts/qemu_test.py

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

echo "== OMNI-Kernel Batch 8 verification =="
echo "root: $ROOT"

# ---- 1. required files -------------------------------------------------------
required=(
  README.md REPORT.md EXILE-OS-STATUS.md manifest.json Cargo.toml Cargo.lock
  src/lib.rs src/desc.rs src/idt.rs src/multiboot.rs src/frames.rs src/paging.rs
  src/report.rs src/selftest.rs src/timer.rs src/usermode.rs
  kernel/main.rs link/linker.ld boot/grub.cfg
  arch/x86_64/boot.S arch/x86_64/isr.S arch/x86_64/probe.S
  tests/desc_tests.rs tests/idt_tests.rs tests/paging_tests.rs tests/frames_tests.rs
  tests/multiboot_tests.rs tests/timer_usermode_tests.rs tests/selftest_tests.rs
  tests/expected/base.txt tests/expected/ring3.txt tests/expected/ring3-fault.txt
  tests/expected/trap-fault.txt tests/expected/magic-fault.txt tests/expected/cpu-fault.txt
  scripts/build.sh scripts/check_image.py scripts/check_asm.py scripts/gen_isr.py
  scripts/qemu_test.py scripts/verify.sh
  docs/AUDIT.md docs/BOOT.md docs/GDT-TSS.md docs/IDT-EXCEPTIONS.md docs/MEMORY.md
  docs/PRIVILEGE-TRANSITION.md docs/SCHEDULER.md docs/TIMER.md docs/SYSCALL.md
  docs/SECURITY.md docs/TEST-PLAN.md
  ../OMNI-Kernel-Batch-7/Cargo.toml ../OMNI-Kernel-Batch-7/kernel/lib.rs
  ../OMNI-Kernel-Batch-6/build.zig ../OMNI-Kernel-Batch-6/src/tests.zig
)
missing=0
for f in "${required[@]}"; do
  if [ ! -f "$f" ]; then printf 'FAIL    missing required file: %s\n' "$f"; missing=$((missing + 1)); fi
done
if [ "$missing" -eq 0 ]; then ok "all ${#required[@]} required files present"; else fail=$((fail + 1)); fi

# ---- 2. toolchain ------------------------------------------------------------
need=0
for t in rustc cargo gcc ld objdump objcopy python3; do
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
echo "gcc:   $(gcc --version | head -n1)"
echo "ld:    $(ld --version | head -n1)"
if [ "$actual" = "$EXPECTED" ]; then ok "rustc is the pinned $EXPECTED"
else warned "rustc is $actual, not the pinned $EXPECTED; the kernel image is built with the host-target fallback (docs/BOOT.md)"; fi
if [ "$(uname -m)" != "x86_64" ] || [ "$(uname -s)" != "Linux" ]; then
  echo "NOTE: the image build and hosted tests need Linux on x86-64; this host is $(uname -s)/$(uname -m)."
fi

# ---- 3. formatting -----------------------------------------------------------
if cargo fmt --check >"$TMP/fmt.log" 2>&1 && rustfmt --edition 2024 --check kernel/main.rs >>"$TMP/fmt.log" 2>&1; then ok "cargo fmt --check and rustfmt --check kernel/main.rs"
else bad "formatting"; cat "$TMP/fmt.log"; fi

# ---- 4. lints ----------------------------------------------------------------
if cargo clippy --version >/dev/null 2>&1; then
  if cargo clippy --offline --all-targets -- -D warnings >"$TMP/clippy.log" 2>&1; then ok "cargo clippy --all-targets -D warnings (hosted crate; kernel/main.rs is not linted)"
  else bad "cargo clippy"; cat "$TMP/clippy.log"; fi
else
  skipped "cargo clippy (clippy not installed)"
fi

# ---- 5. hosted tests ---------------------------------------------------------
for profile in debug release; do
  flag=""; [ "$profile" = release ] && flag="--release"
  if timeout 600 cargo test --offline $flag >"$TMP/test-$profile.log" 2>&1; then
    n="$(grep -E '^test result: ok\. [0-9]+ passed' "$TMP/test-$profile.log" | sed -E 's/.* ([0-9]+) passed.*/\1/' | awk '{s+=$1} END{print s}')"
    ok "cargo test ($profile): ${n:-?} passed"
  else
    bad "cargo test ($profile) (output follows)"; cat "$TMP/test-$profile.log"
  fi
done

# ---- 6. image builds and static checks ---------------------------------------
for v in base ring3 ring3-fault trap-fault magic-fault cpu-fault; do
  if scripts/build.sh --variant "$v" --test-exit --out "$TMP/img-$v" >"$TMP/build-$v.log" 2>&1; then
    ok "kernel image builds: variant $v (host-target fallback)"
  else
    bad "kernel image build: variant $v (output follows)"; tail -n 40 "$TMP/build-$v.log"
  fi
done
for v in base ring3; do
  if [ -f "$TMP/img-$v/kernel.elf" ]; then
    if python3 -I scripts/check_image.py "$TMP/img-$v/kernel.elf" --elf32 "$TMP/img-$v/kernel32.elf" >"$TMP/image-$v.log" 2>&1
    then ok "image checks ($v): $(tail -n1 "$TMP/image-$v.log")"
    else bad "image checks ($v)"; grep '^FAIL' "$TMP/image-$v.log"; fi
    if python3 -I scripts/check_asm.py "$TMP/img-$v/kernel.elf" >"$TMP/asm-$v.log" 2>&1
    then ok "assembly checks ($v): $(tail -n1 "$TMP/asm-$v.log")"
    else bad "assembly checks ($v)"; grep '^FAIL' "$TMP/asm-$v.log"; fi
  fi
done
# reproducibility: the same inputs must give byte-identical images
if scripts/build.sh --variant base --test-exit --out "$TMP/img-base-2" >"$TMP/build-base-2.log" 2>&1 \
   && cmp -s "$TMP/img-base/kernel.elf" "$TMP/img-base-2/kernel.elf"; then
  ok "two independent builds of the base image are byte-identical"
else
  bad "two builds of the base image differ (or the second build failed)"
fi
# the non-test-exit image (what an interactive boot uses) must also build
if scripts/build.sh --variant base --out "$TMP/img-plain" >"$TMP/build-plain.log" 2>&1; then ok "kernel image builds without --test-exit"
else bad "kernel image build without --test-exit"; tail -n 20 "$TMP/build-plain.log"; fi
if scripts/build.sh --variant ring3 --profile debug --out "$TMP/img-dbg" >"$TMP/build-dbg.log" 2>&1; then ok "kernel image builds with --profile debug"
else bad "kernel image build with --profile debug"; tail -n 20 "$TMP/build-dbg.log"; fi

# ---- 7. QEMU -----------------------------------------------------------------
if python3 -I scripts/qemu_test.py --selftest >"$TMP/qself.log" 2>&1; then ok "QEMU harness self-test (transcript matcher only; boots nothing)"
else bad "QEMU harness self-test"; cat "$TMP/qself.log"; fi
python3 -I scripts/qemu_test.py --out "$TMP/qemu" >"$TMP/qemu.log" 2>&1
case $? in
  0) ok "QEMU boot tests: $(tail -n1 "$TMP/qemu.log")" ;;
  3) skipped "QEMU boot tests NOT RUN: $(head -n1 "$TMP/qemu.log")" ;;
  *) bad "QEMU boot tests (output follows)"; cat "$TMP/qemu.log" ;;
esac

# ---- 8. regression: Batches 2-6 ----------------------------------------------
if command -v "$ZIG" >/dev/null 2>&1 || [ -x "$ZIG" ]; then
  if ( cd ../OMNI-Kernel-Batch-6 && "$ZIG" build test --summary all \
        --cache-dir "$TMP/zig-cache" --global-cache-dir "$TMP/zig-global" ) >"$TMP/zig.log" 2>&1; then
    ok "Batch 2-6 regression (zig $("$ZIG" version 2>&1)): $(grep -E 'Build Summary' "$TMP/zig.log" | tail -1)"
  else
    bad "Batch 2-6 regression (output follows)"; cat "$TMP/zig.log"
  fi
else
  skipped "Batch 2-6 regression (zig not found; set ZIG=/path/to/zig)"
fi

# ---- 9. repository hygiene ---------------------------------------------------
if command -v git >/dev/null 2>&1 && git -C "$ROOT" rev-parse --is-inside-work-tree >/dev/null 2>&1; then
  if git -C "$ROOT" diff --check HEAD -- . >/dev/null 2>&1 || git -C "$ROOT" diff --check >/dev/null 2>&1; then ok "git diff --check"
  else bad "git diff --check reported whitespace problems"; fi
else
  skipped "git diff --check (not inside a git work tree)"
fi

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
if "manifest.json" in listed: print("  manifest lists itself"); bad += 1
sys.exit(1 if bad else 0)
PY
then ok "manifest.json matches package files (no missing, mismatched or unlisted files)"; else bad "manifest.json does not match the package files"; fi

if python3 -I - "$ROOT" <<'PY'
# No machine-specific absolute paths, AI-session links, generated-by watermarks or purple in the package.
import os, re, sys
root = sys.argv[1]
me = "scripts/verify.sh"
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
        if rel == me:
            continue
        try:
            t = open(p, encoding="utf-8").read()
        except UnicodeDecodeError:
            continue
        for name, rx in pats.items():
            if name == "purple" and not rel.endswith((".md", ".json", ".cfg", ".txt")):
                continue
            m = rx.search(t)
            if m:
                print(f"  {name} in {rel}: {m.group(0)!r}"); bad += 1
sys.exit(1 if bad else 0)
PY
then ok "no absolute paths, session links, watermarks or purple in the package"; else bad "forbidden content found in the package"; fi

if [ -d target ] || [ -d .zig-cache ] || [ -d zig-out ] || [ -d __pycache__ ] || [ -d scripts/__pycache__ ]; then
  bad "build artifact or cache directory inside the package (target/, .zig-cache/, zig-out/, __pycache__/)"
else ok "no build artifact or cache directories in the package"; fi

if command -v git >/dev/null 2>&1 && git -C "$ROOT" rev-parse --is-inside-work-tree >/dev/null 2>&1; then
  REPO="$(git -C "$ROOT" rev-parse --show-toplevel)"
  prot=(Kernel ExileKernel Documentation/OMNI-Kernel Meta/Experiments/OMNI-Kernel-Batch-2 Meta/Experiments/OMNI-Kernel-Batch-3 Meta/Experiments/OMNI-Kernel-Batch-4 Meta/Experiments/OMNI-Kernel-Batch-5 Meta/Experiments/OMNI-Kernel-Batch-6 Meta/Experiments/OMNI-Kernel-Batch-7)
  dirty="$(git -C "$REPO" status --porcelain -- "${prot[@]}" 2>/dev/null)"
  if [ -z "$dirty" ]; then ok "Kernel/, ExileKernel/ and Batches 2-7 have no local modifications"
  else bad "protected paths modified:"; echo "$dirty"; fi
  if [ -n "${OMNI_BASE_REF:-}" ]; then
    if git -C "$REPO" rev-parse --verify -q "$OMNI_BASE_REF^{commit}" >/dev/null; then
      outside="$(git -C "$REPO" diff --name-only "$OMNI_BASE_REF" HEAD | grep -v '^Meta/Experiments/OMNI-Kernel-Batch-8/' || true)"
      if [ -z "$outside" ]; then ok "diff against $OMNI_BASE_REF touches only Meta/Experiments/OMNI-Kernel-Batch-8/"
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
