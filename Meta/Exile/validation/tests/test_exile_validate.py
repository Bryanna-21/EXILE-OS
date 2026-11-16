#!/usr/bin/env python3
"""Pure unit tests for exile_validate.py. Standard library only.

These tests validate parsers, metadata contracts, and synthetic boot classification.
They do not invoke kernel builds, create ISOs, or launch QEMU.
Actual build and runtime validation belongs on the Linux CI runner.
"""
import hashlib
import json
import os
import re
import stat
import struct
import subprocess
import sys
import tempfile
import unittest

sys.dont_write_bytecode = True
HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, os.path.join(HERE, ".."))
import exile_validate as ev  # noqa: E402

REPO = ev.REPO
B8 = os.path.join(REPO, ev.B8)
QT = ev.load_module(os.path.join(B8, "scripts", "qemu_test.py"), "qt_under_test")


def expected(variant):
    with open(os.path.join(B8, "tests", "expected", variant + ".txt")) as f:
        return f.read()


CONCRETE = {
    "~OMNI8   page-fault: read protection user cr2=0x0*100000": "OMNI8   page-fault: read protection user cr2=0x100000",
    "~OMNI8   rip=0x[0-9a-f]+ cs=0x0023 ring3 rflags=.*": "OMNI8   rip=0x401000 cs=0x0023 ring3 rflags=0x202",
    "~OMNI8 FATAL EXCEPTION vector=14 name=#PF page fault err=0x[0-9a-f]+": "OMNI8 FATAL EXCEPTION vector=14 name=#PF page fault err=0x5",
    "~OMNI8 FATAL EXCEPTION vector=6 name=#UD invalid opcode err=0x0+$": "OMNI8 FATAL EXCEPTION vector=6 name=#UD invalid opcode err=0x0",
    "~OMNI8 MEM: map parsed regions=[0-9]+ usable_ranges=[0-9]+": "OMNI8 MEM: map parsed regions=5 usable_ranges=2",
}


def transcript(variant):
    """A concrete transcript satisfying the real expected file (regex lines instantiated by hand)."""
    _, lines, _ = QT.parse_expected(expected(variant))
    out = []
    for l in lines:
        if l.startswith("~"):
            c = CONCRETE[l]
            assert QT.line_ok(l, c), (l, c)
            l = c
        out.append(l)
    return "\n".join(out) + "\n"


def make_elf(flags=5, mb=True, mb_off=64 + 56, cksum_ok=True, entry_ok=True, wx=False, cls=2, mb_flags=3):
    seg_flags = 7 if wx else flags
    base = 0x100000
    data = bytearray(4096)
    ident = b"\x7fELF" + bytes([cls, 1, 1]) + b"\0" * 9
    entry = base + 0x100 if entry_ok else 0x9000000
    data[0:16] = ident
    struct.pack_into("<HHIQQQIHHHHHH", data, 16, 2, 62, 1, entry, 64, 0, 0, 64, 56, 1, 0, 0, 0)
    struct.pack_into("<IIQQQQQQ", data, 64, 1, seg_flags, 0, base, base, 4096, 4096, 4096)
    if mb:
        cs = (-(0x1BADB002 + mb_flags)) & 0xFFFFFFFF if cksum_ok else 1
        struct.pack_into("<III", data, mb_off, 0x1BADB002, mb_flags, cs)
    return bytes(data)


def failed(checks):
    return {n for n, ok, _ in checks if not ok}


