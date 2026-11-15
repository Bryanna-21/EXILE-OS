#!/usr/bin/env bash
# Build the Exile OS kernel with the repository's own build system (Meta/exile.sh superbuild + Toolchain/).
#
# This is a checked wrapper, not a second build system. It runs the same CMake/Ninja steps Meta/exile.sh runs, but only as far
# as the Kernel target, and it never reports success unless an up-to-date kernel ELF came out of the build.
#
# Exit codes (shared by all scripts in this directory):
#   0 success                      3 BLOCKED: a prerequisite artifact is missing (cross toolchain, ...)
#   1 usage error                  4 a build step failed
#   2 a required host tool is missing or too old
# The last line of output is always:  RESULT <PASS|FAIL|BLOCKED|USAGE> step=<name> reason=<text>
set -u -o pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"
ARCH="${EXILE_ARCH:-x86_64}"
TOOLCHAIN="${EXILE_TOOLCHAIN:-GNU}"
JOBS="${MAKEJOBS:-}"
OUT="${EXILE_VALIDATION_OUT:-$ROOT/Build/m1-validation}"
CHECK_ONLY=0
REPRODUCIBLE=0

usage() {
    cat <<EOF
Usage: ${0##*/} [--arch x86_64] [--toolchain GNU|Clang] [--jobs N] [--out DIR] [--check-only] [--reproducible]

  --check-only    Validate host tools and the cross toolchain, print what is missing, build nothing.
  --reproducible  Export SOURCE_DATE_EPOCH (commit time of HEAD), TZ=UTC and LC_ALL=C for the build. This only removes
                  timestamp/locale variation; the toolchain itself must also be identical for identical output.
  --out DIR       Where logs and the build summary go (default: Build/m1-validation).
Only x86_64 is validated by this project's tests; other architectures are accepted but unvalidated.
EOF
}

result() { # state step reason exit-code
    echo "RESULT $1 step=$2 reason=$3"
    exit "$4"
}

while [ $# -gt 0 ]; do
    case "$1" in
        --arch) ARCH="${2:-}"; shift 2 || result USAGE args "--arch needs a value" 1 ;;
        --toolchain) TOOLCHAIN="${2:-}"; shift 2 || result USAGE args "--toolchain needs a value" 1 ;;
        --jobs) JOBS="${2:-}"; shift 2 || result USAGE args "--jobs needs a value" 1 ;;
        --out) OUT="${2:-}"; shift 2 || result USAGE args "--out needs a value" 1 ;;
        --check-only) CHECK_ONLY=1; shift ;;
        --reproducible) REPRODUCIBLE=1; shift ;;
        -h|--help) usage; exit 0 ;;
        *) usage >&2; result USAGE args "unknown argument: $1" 1 ;;
    esac
done

case "$ARCH" in x86_64|aarch64|riscv64) ;; *) result USAGE args "unsupported --arch '$ARCH'" 1 ;; esac
case "$TOOLCHAIN" in GNU|Clang) ;; *) result USAGE args "unsupported --toolchain '$TOOLCHAIN'" 1 ;; esac
if [ -n "$JOBS" ] && ! [[ "$JOBS" =~ ^[1-9][0-9]*$ ]]; then result USAGE args "--jobs must be a positive integer" 1; fi
[ "$ARCH" = x86_64 ] || echo "WARNING: $ARCH is not validated by this project's tests" >&2

