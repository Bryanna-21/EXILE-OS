#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"
build_dir="$(mktemp -d "${TMPDIR:-/tmp}/exile-monitor-tests.XXXXXX")"
trap 'rm -rf "$build_dir"' EXIT
cxx="${CXX:-c++}"
common_flags=(-std=c++20 -Wall -Wextra -Werror -Wpedantic -O2)

"$cxx" "${common_flags[@]}" \
    -I"$repo_root/Userland/Services/HealthMonitor" \
    -I"$repo_root/Userland/Services/MonitoringCommon" \
    "$repo_root/Userland/Services/HealthMonitor/HealthMonitor.cpp" \
    "$repo_root/Tests/Services/Monitoring/HealthMonitorHostTests.cpp" \
    -o "$build_dir/health-monitor-tests"

"$cxx" "${common_flags[@]}" \
    -I"$repo_root/Userland/Services/StorageMonitor" \
    "$repo_root/Userland/Services/StorageMonitor/StorageMonitor.cpp" \
    "$repo_root/Tests/Services/Monitoring/StorageMonitorHostTests.cpp" \
    -o "$build_dir/storage-monitor-tests"

cd "$repo_root"
"$build_dir/health-monitor-tests"
"$build_dir/storage-monitor-tests"
