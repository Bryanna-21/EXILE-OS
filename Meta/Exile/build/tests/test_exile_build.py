#!/usr/bin/env python3
"""Tests for exile_build.py. Standard library only.

What these tests can and cannot show: the pinned rustc 1.91.1 / x86_64-unknown-none toolchain is NOT available in the
sandbox that wrote them, so no ACCEPTED bare-metal build is produced here. They prove (a) the pipeline refuses to build
without the pinned toolchain, (b) it rejects host-target builds even when a shim lies about the compiler version, and
(c) its ELF/ISO/manifest checks work. The accepted path is covered only at function level until the pinned toolchain exists.
"""
import hashlib
import json
import os
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
import exile_build as eb  # noqa: E402

REPO = eb.REPO
PIN = eb.read_pin(REPO)
TOOL = os.path.join(HERE, "..", "exile_build.py")


def jl(path):
    with open(path) as f:
        return json.load(f)


def jd(obj, path):
    with open(path, "w") as f:
        json.dump(obj, f)


def wb(path, data):
    with open(path, "wb") as f:
        f.write(data)


def rb(path):
    with open(path, "rb") as f:
        return f.read()


def rt(path):
    with open(path) as f:
        return f.read()


def exe(path, body):
    with open(path, "w") as f:
        f.write(body)
    os.chmod(path, os.stat(path).st_mode | stat.S_IXUSR)


def real_rustc():
    path = shutil.which("rustc")
    if not path:
        raise RuntimeError("rustc is required for compiler-shim tests")
    return os.path.abspath(path)


def tmpdir(tc):
    d = tempfile.mkdtemp(prefix="exile-build-test-")
    tc.addCleanup(lambda: subprocess.run(["rm", "-rf", d]))
    return d


def fake_sysroot(root, with_target=True):
    os.makedirs(os.path.join(root, "bin"), exist_ok=True)
    if with_target:
        lib = os.path.join(root, "lib", "rustlib", PIN["rust_target"], "lib")
        os.makedirs(lib, exist_ok=True)
        open(os.path.join(lib, "libcore-0123456789abcdef.rlib"), "wb").close()


def shim_rustc(root, release, forward=True):
    """A rustc that CLAIMS `release` and, if forward, compiles with the real rustc (dropping --target)."""
    real = real_rustc()
    exe(os.path.join(root, "bin", "rustc"), f"""#!/usr/bin/env python3
import os, subprocess, sys
a = sys.argv[1:]
if a == ['-vV']:
    print('rustc {release} (shim)\\nbinary: rustc\\ncommit-hash: 0\\nhost: x86_64-unknown-linux-gnu\\nrelease: {release}\\nLLVM version: 0'); sys.exit(0)
if a == ['-V']:
    print('rustc {release} (shim)'); sys.exit(0)
if a[:2] == ['--print', 'sysroot']:
    print({root!r}); sys.exit(0)
out = []
i = 0
while i < len(a):
    if a[i] == '--target':
        i += 2
        continue
    out.append(a[i]); i += 1
{'sys.exit(subprocess.call([' + repr(real) + '] + out))' if forward else 'sys.exit(1)'}
""")


def env_with(sysroot):
    e = dict(os.environ)
    e["EXILE_RUST_SYSROOT"] = sysroot
    return e


def cli(args, env=None):
    return subprocess.run([sys.executable, "-B", TOOL, "--repo", REPO] + args, capture_output=True, text=True, env=env or dict(os.environ))


