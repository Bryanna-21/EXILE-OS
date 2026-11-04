#!/usr/bin/env bash
# Reproducible host (Lagom) build and test of the Phase One components.
#
# This builds and runs HOST tests only. It does not build the Exile OS image, the EFI prekernel or a cross toolchain,
# and it does not boot anything. A pass here is not evidence of a bootable or verified OS.
#
# Usage: Meta/Exile/build-and-test-host.sh <build-dir> [--offline-data] [--full]
#   --offline-data  disable every dataset download (CA bundle, Unicode, CLDR, time zones, ...). Use this when the network
#                   only reaches package registries. Unicode-dependent tests are then not meaningful, and LibUnicode needs
#                   -Wno-error=unused-parameter (a pre-existing defect on the no-data path, see the closure report).
#   --full          build every target and run the whole ctest suite instead of only the Phase One targets.
#
# Required tools: clang/clang++ >= 20, CMake >= 3.25, Ninja, Python 3. Versions used for the recorded results are in
# Documentation/Project-Audits/PHASE_ONE_CLOSURE_REPORT.md. g++ cannot be used (no C++26 support in g++ 13).
set -euo pipefail
BUILD_DIR="${1:?usage: $0 <build-dir> [--offline-data] [--full]}"; shift || true
OFFLINE=0; FULL=0
for a in "$@"; do case "$a" in --offline-data) OFFLINE=1;; --full) FULL=1;; *) echo "unknown option $a" >&2; exit 1;; esac; done
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
CC="${CC:-$(command -v clang-20 || command -v clang)}"; CXX="${CXX:-$(command -v clang++-20 || command -v clang++)}"
[ -n "$CC" ] && [ -n "$CXX" ] || { echo "clang >= 20 not found" >&2; exit 1; }

echo "== toolchain =="; "$CXX" --version | head -1; cmake --version | head -1; echo "ninja $(ninja --version)"

ARGS=(-S "$ROOT/Meta/Lagom" -B "$BUILD_DIR" -G Ninja -DCMAKE_BUILD_TYPE=Release -DCMAKE_C_COMPILER="$CC" -DCMAKE_CXX_COMPILER="$CXX"
      -DBUILD_LAGOM=ON -DENABLE_LAGOM_LADYBIRD=OFF -DENABLE_GUI_TARGETS=OFF)
if [ "$OFFLINE" -eq 1 ]; then
    ARGS+=(-DENABLE_NETWORK_DOWNLOADS=OFF -DENABLE_CACERT_DOWNLOAD=OFF -DENABLE_UNICODE_DATABASE_DOWNLOAD=OFF
           -DENABLE_TIME_ZONE_DATABASE_DOWNLOAD=OFF -DENABLE_PUBLIC_SUFFIX_DOWNLOAD=OFF -DENABLE_CLDR_DOWNLOAD=OFF
           -DENABLE_PNP_IDS_DOWNLOAD=OFF -DENABLE_ADOBE_ICC_PROFILES_DOWNLOAD=OFF -DENABLE_COMMONMARK_SPEC_DOWNLOAD=OFF
           -DCMAKE_CXX_FLAGS=-Wno-error=unused-parameter)
fi
cmake "${ARGS[@]}"

if [ "$FULL" -eq 1 ]; then
    cmake --build "$BUILD_DIR"
    ctest --test-dir "$BUILD_DIR" --output-on-failure --timeout 1800
else
    cmake --build "$BUILD_DIR" --target ExileBootManifest TestBootGuardian TestLibCoreIPCProcess
    ctest --test-dir "$BUILD_DIR" --output-on-failure --no-tests=error -R 'TestBootGuardian|ExileBootTamperTests|TestLibCoreIPCProcess'
fi
