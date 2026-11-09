#!/usr/bin/env bash
# Syntax and type check (no code generation, no linking, no execution) of Kernel/ C++ translation units for x86-64 using a HOST clang >= 18.
#
# This is a STATIC CHECK ONLY. It is not the project toolchain, not a build, and not a test. It exists so changes to Kernel/ can at
# least be parsed and type-checked where the full cross toolchain is unavailable. It generates the configuration headers
# (AK/Debug.h, Kernel/Debug.h, Kernel/Version.h) into a temporary directory with every optional debug switch OFF; the real build
# generates them from CMake options, so code behind an enabled switch is not checked.
#
# usage: kernel_syntax_check.sh [REPO_ROOT] [FILE.cpp ...]     (default: every Kernel/**/*.cpp except the other architecture's directory, riscv64, the prekernels and the two kernel-coverage (KCOV) files, which only build when that option is on)
# env:   CLANGXX (default clang++); KARCH=x86_64 (default) or aarch64
# exit:  0 all files passed, 1 at least one file failed, 3 NOT RUN (no suitable clang or libstdc++ headers)
set -u
REPO="${1:-$(cd "$(dirname "${BASH_SOURCE[0]}")/../../../.." && pwd)}"; shift || true
CLANGXX="${CLANGXX:-clang++}"
KARCH="${KARCH:-x86_64}"
case "$KARCH" in
  x86_64) ARCHFLAGS=(-target x86_64-unknown-none-elf -mno-red-zone -mno-sse -mno-sse2 -mno-mmx -mno-80387); OTHER=aarch64 ;;
  aarch64) ARCHFLAGS=(-target aarch64-unknown-none-elf -mstrict-align); OTHER=x86_64 ;;
  *) echo "unsupported KARCH=$KARCH"; exit 2 ;;
esac
command -v "$CLANGXX" >/dev/null 2>&1 || { echo "NOT RUN: $CLANGXX not found"; exit 3; }
major="$("$CLANGXX" --version | sed -n 's/.*clang version \([0-9]*\).*/\1/p' | head -n1)"
[ "${major:-0}" -ge 18 ] || { echo "NOT RUN: clang >= 18 required (deducing this), found '${major:-none}'"; exit 3; }
CXXINC="$(ls -d /usr/include/c++/[0-9]* 2>/dev/null | sort -V | tail -n1)"
[ -n "$CXXINC" ] || { echo "NOT RUN: libstdc++ headers not found under /usr/include/c++"; exit 3; }
GCCVER="$(basename "$CXXINC")"
RES="$("$CLANGXX" -print-resource-dir)"
GEN="$(mktemp -d)"; trap 'rm -rf "$GEN"' EXIT
mkdir -p "$GEN/Kernel" "$GEN/AK" "$GEN/stubs/gnu"
: > "$GEN/stubs/gnu/stubs-32.h"   # host glibc headers are x86-64 specific; only needed when checking aarch64
for pair in "Kernel/Debug.h.in:Kernel/Debug.h" "AK/Debug.h.in:AK/Debug.h"; do
  sed -E 's/^#( *)cmakedefine01 ([A-Za-z_0-9]+)/#\1define \2 0/; s/^#( *)cmakedefine ([A-Za-z_0-9]+)(.*)/\/\/ \2 left undefined/' "$REPO/${pair%%:*}" > "$GEN/${pair##*:}" || exit 1
done
sh "$REPO/Kernel/generate-version-file.sh" "$GEN/Kernel/Version.h" || exit 1
FLAGS=(-std=c++23 -fsyntax-only -w -nostdinc -nostdinc++ -isystem "$RES/include" -isystem "$CXXINC"
  -isystem "/usr/include/x86_64-linux-gnu/c++/$GCCVER" -idirafter /usr/include -idirafter /usr/include/x86_64-linux-gnu -idirafter "$GEN/stubs"
  -fsized-deallocation -Wno-error=invalid-constexpr -ffreestanding -fno-exceptions -fno-rtti "${ARCHFLAGS[@]}" -D__serenity__ -DKERNEL -DSANITIZE_PTRS "-DDISALLOW(m)="
  -I"$REPO" -I"$GEN" -I"$REPO/Userland/Libraries/LibC" -I"$REPO/Kernel" -I"$REPO/Userland/Libraries" -I"$REPO/Userland")
cd "$REPO" || exit 1
if [ "$#" -gt 0 ]; then printf '%s\n' "$@" > "$GEN/list"
else find Kernel -name '*.cpp' -not -path "*/$OTHER/*" -not -path '*/riscv64/*' -not -path '*/Prekernel/*' -not -path '*/EFIPrekernel/*' -not -name KCOVDevice.cpp -not -name SanCov.cpp | sort > "$GEN/list"; fi
total=$(wc -l < "$GEN/list")
while IFS= read -r f; do
  log="$GEN/log.$(echo "$f" | tr / _)"
  if "$CLANGXX" "${FLAGS[@]}" "$f" >"$log" 2>&1 </dev/null; then echo "OK   $f"
  elif grep -q "EscapeSequenceStateMachine.h' file not found" "$log"; then echo "SKIP $f (needs LibVT's generated state-machine header; not checked)"
  else echo "FAIL $f"; grep -m3 -E 'error' "$log" | sed 's/^/     /'; fi
done < "$GEN/list" | tee "$GEN/results"
ok=$(grep -c '^OK ' "$GEN/results"); fail=$(grep -c '^FAIL ' "$GEN/results"); skip=$(grep -c '^SKIP ' "$GEN/results")
echo "kernel_syntax_check: $ok passed, $fail failed, $skip not checked, of $total (static check only)"
[ "$fail" -eq 0 ]