class Preflight(unittest.TestCase):
    def test_pin_is_read(self):
        self.assertEqual(PIN["rustc"], "1.91.1")
        self.assertEqual(PIN["rust_target"], "x86_64-unknown-none")
        self.assertEqual(PIN["zig"], "0.15.2")

    def test_wrong_rustc_blocks(self):
        d = tmpdir(self)
        fake_sysroot(d)
        shim_rustc(d, "1.97.0")
        r = eb.preflight(REPO, env=env_with(d) | {"PATH": "/usr/bin:/bin"})
        self.assertEqual(r["status"], eb.BLOCKED)
        self.assertIn("rustc.pinned", [b["check"] for b in r["blockers"]])

    def test_pinned_rustc_without_target_blocks(self):
        d = tmpdir(self)
        fake_sysroot(d, with_target=False)
        shim_rustc(d, PIN["rustc"])
        r = eb.preflight(REPO, env=env_with(d) | {"PATH": "/usr/bin:/bin"})
        self.assertEqual(r["status"], eb.BLOCKED)
        self.assertIn("rust.target-installed", [b["check"] for b in r["blockers"]])
        self.assertFalse(r["toolchain"]["target_installed"])

    def test_pinned_rustc_with_target_passes_preflight(self):
        d = tmpdir(self)
        fake_sysroot(d)
        shim_rustc(d, PIN["rustc"])
        r = eb.preflight(REPO, env=env_with(d))
        self.assertEqual(r["status"], eb.PASS, r["blockers"])
        self.assertEqual(r["toolchain"]["rustc"]["origin"], "EXILE_RUST_SYSROOT")

    def test_zig_only_blocks_when_asked(self):
        d = tmpdir(self)
        fake_sysroot(d)
        shim_rustc(d, PIN["rustc"])
        e = env_with(d) | {"PATH": os.environ["PATH"]}
        base = eb.preflight(REPO, env=e)
        strict = eb.preflight(REPO, env=e, with_zig=True)
        zig_ok = base["toolchain"]["zig"]["version"] == PIN["zig"]
        self.assertEqual(base["status"], eb.PASS)
        self.assertEqual(strict["status"], eb.PASS if zig_ok else eb.BLOCKED)

    def test_cli_preflight_exit_code_is_3_when_blocked(self):
        d = tmpdir(self)
        fake_sysroot(d, with_target=False)
        shim_rustc(d, PIN["rustc"])
        r = cli(["preflight"], env=env_with(d))
        self.assertEqual(r.returncode, 3)
        self.assertIn("PREFLIGHT BLOCKED", r.stdout)


class BuildRefusesWithoutPin(unittest.TestCase):
    def test_blocked_build_makes_no_artifacts_and_unverifiable_manifest(self):
        d = tmpdir(self)
        sysroot = os.path.join(d, "sr")
        fake_sysroot(sysroot, with_target=False)
        shim_rustc(sysroot, PIN["rustc"])
        out = os.path.join(d, "out")
        r = cli(["build", "--out", out], env=env_with(sysroot))
        self.assertEqual(r.returncode, 3, r.stdout + r.stderr)
        m = jl(os.path.join(out, "build-manifest.json"))
        self.assertEqual((m["status"], m["accepted"], m["variants"]), (eb.BLOCKED, False, []))
        self.assertFalse(os.path.exists(os.path.join(out, "variants")))
        self.assertFalse(m["config"]["host_fallback_allowed"])
        ok, problems = eb.verify_manifest(os.path.join(out, "build-manifest.json"), REPO)
        self.assertFalse(ok)

    def test_unknown_variant_is_usage_error(self):
        self.assertEqual(cli(["build", "--out", "/tmp/x-unused", "--variants", "nope"]).returncode, 2)

    def test_no_host_fallback_switch_exists(self):
        h = cli(["build", "--help"]).stdout.lower()
        for word in ("fallback", "allow-host", "host-target"):
            self.assertNotIn(word, h)


