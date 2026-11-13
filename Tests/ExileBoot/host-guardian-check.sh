#!/usr/bin/env bash
# Host-only check of Kernel/Boot/Exile/BootGuardian.cpp. Needs clang++ (or $CXX) and python3.
# Usage: Tests/ExileBoot/host-guardian-check.sh   (run from anywhere)

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "${SCRIPT_DIR}/../.." && pwd)"
CXX="${CXX:-clang++}"
WORK="$(mktemp -d)"
trap 'rm -rf "${WORK}"' EXIT

"${CXX}" -std=c++23 -Wall -Wextra -Werror -D__GCC_DESTRUCTIVE_SIZE=64 -D__GCC_CONSTRUCTIVE_SIZE=64 \
    -I"${ROOT}" "${SCRIPT_DIR}/HostGuardianCheck.cpp" "${ROOT}/Kernel/Boot/Exile/BootGuardian.cpp" -o "${WORK}/check"

"${WORK}/check" | python3 -I -c '
import hashlib, sys

bad = 0
count = 0

for line in sys.stdin:
    parts = line.split()
    if len(parts) != 2:
        print("Malformed digest record:", line.rstrip(), file=sys.stderr)
        sys.exit(1)

    size, digest = parts
    data = bytes((i * 7 + 3) & 255 for i in range(int(size)))
    count += 1

    if hashlib.sha256(data).hexdigest() != digest:
        bad += 1
        print("SHA-256 mismatch at size", size)

print(f"sha256: {count} sizes compared, {bad} mismatches")

if count != 301:
    print(f"Expected 301 digest records, received {count}", file=sys.stderr)
    sys.exit(1)

sys.exit(1 if bad else 0)
\'
