#!/usr/bin/env python3
"""Unit + integration tests for exile_validate.py. Standard library only.

These tests use FAKE qemu/grub tools to exercise the orchestrator's control flow. A passing test
here is NOT evidence that any kernel boots; it shows the orchestrator classifies synthetic
transcripts correctly and never reports PASS when tools are missing.
"""
import hashlib
import json
import os
import re
import stat
import struct
import shutil
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
BUILD = ev.load_module(
    os.path.join(REPO, "Meta", "Exile", "build", "exile_build.py"),
    "exile_build_test",
)
ELF_LINKER_AVAILABLE = BUILD.elf_linker_capability()[0]


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

    def test_real_artifacts_if_built(self):
        # Parser agreement with the real builds is exercised in the integration test below.
        self.assertTrue(callable(ev.validate_elf_multiboot))


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
        r = self.cls("base", None, "OMNI8 BOOT: start\n",
                     "CPU Reset (CPU 0)\nCPU Reset (CPU 0)\n",
                     timed_out=True)
        self.assertEqual(r["status"], ev.FAIL)
        self.assertTrue(r["flags"]["timeout"])
        self.assertTrue(r["flags"]["reset"])

    def test_timeout_with_triple_fault_sets_reset_flag(self):
        r = self.cls("base", None, "OMNI8 BOOT: start\n",
                     "Triple fault\n", timed_out=True)
        self.assertEqual(r["status"], ev.FAIL)
        self.assertTrue(r["flags"]["reset"])

    def test_timeout_with_single_startup_reset_is_not_reset_fault(self):
        r = self.cls("base", None, "OMNI8 BOOT: start\n",
                     "CPU Reset (CPU 0)\n", timed_out=True)
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

    def test_run_qemu_timeout_kills_group(self):
        with tempfile.TemporaryDirectory() as d:
            status, timed_out, _ = ev.run_qemu([sys.executable, "-c", "import time; time.sleep(60)"], os.path.join(d, "s"), 1)
            self.assertTrue(timed_out)
            self.assertIsNone(status)

    def test_run_qemu_exit_status(self):
        with tempfile.TemporaryDirectory() as d:
            status, timed_out, _ = ev.run_qemu([sys.executable, "-c", "import sys; sys.exit(33)"], os.path.join(d, "s"), 20)
            self.assertEqual((status, timed_out), (33, False))


def write_exe(path, body):
    if sys.platform == "win32":
        is_python = body.startswith("#!/usr/bin/env python")
        suffix = ".py" if is_python else ".sh"
        script = path + suffix
        with open(script, "w", encoding="utf-8", newline="") as f:
            f.write(body)
        launcher = path + ".cmd"
        runner = sys.executable if is_python else shutil.which("bash")
        if not runner:
            raise RuntimeError("bash is required for shell test shims")
        with open(launcher, "w", encoding="utf-8", newline="") as f:
            f.write('@echo off\\r\\n')
            f.write('"%s" "%s" %%*\\r\\n' % (runner, script) if is_python
                    else '"%s" "%s" %%*\\r\\n' % (runner, script))
            f.write('exit /b %%ERRORLEVEL%%\\r\\n')
        return
    with open(path, "w", encoding="utf-8") as f:
        f.write(body)
    os.chmod(path, os.stat(path).st_mode | stat.S_IXUSR)