class HostBuildsAreRejected(unittest.TestCase):
    """Uses the EXISTING build.sh without --target to make a real host-target ELF, then proves it is rejected."""

    @classmethod
    def setUpClass(cls):
        cls.d = tempfile.mkdtemp(prefix="exile-build-fb-")
        cls.out = os.path.join(cls.d, "fb")
        r = subprocess.run(["bash", os.path.join(REPO, eb.B8, "scripts", "build.sh"), "--variant", "base", "--test-exit", "--out", cls.out],
                           capture_output=True, text=True)
        cls.built = r.returncode == 0

    @classmethod
    def tearDownClass(cls):
        subprocess.run(["rm", "-rf", cls.d])

    def test_valid_elf_but_rejected(self):
        self.assertTrue(self.built)
        ok, checks, _ = eb.inspect_elf(os.path.join(self.out, "kernel.elf"), REPO)
        self.assertTrue(ok, [c for c in checks if not c[1]])
        info = rt(os.path.join(self.out, "build-info.txt"))
        why = eb.judge_provenance(info, eb.provenance(self.out, PIN), PIN)
        self.assertTrue(any("host target triple" in x for x in why), why)
        self.assertTrue(any("pinned target triple" in x for x in why), why)
        r = cli(["inspect", self.out])
        self.assertEqual(r.returncode, 4)
        self.assertIn("acceptance: REJECTED", r.stdout)

    def test_elf_without_build_info_is_unattributed_and_rejected(self):
        d = tmpdir(self)
        subprocess.run(["cp", os.path.join(self.out, "kernel.elf"), d])
        r = cli(["inspect", os.path.join(d, "kernel.elf")])
        self.assertEqual(r.returncode, 4)
        self.assertIn("provenance is unknown", r.stdout)

    def test_a_shim_that_lies_about_its_version_is_still_rejected(self):
        # The shim reports the pinned version and build.sh records the pinned target.
        # The real compiler drops --target, so the resulting rlibs expose the host build.
        d = tmpdir(self)
        sr = os.path.join(d, "sr")
        fake_sysroot(sr)
        shim_rustc(sr, PIN["rustc"])
        out = os.path.join(d, "out")
        r = cli(["build", "--out", out, "--variants", "base"], env=env_with(sr))
        self.assertEqual(r.returncode, 4, r.stdout + r.stderr)
        m = jl(os.path.join(out, "build-manifest.json"))
        v = m["variants"][0]
        self.assertIn("x86_64-unknown-none", v["build_info"])  # the lie
        self.assertIn("rustc 1.91.1", v["build_info"])          # the lie
        self.assertEqual((m["status"], m["accepted"], v["status"]), (eb.REJECTED, False, eb.REJECTED))
        self.assertTrue(any("pinned target triple" in x for x in v["reasons"]), v["reasons"])
        self.assertTrue(any("host target triple" in x for x in v["reasons"]), v["reasons"])
        # an attacker who edits the manifest to say PASS is still caught on verification
        m["status"], m["accepted"] = eb.PASS, True
        m["variants"][0]["status"] = eb.PASS
        mp = os.path.join(out, "build-manifest.json")
        jd(m, mp)
        ok, problems = eb.verify_manifest(mp, REPO)
        self.assertFalse(ok)
        self.assertTrue(any("pinned target triple" in p for p in problems), problems)


class Judge(unittest.TestCase):
    GOOD_INFO = "variant=base test_exit=1 profile=release target=x86_64-unknown-none\nrustc: rustc 1.91.1 (abc 2025-11-10)\n"
    GOOD_PROV = {"rlib_rustc_versions": ["1.91.1"], "rlib_has_pinned_target": True, "host_triples_found": []}

    def test_consistent_pinned_evidence_is_acceptable(self):
        self.assertEqual(eb.judge_provenance(self.GOOD_INFO, self.GOOD_PROV, PIN), [])

    def test_each_defect_is_caught_independently(self):
        cases = {
            "wrong target": (self.GOOD_INFO.replace("x86_64-unknown-none", "host-fallback"), self.GOOD_PROV),
            "wrong rustc in info": (self.GOOD_INFO.replace("1.91.1", "1.97.0"), self.GOOD_PROV),
            "wrong rustc in rlib": (self.GOOD_INFO, dict(self.GOOD_PROV, rlib_rustc_versions=["1.97.0"])),
            "mixed rustc in rlibs": (self.GOOD_INFO, dict(self.GOOD_PROV, rlib_rustc_versions=["1.91.1", "1.97.0"])),
            "no rlib version": (self.GOOD_INFO, dict(self.GOOD_PROV, rlib_rustc_versions=[])),
            "rlib target missing": (self.GOOD_INFO, dict(self.GOOD_PROV, rlib_has_pinned_target=False)),
            "rlib target unknown": (self.GOOD_INFO, dict(self.GOOD_PROV, rlib_has_pinned_target=None)),
            "host triple in staticlib": (self.GOOD_INFO, dict(self.GOOD_PROV, host_triples_found=["x86_64-unknown-linux-gnu"])),
            "fallback notice": (self.GOOD_INFO + "NOTICE: built with the host-target fallback; the pinned bare-metal toolchain was not used.\n", self.GOOD_PROV),
            "empty info": ("", self.GOOD_PROV),
        }
        for name, (info, prov) in cases.items():
            self.assertTrue(eb.judge_provenance(info, prov, PIN), name)


