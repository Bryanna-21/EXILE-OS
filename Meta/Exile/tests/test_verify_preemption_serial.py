#!/usr/bin/env python3
"""Unit tests for verify-preemption-serial.py. These use SYNTHETIC logs: they test the parser, not the kernel."""

import importlib.util
import os
import subprocess
import sys
import tempfile
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
SCRIPT = os.path.join(HERE, "..", "verify-preemption-serial.py")
spec = importlib.util.spec_from_file_location("vps", SCRIPT)
vps = importlib.util.module_from_spec(spec)
spec.loader.exec_module(vps)

GOOD = (
    "Kernel boot...\n"
    "Running test 'cpu_bound_threads_are_preempted'.\n"
    "PREEMPT-TEST: cpus=1 threads=2 min_count=1000 max_count=1500\n"
    "PREEMPT-TEST: complete\n"
    "TEST-SUITE-RESULT: failed=0\n"
)


class Verify(unittest.TestCase):
    def test_good(self):
        self.assertEqual(vps.verify(GOOD)[0], 0)

    def test_missing_suite_result(self):
        log = GOOD.replace("TEST-SUITE-RESULT: failed=0\n", "")
        self.assertEqual(vps.verify(log)[0], 1)

    def test_nonzero_suite_result(self):
        log = GOOD.replace("failed=0", "failed=2")
        code, reason = vps.verify(log)
        self.assertEqual(code, 1)
        self.assertIn("2 failure", reason)

    def test_malformed_suite_result(self):
        log = GOOD.replace("failed=0", "failed=unknown")
        self.assertEqual(vps.verify(log)[0], 1)

    def test_duplicate_suite_result(self):
        self.assertEqual(vps.verify(GOOD + "TEST-SUITE-RESULT: failed=0\n")[0], 1)

    def test_panic_wins_even_with_good_line(self):
        self.assertEqual(vps.verify(GOOD + "KERNEL PANIC\n")[0], 2)

    def test_assertion_failed(self):
        self.assertEqual(vps.verify("ASSERTION FAILED: x\n" + GOOD)[0], 2)

    def test_missing_line_is_timeout_or_hang(self):
        code, reason = vps.verify("Kernel boot...\nTEST-SUITE-RESULT: failed=0\n")
        self.assertEqual(code, 1)
        self.assertIn("hung", reason)

    def test_threads_not_greater_than_cpus(self):
        log = GOOD.replace("cpus=1 threads=2", "cpus=2 threads=2")
        self.assertEqual(vps.verify(log)[0], 1)

    def test_starved_thread(self):
        log = GOOD.replace("min_count=1000", "min_count=0")
        self.assertEqual(vps.verify(log)[0], 1)

    def test_max_less_than_min(self):
        log = GOOD.replace("max_count=1500", "max_count=10")
        self.assertEqual(vps.verify(log)[0], 1)

    def test_completion_missing(self):
        log = GOOD.replace("PREEMPT-TEST: complete", "Something else")
        self.assertEqual(vps.verify(log)[0], 1)

    def test_fairness_bound(self):
        log = GOOD.replace("max_count=1500", "max_count=100001")
        self.assertEqual(vps.verify(log)[0], 1)

    def test_last_result_line_wins(self):
        bad = "PREEMPT-TEST: cpus=1 threads=1 min_count=5 max_count=5\n"
        self.assertEqual(vps.verify(bad + GOOD)[0], 0)


class Cli(unittest.TestCase):
    def run_cli(self, *args):
        return subprocess.run([sys.executable, "-I", SCRIPT, *args], capture_output=True, text=True)

    def test_usage(self):
        result = self.run_cli()
        self.assertEqual(result.returncode, 3)
        self.assertTrue(result.stdout.strip().splitlines()[-1].startswith("RESULT status=USAGE"))

    def test_unreadable(self):
        self.assertEqual(self.run_cli("/nonexistent/serial.log").returncode, 3)

    def test_end_to_end_pass_and_result_line_last(self):
        with tempfile.NamedTemporaryFile("w", suffix=".log", delete=False) as handle:
            handle.write(GOOD)
        try:
            result = self.run_cli(handle.name)
        finally:
            os.unlink(handle.name)
        self.assertEqual(result.returncode, 0)
        self.assertTrue(result.stdout.strip().splitlines()[-1].startswith("RESULT status=PASS"))


if __name__ == "__main__":
    unittest.main()