class EndToEnd(unittest.TestCase):
    """Runs the real orchestrator CLI against the real repo (real base build) with fake emulator tools."""

    def run_cli(self, extra, env_extra=None, fakes=None, out=None, diagnostic=True, without_tools=()):
        d = tempfile.mkdtemp(prefix="exile-val-test-")
        self.addCleanup(lambda: shutil.rmtree(d, ignore_errors=True))
        env = dict(os.environ, PYTHONDONTWRITEBYTECODE="1")
        # Hermetic PATH: only what the stages need, never a real qemu/grub even if present on the host.
        bin_dir = os.path.join(d, "bin")
        os.makedirs(bin_dir)
        if sys.platform.startswith(("win", "cygwin")):
            # Keep native Windows/MSYS tools at their original locations.
            # Relocating MSYS executables breaks their DLL and runtime lookup.
            env["PATH"] = bin_dir + os.pathsep + os.environ.get("PATH", "")

            blocked = set(without_tools)
            blocked.update(
                name for name in ("qemu-system-x86_64", "grub-mkrescue", "xorriso")
                if name not in (fakes or {})
            )
            for name in blocked:
                write_exe(
                    os.path.join(bin_dir, name),
                    "#!/usr/bin/env python3\\nimport sys\\nsys.exit(127)\\n"
                )
        else:
            # Linux retains the original isolated tool environment.
            for tool in ("python3", "git", "bash", "gcc", "ld", "objcopy", "clang", "rustc", "cargo", "rustup", "sh", "env",
                         "cp", "mkdir", "rm", "cat", "sed", "awk", "grep", "sort", "uname", "tr", "head", "tail", "dirname",
                         "basename", "readlink", "mktemp", "date", "find", "xargs", "wc", "cut", "ls", "chmod", "ar", "as",
                         "nm", "readelf", "objdump", "strip", "cc", "make", "touch", "tee", "diff", "cmp", "sha256sum", "od",
                         "stat", "ln", "mv", "printf", "true", "false", "expr", "test", "id", "which", "realpath", "sleep"):
                if tool in without_tools:
                    continue
                tool_path = shutil.which(tool)
                if tool_path:
                    dest = os.path.join(bin_dir, tool)
                    if not os.path.exists(dest):
                        os.symlink(os.path.abspath(tool_path), dest)
        for name, body in (fakes or {}).items():
            write_exe(os.path.join(bin_dir, name), body)
        if not sys.platform.startswith(("win", "cygwin")):
            env["PATH"] = bin_dir
        for k in ("EXILE_VALIDATE_QEMU", "EXILE_VALIDATE_GRUB_MKRESCUE", "ZIG"):
            env.pop(k, None)
        env.update(env_extra or {})
        outdir = out or os.path.join(d, "out")
        r = subprocess.run([sys.executable, os.path.join(HERE, "..", "exile_validate.py"), "--out", outdir,
                            "--no-host-tests", "--kernel-syntax", "skip"] + (["--diagnostic-host-build"] if diagnostic else []) + extra,
                           capture_output=True, text=True, env=env, timeout=900)
        with open(os.path.join(outdir, "reports", "validation-report.json")) as f:
            rep = json.load(f)
        return r, rep, outdir

    def st(self, rep, sid):
        return next(s for s in rep["stages"] if s["id"] == sid)

    def test_missing_tools_never_pass(self):
        r, rep, out = self.run_cli(["--only", "env,build.base,image.base,iso.base,boot.base,integrity"])
        self.assertEqual(self.st(rep, "env.runtime-tools")["status"], ev.BLOCKED)
        self.assertEqual(self.st(rep, "iso.base")["status"], ev.BLOCKED)
        self.assertEqual(self.st(rep, "boot.base")["status"], ev.BLOCKED)
        self.assertFalse(rep["summary"]["runtime_validated"])
        self.assertNotEqual(rep["summary"]["overall"], ev.PASS)
        self.assertEqual(r.returncode, 3 if rep["summary"]["overall"] != ev.FAIL else 4)
        self.assertEqual(self.st(rep, "integrity.tracked-files")["status"], ev.PASS)

    @unittest.skipUnless(ELF_LINKER_AVAILABLE, "requires an x86-64 ELF-capable linker")
    def test_fake_boot_pass_is_not_runtime_validated_overall(self):
        tx = os.path.join(tempfile.mkdtemp(), "tx.txt")
        open(tx, "w").write(transcript("base"))
        self.addCleanup(lambda: os.remove(tx))
        fake_qemu = f"""#!/usr/bin/env python3
import sys
a = sys.argv[1:]
if '--version' in a:
    print('QEMU emulator version 8.2.0 (FAKE)'); sys.exit(0)
ser = [x for x in a if x.startswith('file:')][0][5:]
open(ser, 'w').write(open({tx!r}).read())
dbg = a[a.index('-D') + 1]
open(dbg, 'w').write('CPU Reset (CPU 0)\n')
sys.exit(33)
"""
        fake_grub = """#!/usr/bin/env python3
import sys
if '--version' in sys.argv: print('grub-mkrescue (GRUB) 2.12 FAKE'); sys.exit(0)
o = sys.argv[sys.argv.index('-o') + 1]
open(o, 'wb').write(b'FAKE-ISO')
"""
        fake_xorriso = "#!/bin/sh\necho 'xorriso 1.5.6 FAKE'\n"
        r, rep, out = self.run_cli(["--only", "env,build.base,image.base,iso.base,boot.base,integrity"],
                                   fakes={"qemu-system-x86_64": fake_qemu, "grub-mkrescue": fake_grub, "xorriso": fake_xorriso})
        self.assertEqual(self.st(rep, "image.base")["status"], ev.PASS, self.st(rep, "image.base")["detail"])
        self.assertEqual(self.st(rep, "boot.base")["status"], ev.PASS, self.st(rep, "boot.base")["detail"])
        # Only one of seven variants ran, so the report must not claim runtime validation.
        self.assertFalse(rep["summary"]["runtime_validated"])
        self.assertEqual(rep["tools"]["qemu"]["version"], "8.2.0")
        self.assertTrue(rep["tools"]["qemu"]["sha256"])

    @unittest.skipUnless(ELF_LINKER_AVAILABLE, "requires an x86-64 ELF-capable linker")
    def test_fake_boot_wrong_status_fails_and_overall_fail(self):
        fake_qemu = """#!/usr/bin/env python3
import sys
a = sys.argv[1:]
if '--version' in a:
    print('QEMU emulator version 8.2.0 (FAKE)'); sys.exit(0)
ser = [x for x in a if x.startswith('file:')][0][5:]
open(ser, 'w').write('OMNI8 BOOT: start\n')
sys.exit(1)
"""
        fake_grub = "#!/usr/bin/env python3\nimport sys\nif '--version' in sys.argv: print('grub 2.12'); sys.exit(0)\nopen(sys.argv[sys.argv.index('-o')+1],'wb').write(b'X')\n"
        r, rep, out = self.run_cli(["--only", "env,build.base,image.base,iso.base,boot.base"],
                                   fakes={"qemu-system-x86_64": fake_qemu, "grub-mkrescue": fake_grub, "xorriso": "#!/bin/sh\necho xorriso 1.5\n"})
        self.assertEqual(self.st(rep, "boot.base")["status"], ev.FAIL)
        self.assertEqual(rep["summary"]["overall"], ev.FAIL)
        self.assertEqual(r.returncode, 4 if ELF_LINKER_AVAILABLE else 3)

    def test_old_qemu_blocks(self):
        fake_qemu = "#!/bin/sh\necho 'QEMU emulator version 4.2.1'\n"
        r, rep, out = self.run_cli(["--only", "env.runtime-tools"], fakes={"qemu-system-x86_64": fake_qemu})
        s = self.st(rep, "env.runtime-tools")
        self.assertEqual(s["status"], ev.BLOCKED)
        self.assertIn("older than", s["detail"])

    def test_manifest_hashes_verify(self):
        r, rep, out = self.run_cli(["--only", "selftest,build.base,image.base"])
        for line in open(os.path.join(out, "MANIFEST.sha256")):
            digest, rel = line.rstrip("\n").split("  ", 1)
            h = hashlib.sha256(open(os.path.join(out, rel), "rb").read()).hexdigest()
            self.assertEqual(h, digest, rel)
        self.assertTrue(any(rel.endswith("validation-report.json") for rel in
                            (l.split("  ", 1)[1].strip() for l in open(os.path.join(out, "MANIFEST.sha256")))))
        # real build artifacts are validated by the independent parser
        if ELF_LINKER_AVAILABLE:
            self.assertEqual(self.st(rep, "build.base")["status"], ev.PASS)
            self.assertEqual(self.st(rep, "image.base")["status"], ev.PASS)
        else:
            self.assertEqual(self.st(rep, "build.base")["status"], ev.BLOCKED)
            self.assertNotEqual(self.st(rep, "image.base")["status"], ev.PASS)

    # ---- Work Order 1: bare-metal enforcement
    def test_default_mode_blocks_without_pinned_toolchain_and_builds_nothing(self):
        r, rep, out = self.run_cli(
            ["--only", "env,build,image,iso,boot,integrity"],
            diagnostic=False,
            without_tools=("rustc", "cargo", "rustup"),
        )
        self.assertEqual(self.st(rep, "build.bare-metal")["status"], ev.BLOCKED)
        for v in ev.VARIANTS:
            self.assertEqual(self.st(rep, f"build.{v}")["status"], ev.BLOCKED, v)
            self.assertNotIn(self.st(rep, f"image.{v}")["status"], (ev.PASS,))
            self.assertNotEqual(self.st(rep, f"boot.{v}")["status"], ev.PASS)
        self.assertFalse(os.path.exists(os.path.join(out, "bare-metal", "variants")))
        self.assertFalse(rep["summary"]["runtime_validated"])
        self.assertNotEqual(rep["summary"]["overall"], ev.PASS)
        self.assertIn("pinned bare-metal toolchain unavailable", self.st(rep, "build.bare-metal")["detail"])

    def test_diagnostic_host_build_is_labelled_and_can_never_pass(self):
        r, rep, out = self.run_cli(["--only", "build.base,integrity"])
        s = self.st(rep, "build.base")
        if not ELF_LINKER_AVAILABLE:
            self.assertEqual(s["status"], ev.BLOCKED)
            self.assertIn("ELF emulation unavailable", s["detail"])
        else:
            self.assertEqual(s["status"], ev.PASS)
            self.assertTrue(s["detail"].startswith("DIAGNOSTIC host build"), s["detail"])
            self.assertTrue(s["data"]["diagnostic_only"])
        self.assertTrue(any("--diagnostic-host-build" in l for l in rep["summary"]["limits"]))
        self.assertFalse(rep["summary"]["runtime_validated"])
        self.assertNotEqual(rep["summary"]["overall"], ev.PASS)

    def test_lying_compiler_shim_fails_the_orchestrator_stage(self):
        d = tempfile.mkdtemp(prefix="exile-shim-")
        self.addCleanup(lambda: shutil.rmtree(d, ignore_errors=True))
        lib = os.path.join(d, "lib", "rustlib", "x86_64-unknown-none", "lib")
        os.makedirs(lib)
        os.makedirs(os.path.join(d, "bin"))
        open(os.path.join(lib, "libcore-0123456789abcdef.rlib"), "wb").close()
        real = shutil.which("rustc")
        self.assertIsNotNone(real, "rustc must be available for the compiler-shim test")
        real = os.path.abspath(real)
        write_exe(os.path.join(d, "bin", "rustc"), f"""#!/usr/bin/env python3
import subprocess, sys
a = sys.argv[1:]
if a == ['-vV']:
    print('rustc 1.91.1 (shim)\\nhost: x86_64-unknown-linux-gnu\\nrelease: 1.91.1'); sys.exit(0)
if a == ['-V']:
    print('rustc 1.91.1 (shim)'); sys.exit(0)
if a[:2] == ['--print', 'sysroot']:
    print({d!r}); sys.exit(0)
o = []; i = 0
while i < len(a):
    if a[i] == '--target': i += 2; continue
    o.append(a[i]); i += 1
sys.exit(subprocess.call([{real!r}] + o))
""")
        r, rep, out = self.run_cli(["--only", "build.base,image.base,iso.base,boot.base"], env_extra={"EXILE_RUST_SYSROOT": d}, diagnostic=False)
        expected_status = ev.FAIL if ELF_LINKER_AVAILABLE else ev.BLOCKED
        self.assertEqual(
            self.st(rep, "build.bare-metal")["status"],
            expected_status,
            self.st(rep, "build.bare-metal")["detail"],
        )
        self.assertEqual(self.st(rep, "build.base")["status"], expected_status)
        detail = self.st(rep, "build.base")["detail"]
        if ELF_LINKER_AVAILABLE:
            self.assertIn("host target triple", detail)
        else:
            self.assertIn("ELF emulation unavailable", detail)
        self.assertNotEqual(self.st(rep, "image.base")["status"], ev.PASS)
        self.assertNotEqual(self.st(rep, "boot.base")["status"], ev.PASS)
        self.assertFalse(rep["summary"]["runtime_validated"])
        self.assertEqual(r.returncode, 4 if ELF_LINKER_AVAILABLE else 3)

    def test_stage_selection_includes_bare_metal(self):
        self.assertIn("build.bare-metal", ev.stage_ids())

    def test_unknown_selector_is_usage_error(self):
        r = subprocess.run([sys.executable, os.path.join(HERE, "..", "exile_validate.py"), "--only", "nonsense"],
                           capture_output=True, text=True)
        self.assertEqual(r.returncode, 2)

    def test_repo_state_untouched(self):
        before = subprocess.run(["git", "-C", REPO, "status", "--porcelain", "--untracked-files=no"], capture_output=True, text=True).stdout
        self.run_cli(["--only", "env,selftest,build.base,image.base"])
        after = subprocess.run(["git", "-C", REPO, "status", "--porcelain", "--untracked-files=no"], capture_output=True, text=True).stdout
        self.assertEqual(before, after)


if __name__ == "__main__":
    unittest.main(verbosity=2)
