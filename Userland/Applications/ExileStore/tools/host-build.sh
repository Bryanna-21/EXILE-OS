#!/bin/sh
# Builds and tests the standalone (host) Exile Store out of tree.
#
# The in-tree CMakeLists.txt is the native Exile OS application target. The standalone build lives in
# CMakeLists.host.cmake and expects the Store tree to be the project root, so this script stages a copy of the
# tree in a work directory, uses the host file as its CMakeLists.txt, then builds and runs the tests there.
#
# Usage: tools/host-build.sh [work-directory]

set -eu

here=$(cd "$(dirname "$0")/.." && pwd)
work=${1:-"${TMPDIR:-/tmp}/exile-store-host"}

rm -rf "$work"
mkdir -p "$work/src"
cp -R "$here/." "$work/src/"
cp "$work/src/CMakeLists.host.cmake" "$work/src/CMakeLists.txt"

cmake -S "$work/src" -B "$work/build"
cmake --build "$work/build" -j
(cd "$work/build" && ctest --output-on-failure)

echo "Standalone Exile Store built and tested in $work"
