#!/usr/bin/env python3
"""Verify a QEMU serial log shows the kernel preempting CPU-bound threads.

Evidence required (all of it, from the same log):
  1. no kernel panic / assertion failure marker,
  2. the TestPreemption line "PREEMPT-TEST: cpus=C threads=T min_count=A max_count=B" with T > C and A > 0,
  3. the "PREEMPT-TEST: complete" line the test prints after its checks, and min_count * 100 >= max_count (the same
     loose fairness bound the test asserts).

A log produced by anything other than a booted kernel (a hand-written file, a host run of the test) satisfies the
parser just as well as a real one, so the verdict only means something when the log came from QEMU running this
kernel. The caller (CI) is responsible for that; this script never claims it.

Exit codes: 0 evidence present, 1 evidence missing or invalid, 2 panic marker found, 3 usage / unreadable file.
Last output line is always "RESULT status=... reason=...".
"""

import re
import sys

PANIC_MARKERS = (
    "KERNEL PANIC",
    "Kernel panic",
    "ASSERTION FAILED",
    "VERIFICATION FAILED",
    "Unhandled exception",
    "Double fault",
)
PREEMPT_RE = re.compile(r"PREEMPT-TEST: cpus=(\d+) threads=(\d+) min_count=(\d+) max_count=(\d+)")
SUITE_RE = re.compile(r"(?m)^TEST-SUITE-RESULT: failed=(\d+)\s*$")
SUITE_MARKER = "TEST-SUITE-RESULT:"
COMPLETED = "PREEMPT-TEST: complete"


def verify(text: str) -> tuple[int, str]:
    for marker in PANIC_MARKERS:
        if marker in text:
            return 2, f"panic marker present: {marker!r}"
    suite_marker_count = text.count(SUITE_MARKER)
    suite_matches = SUITE_RE.findall(text)
    if suite_marker_count == 0:
        return 1, "test-suite result missing (guest test runner did not report its final status)"
    if suite_marker_count != 1 or len(suite_matches) != 1:
        return 1, "test-suite result is duplicated or malformed"
    suite_failures = int(suite_matches[0])
    if suite_failures != 0:
        return 1, f"installed test suite reported {suite_failures} failure(s)"

    matches = PREEMPT_RE.findall(text)
    if not matches:
        return 1, "no PREEMPT-TEST result line (test did not run, hung or boot did not reach it)"
    cpus, threads, minimum, maximum = (int(x) for x in matches[-1])
    if threads <= cpus:
        return 1, f"threads={threads} not greater than cpus={cpus}: run proves nothing about preemption"
    if minimum == 0:
        return 1, "a spinner never ran (min_count=0): starvation"
    if maximum < minimum:
        return 1, "inconsistent counts (max < min)"
    if minimum * 100 < maximum:
        return 1, f"fairness bound violated (min_count={minimum}, max_count={maximum})"
    if COMPLETED not in text:
        return 1, "completion line missing (test aborted before its checks finished)"
    return 0, f"test_suite_failures=0 cpus={cpus} threads={threads} min_count={minimum} max_count={maximum}"


def main(argv: list[str]) -> int:
    if len(argv) != 2:
        print("usage: verify-preemption-serial.py <serial.log>", file=sys.stderr)
        print("RESULT status=USAGE reason=bad-arguments")
        return 3
    try:
        with open(argv[1], encoding="utf-8", errors="replace") as handle:
            text = handle.read()
    except OSError as error:
        print(f"cannot read {argv[1]}: {error}", file=sys.stderr)
        print("RESULT status=USAGE reason=unreadable-log")
        return 3
    code, reason = verify(text)
    status = {0: "PASS", 1: "FAIL", 2: "PANIC"}[code]
    print(f"RESULT status={status} reason={reason}")
    return code


if __name__ == "__main__":
    sys.exit(main(sys.argv))
