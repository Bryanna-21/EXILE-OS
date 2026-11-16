#!/usr/bin/env python3
"""Validate the exact stage inventory and outcome of the boot-trust runner."""

import re
import sys

if len(sys.argv) != 3:
    print(
        "Usage: validate-boot-trust-summary.py SUMMARY_FILE "
        "{PASS|BLOCKED}",
        file=sys.stderr,
    )
    sys.exit(2)

path, expected_m2_state = sys.argv[1:]
expected_m2_state = expected_m2_state.upper()

if expected_m2_state not in {"PASS", "BLOCKED"}:
    print("Expected M2 state must be PASS or BLOCKED.", file=sys.stderr)
    sys.exit(2)

expected_host = {
    "build ExileBootManifest",
    "build TestBootGuardian",
    "TestBootGuardian (12 cases)",
    "ExileBootTamperTests (53 checks)",
    "M1b key checks (harness + tool)",
    "fixture matrix (6 scenarios)",
    "SHA-256 guardian cross-check",
}
expected_m2 = "M2 QEMU+OVMF boot (3 cases)"

stages = []

try:
    with open(path, encoding="utf-8") as stream:
        for line in stream:
            match = re.match(
                r"^(?P<stage>.{34})\s+"
                r"(?P<state>PASS|FAIL|BLOCKED)(?:\s|$)",
                line.rstrip("\n"),
            )
            if match:
                stages.append(
                    (match.group("stage").strip(), match.group("state"))
                )
except OSError as error:
    print(f"::error::Cannot read summary: {error}")
    sys.exit(1)

host_stages = [
    (name, state)
    for name, state in stages
    if not re.match(r"^M2(?:\s|$)", name)
]
m2_stages = [
    (name, state)
    for name, state in stages
    if re.match(r"^M2(?:\s|$)", name)
]

host_names = [name for name, _ in host_stages]
missing = expected_host - set(host_names)
unexpected = set(host_names) - expected_host
duplicates = sorted(
    name for name in set(host_names) if host_names.count(name) > 1
)

if missing or unexpected or duplicates:
    print(
        "::error::Invalid host stage inventory: "
        f"missing={sorted(missing)}, "
        f"unexpected={sorted(unexpected)}, "
        f"duplicates={duplicates}"
    )
    sys.exit(1)

host_failures = [
    (name, state) for name, state in host_stages if state != "PASS"
]

if host_failures:
    for name, state in host_failures:
        print(f"::error::Host stage not passed: {name} [{state}]")
    sys.exit(1)

if m2_stages != [(expected_m2, expected_m2_state)]:
    print(
        "::error::Expected exactly "
        f"{expected_m2!r} {expected_m2_state}; got {m2_stages}"
    )
    sys.exit(1)

if expected_m2_state == "PASS":
    print("All seven host stages and M2 passed.")
else:
    print(
        "All seven host stages passed. "
        "M2 is explicitly BLOCKED; no boot success is claimed."
    )
