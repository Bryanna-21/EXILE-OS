#!/usr/bin/env bash
# Host test for Kernel/Tasks/SchedulerPriority.h (priority -> ready-queue bucket mapping).
# Reads THREAD_PRIORITY_* from the kernel's own sched.h so the test cannot drift from the real constants.
# Exit: 0 pass, 1 test failed, 2 skipped (no C++ compiler).
set -u
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"
CXX="${CXX:-$(command -v g++ || command -v clang++ || true)}"
[ -n "$CXX" ] || { echo "SKIP: no C++ compiler"; echo "RESULT SKIP"; exit 2; }
H="$ROOT/Kernel/API/POSIX/sched.h"
defs=()
for name in MIN LOW NORMAL HIGH MAX; do
    value="$(sed -n "s/^#define THREAD_PRIORITY_${name} \([0-9][0-9]*\)$/\1/p" "$H")"
    [ -n "$value" ] || { echo "FAIL: cannot read THREAD_PRIORITY_${name} from $H"; echo "RESULT FAIL"; exit 1; }
    defs+=("-DTHREAD_PRIORITY_${name}=${value}")
done
tmp="$(mktemp -d)"; trap 'rm -rf "$tmp"' EXIT
"$CXX" -std=c++23 -Wall -Wextra -Werror "${defs[@]}" -I"$ROOT" \
    "$ROOT/Meta/Exile/tests/sched-priority-map-host-test.cpp" -o "$tmp/test" || { echo "RESULT FAIL (compile or static_assert)"; exit 1; }
"$tmp/test"
