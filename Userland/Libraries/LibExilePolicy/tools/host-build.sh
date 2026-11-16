#!/bin/sh
# Builds and tests LibExilePolicy on the host (no kernel, no QEMU needed).
# Usage: tools/host-build.sh [work-directory] [extra cmake args, e.g. -DCMAKE_CXX_FLAGS=-fsanitize=address,undefined]
set -eu
here=$(cd "$(dirname "$0")/.." && pwd)
repo=$(cd "$here/../../.." && pwd)
work=${1:-"${TMPDIR:-/tmp}/exile-policy-host"}
[ $# -gt 0 ] && shift
rm -rf "$work"
mkdir -p "$work/src"
cp -R "$here/." "$work/src/"
cp "$work/src/CMakeLists.host.cmake" "$work/src/CMakeLists.txt"
cmake -S "$work/src" -B "$work/build" -DEXILE_STORE_CORE="$repo/Userland/Applications/ExileStore/src/core" -DEXILE_REPO_ROOT="$repo" "$@"
cmake --build "$work/build" -j
"$work/build/exile_policy_tests"