class ElfMultiboot(unittest.TestCase):
    def test_good(self):
        self.assertEqual(failed(ev.validate_elf_multiboot(make_elf())), set())

    def test_bad_magic(self):
        self.assertIn("ELF magic", failed(ev.validate_elf_multiboot(b"MZ" + make_elf()[2:])))

    def test_elf32(self):
        self.assertIn("ELF64 little-endian", failed(ev.validate_elf_multiboot(make_elf(cls=1))))

    def test_missing_multiboot(self):
        self.assertTrue(any("Multiboot1 magic" in n for n in failed(ev.validate_elf_multiboot(make_elf(mb=False)))))

    def test_bad_checksum(self):
        self.assertIn("Multiboot1 checksum", failed(ev.validate_elf_multiboot(make_elf(cksum_ok=False))))

    def test_header_past_8192(self):
        data = bytearray(make_elf(mb=False) + b"\0" * 12000)
        struct.pack_into("<III", data, 9000, 0x1BADB002, 3, (-(0x1BADB002 + 3)) & 0xFFFFFFFF)
        self.assertTrue(any("Multiboot1 magic" in n for n in failed(ev.validate_elf_multiboot(bytes(data)))))

    def test_unaligned_header_not_found(self):
        self.assertTrue(any("Multiboot1 magic" in n for n in failed(ev.validate_elf_multiboot(make_elf(mb_off=64 + 56 + 2)))))

    def test_wx_segment(self):
        self.assertIn("no writable+executable segment", failed(ev.validate_elf_multiboot(make_elf(wx=True))))

    def test_entry_outside(self):
        self.assertIn("entry point inside an executable PT_LOAD", failed(ev.validate_elf_multiboot(make_elf(entry_ok=False))))

    def test_entry_in_non_exec_segment(self):
        self.assertIn("entry point inside an executable PT_LOAD", failed(ev.validate_elf_multiboot(make_elf(flags=6))))

    def test_truncated(self):
        self.assertTrue(failed(ev.validate_elf_multiboot(make_elf()[:40])))

    def test_load_segment_file_range_outside_file(self):
        data = bytearray(make_elf())
        struct.pack_into("<Q", data, 64 + 8, 4090)
        self.assertIn("PT_LOAD segment bounds and sizes valid",
                      failed(ev.validate_elf_multiboot(bytes(data))))

    def test_load_segment_filesz_exceeds_memsz(self):
        data = bytearray(make_elf())
        struct.pack_into("<Q", data, 64 + 32, 4096)
        struct.pack_into("<Q", data, 64 + 40, 1)
        self.assertIn("PT_LOAD segment bounds and sizes valid",
                      failed(ev.validate_elf_multiboot(bytes(data))))

    def test_load_segment_address_overflow(self):
        data = bytearray(make_elf())
        struct.pack_into("<Q", data, 64 + 16, 0xFFFFFFFFFFFFFFF0)
        self.assertIn("PT_LOAD segment bounds and sizes valid",
                      failed(ev.validate_elf_multiboot(bytes(data))))

    def test_load_segment_invalid_alignment(self):
        data = bytearray(make_elf())
        struct.pack_into("<Q", data, 64 + 48, 3)
        self.assertIn("PT_LOAD segment bounds and sizes valid",
                      failed(ev.validate_elf_multiboot(bytes(data))))


class ClassifyBoot(unittest.TestCase):
    def cls(self, variant, status, serial=None, qlog="", timed_out=False):
        s = transcript(variant) if serial is None else serial
        return ev.classify_boot(variant, status, timed_out, s, qlog, expected(variant), QT)

    def test_every_variant_pass_with_its_real_transcript(self):
        for v in ev.VARIANTS:
            want = 33 if v in ("base", "ring3", "preempt") else 35
            r = self.cls(v, want)
            self.assertEqual(r["status"], ev.PASS, (v, r["reasons"]))
            self.assertTrue(all(m["reached"] for m in r["milestones"]), v)

    def test_wrong_status_fails(self):
        self.assertEqual(self.cls("base", 35)["status"], ev.FAIL)
        self.assertEqual(self.cls("magic-fault", 33)["status"], ev.FAIL)

    def test_status_zero_is_unexpected(self):
        r = self.cls("base", 0)
        self.assertEqual(r["status"], ev.FAIL)
        self.assertTrue(r["flags"]["unexpected_exit"])

    def test_timeout(self):
        r = self.cls("base", None, timed_out=True)
        self.assertEqual(r["status"], ev.FAIL)
        self.assertTrue(r["flags"]["timeout"])

    def test_timeout_even_with_complete_transcript(self):
        self.assertEqual(self.cls("base", 33, timed_out=True)["status"], ev.FAIL)

    def test_timeout_with_repeated_cpu_resets_sets_reset_flag(self):
        r = self.cls("base", None, "OMNI8 BOOT: start\\n",
                     "CPU Reset (CPU 0)\\nCPU Reset (CPU 0)\\n",
                     timed_out=True)
        self.assertEqual(r["status"], ev.FAIL)
        self.assertTrue(r["flags"]["timeout"])
        self.assertTrue(r["flags"]["reset"])

    def test_timeout_with_triple_fault_sets_reset_flag(self):
        r = self.cls("base", None, "OMNI8 BOOT: start\\n",
                     "Triple fault\\n", timed_out=True)
        self.assertEqual(r["status"], ev.FAIL)
        self.assertTrue(r["flags"]["reset"])

    def test_timeout_with_single_startup_reset_is_not_reset_fault(self):
        r = self.cls("base", None, "OMNI8 BOOT: start\\n",
                     "CPU Reset (CPU 0)\\n", timed_out=True)
        self.assertEqual(r["status"], ev.FAIL)
        self.assertTrue(r["flags"]["timeout"])
        self.assertFalse(r["flags"]["reset"])

    def test_panic_in_passing_variant(self):
        s = transcript("base") + "OMNI8 PANIC: boom\n"
        r = self.cls("base", 33, s)
        self.assertEqual(r["status"], ev.FAIL)
        self.assertTrue(r["flags"]["panic"])

    def test_triple_fault_reset(self):
        r = self.cls("base", 0, "OMNI8 BOOT: start\n", "CPU Reset (CPU 0)\nCPU Reset (CPU 0)\ncheck_exception old: 0x8 new 0xb\nTriple fault\n")
        self.assertEqual(r["status"], ev.FAIL)
        self.assertTrue(r["flags"]["reset"])

    def test_triple_fault_even_when_exit_looks_ok(self):
        r = self.cls("base", 33, qlog="Triple fault\n")
        self.assertEqual(r["status"], ev.FAIL)

    def test_single_startup_reset_is_not_a_fault(self):
        self.assertEqual(self.cls("base", 33, qlog="CPU Reset (CPU 0)\n")["status"], ev.PASS)

    def test_truncated_milestones(self):
        lines = transcript("base").splitlines()[:5]
        r = self.cls("base", 33, "\n".join(lines) + "\n")
        self.assertEqual(r["status"], ev.FAIL)
        reached = [m["reached"] for m in r["milestones"]]
        self.assertEqual(reached[:5], [True] * 5)
        self.assertFalse(all(reached))

    def test_empty_serial(self):
        r = self.cls("base", 33, "")
        self.assertEqual(r["status"], ev.FAIL)
        self.assertFalse(any(m["reached"] for m in r["milestones"]))

    def test_fault_variant_with_pass_transcript_fails(self):
        self.assertEqual(self.cls("magic-fault", 35, transcript("base"))["status"], ev.FAIL)

    def test_info_lines_ignored(self):
        s = transcript("base").replace("OMNI8 GDT: loaded", "OMNI8 INFO: free_frames=1\nOMNI8 GDT: loaded")
        self.assertEqual(self.cls("base", 33, s)["status"], ev.PASS)

    def test_extra_trailing_line_fails(self):
        self.assertEqual(self.cls("base", 33, transcript("base") + "unexpected\n")["status"], ev.FAIL)