class ElfRules(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.d = tempfile.mkdtemp(prefix="exile-build-elf-")
        subprocess.run(["bash", os.path.join(REPO, eb.B8, "scripts", "build.sh"), "--variant", "base", "--test-exit", "--out", os.path.join(cls.d, "b")],
                       capture_output=True)
        cls.elf = os.path.join(cls.d, "b", "kernel.elf")
        cls.data = rb(cls.elf)

    @classmethod
    def tearDownClass(cls):
        subprocess.run(["rm", "-rf", cls.d])

    def run_mut(self, mutate):
        b = bytearray(self.data)
        mutate(b, eb.parse_elf(self.data))
        p = os.path.join(self.d, "mut.elf")
        wb(p, b)
        ok, checks, _ = eb.inspect_elf(p, REPO)
        return ok, {c[0] for c in checks if not c[1]}

    def sec_off(self, e, name):
        shoff = struct.unpack_from("<Q", self.data, 40)[0]
        idx = [s["name"] for s in e["sections"]].index(name)
        return shoff + idx * 64

    def test_unmodified_is_valid(self):
        ok, bad = self.run_mut(lambda b, e: None)
        self.assertTrue(ok, bad)

    def test_wrong_machine(self):
        ok, bad = self.run_mut(lambda b, e: struct.pack_into("<H", b, 18, 3))
        self.assertFalse(ok)
        self.assertIn("architecture is x86-64 (EM_X86_64)", bad)

    def test_writable_text_section(self):
        def m(b, e):
            o = self.sec_off(e, ".text") + 8
            struct.pack_into("<Q", b, o, struct.unpack_from("<Q", b, o)[0] | 1)
        ok, bad = self.run_mut(m)
        self.assertFalse(ok)
        self.assertIn("section .text present with correct permissions", bad)

    def test_executable_data_section(self):
        def m(b, e):
            o = self.sec_off(e, ".data") + 8
            struct.pack_into("<Q", b, o, struct.unpack_from("<Q", b, o)[0] | 4)
        self.assertIn("section .data present with correct permissions", self.run_mut(m)[1])

    def test_entry_outside_text(self):
        ok, bad = self.run_mut(lambda b, e: struct.pack_into("<Q", b, 24, 0x200000))
        self.assertFalse(ok)
        self.assertIn("entry point lies inside .text", bad)

    def test_missing_section_is_detected(self):
        def m(b, e):  # rename .rodata so it is "missing"
            shoff = struct.unpack_from("<Q", b, 40)[0]
            shstr = struct.unpack_from("<Q", b, shoff + struct.unpack_from("<H", b, 62)[0] * 64 + 24)[0]
            i = b.find(b".rodata\0", shstr)
            b[i:i + 7] = b".xodata"
        self.assertIn("section .rodata present with correct permissions", self.run_mut(m)[1])

    def test_not_an_elf(self):
        p = os.path.join(self.d, "junk")
        wb(p, b"MZ" + b"\0" * 200)
        ok, checks, _ = eb.inspect_elf(p, REPO)
        self.assertFalse(ok)


class IsoStage(unittest.TestCase):
    def fake_xorriso(self, d, listing, extract_from):
        exe(os.path.join(d, "xorriso"), f"""#!/usr/bin/env python3
import shutil, sys
a = sys.argv[1:]
if '-find' in a:
    print({listing!r}); sys.exit(0)
if '-report_el_torito' in a:
    print('El Torito boot img : 1  BIOS  y   none   0x0000  0x00  2048  1  0'); sys.exit(0)
if '-extract' in a:
    shutil.copy({extract_from!r}, a[a.index('-extract') + 2]); sys.exit(0)
sys.exit(1)
""")
        return os.path.join(d, "xorriso")

    LISTING = "/\n/boot\n/boot/omni8.elf\n/boot/grub\n/boot/grub/grub.cfg\n/boot/grub/i386-pc\n/boot/grub/i386-pc/core.img\n"

    def test_parse_find(self):
        self.assertEqual(eb.parse_xorriso_find("junk\n/a\n'/b c'\n"), ["/a", "/b c"])

    def test_inspect_iso_accepts_matching_kernel(self):
        d = tmpdir(self)
        k = os.path.join(d, "k.elf")
        wb(k, b"KERNEL-BYTES")
        x = self.fake_xorriso(d, self.LISTING, k)
        ok, checks = eb.inspect_iso("/dev/null", hashlib.sha256(b"KERNEL-BYTES").hexdigest(), {"xorriso": x}, d)
        self.assertTrue(ok, [c for c in checks if not c["ok"]])

    def test_inspect_iso_rejects_a_different_kernel_inside(self):
        d = tmpdir(self)
        k = os.path.join(d, "k.elf")
        wb(k, b"OTHER")
        x = self.fake_xorriso(d, self.LISTING, k)
        ok, checks = eb.inspect_iso("/dev/null", hashlib.sha256(b"KERNEL-BYTES").hexdigest(), {"xorriso": x}, d)
        self.assertFalse(ok)
        self.assertTrue(any("byte-identical" in c["name"] and not c["ok"] for c in checks))

    def test_inspect_iso_rejects_missing_grub_cfg(self):
        d = tmpdir(self)
        k = os.path.join(d, "k.elf")
        wb(k, b"K")
        x = self.fake_xorriso(d, "/boot/omni8.elf\n", k)
        ok, _ = eb.inspect_iso("/dev/null", hashlib.sha256(b"K").hexdigest(), {"xorriso": x}, d)
        self.assertFalse(ok)

    def test_iso_stage_blocked_when_build_not_accepted(self):
        d = tmpdir(self)
        os.makedirs(os.path.join(d, "out"))
        m = {"status": eb.BLOCKED, "notes": [], "variants": [], "iso": []}
        jd(m, os.path.join(d, "out", "build-manifest.json"))
        r = cli(["iso", "--out", os.path.join(d, "out")])
        self.assertEqual(r.returncode, 3)
        self.assertFalse(os.path.exists(os.path.join(d, "out", "iso")))

    def test_iso_stage_blocked_without_grub_tools(self):
        d = tmpdir(self)
        out = os.path.join(d, "out")
        os.makedirs(out)
        m = {"status": eb.PASS, "notes": [], "variants": [{"variant": "base", "elf": {"sha256": "0" * 64}}], "iso": []}
        jd(m, os.path.join(out, "build-manifest.json"))
        e = dict(os.environ, PATH="/nonexistent")
        r = subprocess.run([sys.executable, "-B", TOOL, "--repo", REPO, "iso", "--out", out], capture_output=True, text=True, env=e)
        self.assertEqual(r.returncode, 3, r.stdout + r.stderr)
        m2 = jl(os.path.join(out, "build-manifest.json"))
        self.assertEqual(m2["iso"][0]["status"], eb.BLOCKED)


class ManifestVerification(unittest.TestCase):
    def synthetic(self, d):
        vd = os.path.join(d, "variants", "base")
        os.makedirs(vd)
        p = os.path.join(vd, "kernel.elf")
        wb(p, b"ELF")
        m = {"schema_version": 1, "status": eb.PASS, "accepted": True, "config": {"host_fallback_allowed": False},
             "requested_variants": ["base"], "toolchain": {"pin": PIN},
             "variants": [{"variant": "base", "status": eb.PASS, "build_info": "", "artifacts": [
                 {"path": "variants/base/kernel.elf", "sha256": hashlib.sha256(b"ELF").hexdigest(), "bytes": 3}]}]}
        mp = os.path.join(d, "build-manifest.json")
        jd(m, mp)
        return mp, p

    def test_tampered_artifact_is_detected(self):
        d = tmpdir(self)
        mp, p = self.synthetic(d)
        wb(p, b"EVIL")
        ok, problems = eb.verify_manifest(mp, REPO)
        self.assertFalse(ok)
        self.assertTrue(any("hash mismatch" in x for x in problems), problems)

    def test_missing_variant_is_detected(self):
        d = tmpdir(self)
        mp, _ = self.synthetic(d)
        m = jl(mp)
        m["requested_variants"] = ["base", "ring3"]
        jd(m, mp)
        self.assertTrue(any("not every requested variant" in x for x in eb.verify_manifest(mp, REPO)[1]))

    def test_host_fallback_flag_is_detected(self):
        d = tmpdir(self)
        mp, _ = self.synthetic(d)
        m = jl(mp)
        m["config"]["host_fallback_allowed"] = True
        jd(m, mp)
        self.assertTrue(any("host fallback" in x for x in eb.verify_manifest(mp, REPO)[1]))


class Hygiene(unittest.TestCase):
    def test_existing_scripts_are_untouched(self):
        r = subprocess.run(
            ["git", "-C", REPO, "diff", "--name-only", "HEAD", "--", "*.sh", "*.bash"],
            capture_output=True,
            text=True,
            check=True,
        ).stdout
        self.assertEqual(r.strip(), "")

    def test_source_identity_is_stable(self):
        a, b = eb.source_identity(REPO), eb.source_identity(REPO)
        self.assertEqual(a["build_input_digest"], b["build_input_digest"])
        self.assertEqual(len(a["git_head"]), 40)
        self.assertGreater(a["build_input_files"], 20)


if __name__ == "__main__":
    unittest.main(verbosity=2)
