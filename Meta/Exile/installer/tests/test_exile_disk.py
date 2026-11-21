import os, struct, subprocess, sys, tempfile, unittest, shutil, hashlib
HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, os.path.dirname(HERE))
import exile_disk as D


class T(unittest.TestCase):
    def setUp(self):
        self.d = tempfile.mkdtemp()
        self.img = os.path.join(self.d, "d.img")

    def tearDown(self):
        shutil.rmtree(self.d)

    def test_build_validates_and_layout(self):
        parts = D.build(self.img, 700, seed="t")
        self.assertEqual(D.validate(self.img), [])
        self.assertEqual([p["name"] for p in parts], ["EXILE-ESP", "EXILE-ROOT-A", "EXILE-ROOT-B", "EXILE-STATE"])
        self.assertEqual(parts[0]["sectors"], 64 * 2048)
        self.assertTrue(all(p["first"] % 2048 == 0 for p in parts))
        self.assertEqual(os.path.getsize(self.img), 700 * 1024 * 1024)

    def test_deterministic_with_seed(self):
        a = os.path.join(self.d, "a"); b = os.path.join(self.d, "b")
        D.build(a, 700, seed="x"); D.build(b, 700, seed="x")
        def h(p):
            with open(p, "rb") as fh:
                return hashlib.sha256(fh.read()).hexdigest()
        self.assertEqual(h(a), h(b))
        D.build(b, 700, seed="y")
        self.assertNotEqual(h(a), h(b))

    def test_every_header_and_array_corruption_is_detected(self):
        D.build(self.img, 700, seed="t")
        total = os.path.getsize(self.img) // 512
        for off, why in [(512 + 20, "primary hdr"), (1024 + 5, "primary entries"), ((total - 1) * 512 + 24, "backup hdr"), ((total - 33) * 512 + 5, "backup entries"), (510, "mbr sig")]:
            with open(self.img, "r+b") as f:
                f.seek(off); orig = f.read(1); f.seek(off); f.write(bytes([orig[0] ^ 0xFF]))
            self.assertTrue(D.validate(self.img), f"undetected: {why}")
            with open(self.img, "r+b") as f:
                f.seek(off); f.write(orig)
            self.assertEqual(D.validate(self.img), [], f"restore failed: {why}")

    def test_overlap_detected_even_with_valid_crcs(self):
        D.build(self.img, 700, seed="t")
        with open(self.img, "r+b") as f:
            f.seek(1024 + 128 + 32)  # entry1 first LBA := entry0 first
            f.write(struct.pack("<Q", 2048))
            f.seek(1024); ents = f.read(128 * 128)
            f.seek(512); h = bytearray(f.read(92))
            struct.pack_into("<I", h, 88, D.crc(ents)); struct.pack_into("<I", h, 16, 0)
            struct.pack_into("<I", h, 16, D.crc(bytes(h)))
            f.seek(512); f.write(bytes(h))
        self.assertTrue(any("overlap" in p or "differ" in p for p in D.validate(self.img)))

    def test_refuses_devices_and_non_regular(self):
        for bad in ("/dev/null", "/dev/sda", "/dev/loop0"):
            with self.assertRaises(D.DiskError):
                D.check_target(bad)
        with self.assertRaises(D.DiskError):
            D.check_target(self.d)  # directory
        link = os.path.join(self.d, "lnk"); os.symlink("/dev/null", link)
        with self.assertRaises(D.DiskError):
            D.build(link, 700)
        self.assertEqual(D.main(["validate", "/dev/null"]), 2)

    def test_too_small_refused_and_file_not_created(self):
        with self.assertRaises(D.DiskError):
            D.build(self.img, 100)
        self.assertFalse(os.path.exists(self.img))

    def test_cli_exit_codes(self):
        r = subprocess.run([sys.executable, "-I", "-B", os.path.join(os.path.dirname(HERE), "exile_disk.py"), "build", self.img, "--size-mib", "700"], capture_output=True)
        self.assertEqual(r.returncode, 0)
        self.assertEqual(D.main(["validate", self.img]), 0)

    def test_external_tool_cross_check_if_available(self):
        tool = shutil.which("sfdisk")
        if not tool:
            self.skipTest("sfdisk not installed: independent cross-check NOT RUN")
        D.build(self.img, 700, seed="t")
        r = subprocess.run([tool, "--verify", self.img], capture_output=True, text=True)
        self.assertEqual(r.returncode, 0, r.stdout + r.stderr)


if __name__ == "__main__":
    unittest.main(verbosity=2)