# ---------------------------------------------------------------- host tools
missing=()
need() { command -v "$1" >/dev/null 2>&1 || missing+=("$1"); }
need git; need cmake; need ninja; need python3; need sha256sum; need file
if [ ${#missing[@]} -gt 0 ]; then
    echo "missing host tools: ${missing[*]}" >&2
    result BLOCKED host-tools "missing: ${missing[*]}" 2
fi

# CMake minimum is declared by the root CMakeLists.txt; read it instead of hard-coding a number.
cmake_min="$(sed -n 's/^cmake_minimum_required(VERSION \([0-9.]*\)).*/\1/p' "$ROOT/CMakeLists.txt" | head -n1)"
cmake_have="$(cmake --version | sed -n '1s/.* \([0-9][0-9.]*\).*/\1/p')"
if [ -n "$cmake_min" ] && [ "$(printf '%s\n%s\n' "$cmake_min" "$cmake_have" | sort -V | head -n1)" != "$cmake_min" ]; then
    result BLOCKED host-tools "cmake $cmake_have is older than the required $cmake_min" 2
fi

# Host compiler: reuse the project's own selection logic (Meta/find_compiler.sh).
# shellcheck source=../../find_compiler.sh
. "$ROOT/Meta/find_compiler.sh"
pick_host_compiler
if [ -z "${CC:-}" ] || [ -z "${CXX:-}" ] || ! is_supported_compiler "$CXX"; then
    result BLOCKED host-tools "no supported host C++ compiler (GCC >= 14 or Clang >= 17)" 2
fi

# ---------------------------------------------------------------- cross toolchain (never downloaded or built here)
if [ "$TOOLCHAIN" = Clang ]; then
    cross_cxx="$ROOT/Toolchain/Local/clang/bin/clang++"
    build_suffix="clang"
    toolchain_cmd="cd Toolchain && ./BuildClang.sh   # or: Meta/exile.sh rebuild-toolchain $ARCH Clang"
else
    cross_cxx="$ROOT/Toolchain/Local/$ARCH/bin/$ARCH-serenity-g++"
    build_suffix=""
    toolchain_cmd="cd Toolchain && ARCH=$ARCH ./BuildGNU.sh   # or: Meta/exile.sh rebuild-toolchain $ARCH GNU"
fi
if [ ! -x "$cross_cxx" ]; then
    {
        echo "cross compiler not found: $cross_cxx"
        echo "Build it once (downloads and compiles binutils/GCC; needs network and 1-3 hours):"
        echo "    $toolchain_cmd"
        echo "or use the pinned container: docker build -t exile-toolchain -f Toolchain/Dockerfile ."
    } >&2
    result BLOCKED cross-toolchain "missing $cross_cxx" 3
fi

echo "host compiler : $CXX ($($CXX -dumpversion))"
echo "cmake         : $cmake_have (required >= ${cmake_min:-?})"
echo "cross compiler: $cross_cxx"
[ "$CHECK_ONLY" = 1 ] && result PASS check-only "host tools and cross toolchain present (nothing built)" 0

# ---------------------------------------------------------------- configure + build
mkdir -p "$OUT" || result FAIL setup "cannot create $OUT" 4
LOG="$OUT/kernel-build.log"
SUPER_DIR="$ROOT/Build/superbuild-$ARCH$build_suffix"
BUILD_DIR="$ROOT/Build/$ARCH$build_suffix"
: > "$LOG"

if [ "$REPRODUCIBLE" = 1 ]; then
    SOURCE_DATE_EPOCH="$(git -C "$ROOT" log -1 --format=%ct 2>/dev/null)"
    [ -n "$SOURCE_DATE_EPOCH" ] || result FAIL reproducible "cannot read the commit time of HEAD" 4
    export SOURCE_DATE_EPOCH TZ=UTC LC_ALL=C
    echo "reproducible  : SOURCE_DATE_EPOCH=$SOURCE_DATE_EPOCH TZ=UTC LC_ALL=C" | tee -a "$LOG"
fi
[ -n "$JOBS" ] || JOBS="$(cmake -P "$ROOT/Meta/CMake/processor-count.cmake" 2>&1 | tr -d '\n')"
case "$JOBS" in ''|*[!0-9]*) JOBS=2 ;; esac

run_step() { # name command...
    local name="$1"; shift
    echo "== $name: $*" | tee -a "$LOG"
    if ! "$@" >>"$LOG" 2>&1; then
        echo "---- last 40 lines of $LOG ----" >&2
        tail -n 40 "$LOG" >&2
        result FAIL "$name" "step failed (full log: $LOG)" 4
    fi
}

export PATH="$ROOT/Toolchain/Local/cmake/bin:$PATH"
if [ ! -f "$BUILD_DIR/build.ninja" ]; then
    # Same configuration Meta/exile.sh uses. Building the 'exile-configure' step builds and installs the host tools (Lagom)
    # the kernel build needs, then configures the target build directory.
    run_step superbuild-configure cmake -GNinja "-DEXILE_TOOLCHAIN=$TOOLCHAIN" "-DEXILE_ARCH=$ARCH" \
        "-DCMAKE_C_COMPILER=$CC" "-DCMAKE_CXX_COMPILER=$CXX" -S "$ROOT/Meta/CMake/Superbuild" -B "$SUPER_DIR"
    run_step host-tools-and-target-configure cmake --build "$SUPER_DIR" --target exile-configure
    [ -f "$BUILD_DIR/build.ninja" ] || result FAIL configure "configure finished but $BUILD_DIR/build.ninja is missing" 4
fi

stamp="$OUT/.kernel-build-start"; : > "$stamp"
run_step kernel-build ninja -j "$JOBS" -C "$BUILD_DIR" Kernel

# ---------------------------------------------------------------- verify the result; no silent success
KERNEL="$BUILD_DIR/Kernel/Kernel"
[ -s "$KERNEL" ] || result FAIL verify "ninja succeeded but $KERNEL is missing or empty" 4
if ! file -b "$KERNEL" | grep -q 'ELF'; then result FAIL verify "$KERNEL is not an ELF file: $(file -b "$KERNEL")" 4; fi
# Fresh build only: an old binary left over from an earlier build must not count. An up-to-date build that rebuilt nothing is
# fine too, so accept either a newer binary or ninja saying there was nothing to do.
if [ ! "$KERNEL" -nt "$stamp" ] && ! grep -q 'no work to do' "$LOG"; then
    result FAIL verify "$KERNEL was not produced by this run" 4
fi
objs=()
for src in Scheduler FinalizerTask; do
    obj="$(find "$BUILD_DIR/Kernel" -name "$src.cpp.o" 2>/dev/null | head -n1)"
    [ -n "$obj" ] || result FAIL verify "no object file for Kernel/Tasks/$src.cpp (modified scheduler sources were not compiled)" 4
    objs+=("$obj")
done
{
    echo "kernel      : $KERNEL"
    echo "kernel size : $(stat -c %s "$KERNEL") bytes"
    echo "kernel sha256: $(sha256sum "$KERNEL" | cut -d' ' -f1)"
    echo "objects     : ${objs[*]}"
    echo "commit      : $(git -C "$ROOT" rev-parse HEAD 2>/dev/null)"
} | tee "$OUT/kernel-build-summary.txt"
result PASS kernel-build "$KERNEL" 0