class BuildMetadata(unittest.TestCase):
    PIN = {"rustc": "1.91.1", "rust_target": "x86_64-unknown-none"}

    def valid_info(self, variant="base"):
        return (
            f"variant={variant} test_exit=1 profile=release "
            "target=x86_64-unknown-none\n"
            "rustc: rustc 1.91.1 (example)\n"
        )

    def test_valid_pinned_metadata(self):
        self.assertEqual(ev.validate_build_info(self.valid_info(), "base", self.PIN), [])

    def test_missing_metadata_rejected(self):
        self.assertTrue(ev.validate_build_info("", "base", self.PIN))

    def test_host_fallback_rejected(self):
        info = self.valid_info().replace("x86_64-unknown-none", "host-fallback")
        info += "NOTICE: built with the host-target fallback\n"
        self.assertTrue(ev.validate_build_info(info, "base", self.PIN))

    def test_wrong_variant_rejected(self):
        self.assertTrue(ev.validate_build_info(self.valid_info("ring3"), "base", self.PIN))

    def test_wrong_test_exit_rejected(self):
        info = self.valid_info().replace("test_exit=1", "test_exit=0")
        self.assertTrue(ev.validate_build_info(info, "base", self.PIN))

    def test_wrong_profile_rejected(self):
        info = self.valid_info().replace("profile=release", "profile=debug")
        self.assertTrue(ev.validate_build_info(info, "base", self.PIN))

    def test_wrong_target_rejected(self):
        info = self.valid_info().replace("x86_64-unknown-none", "x86_64-unknown-linux-gnu")
        self.assertTrue(ev.validate_build_info(info, "base", self.PIN))

    def test_wrong_compiler_rejected(self):
        info = self.valid_info().replace("rustc 1.91.1", "rustc 1.97.0")
        self.assertTrue(ev.validate_build_info(info, "base", self.PIN))

    def test_missing_compiler_rejected(self):
        info = self.valid_info().splitlines()[0] + "\n"
        self.assertTrue(ev.validate_build_info(info, "base", self.PIN))


class Misc(unittest.TestCase):
    def test_parse_version(self):
        self.assertEqual(ev.parse_version("QEMU emulator version 8.2.2 (Debian)"), (8, 2, 2))
        self.assertEqual(ev.parse_version("grub-mkrescue (GRUB) 2.12"), (2, 12))
        self.assertIsNone(ev.parse_version("no digits"))

    def test_stage_ids_unique_and_complete(self):
        ids = ev.stage_ids()
        self.assertEqual(len(ids), len(set(ids)))
        for v in ev.VARIANTS:
            for k in ("build", "image", "iso", "boot"):
                self.assertIn(f"{k}.{v}", ids)

    def test_pin_reader(self):
        pin = ev.read_pin(REPO)
        self.assertEqual(pin.get("rustc"), "1.91.1")

    def test_run_qemu_timeout_terminates_process(self):
        with tempfile.TemporaryDirectory() as d:
            status, timed_out, _ = ev.run_qemu([sys.executable, "-c", "import time; time.sleep(60)"], os.path.join(d, "s"), 1)
            self.assertTrue(timed_out)
            self.assertIsNone(status)

    def test_run_qemu_exit_status(self):
        with tempfile.TemporaryDirectory() as d:
            status, timed_out, _ = ev.run_qemu([sys.executable, "-c", "import sys; sys.exit(33)"], os.path.join(d, "s"), 20)
            self.assertEqual((status, timed_out), (33, False))


if __name__ == "__main__":
    unittest.main(verbosity=2)
