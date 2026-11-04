#!/usr/bin/env python3
"""QEMU boot tests for the Batch 8 kernel.

For each case: build the kernel variant (scripts/build.sh --test-exit), wrap it in a GRUB ISO,
boot it under qemu-system-x86_64 with the serial port on stdio and isa-debug-exit, then check
the exit status and compare the serial transcript with tests/expected/<case>.txt.

NOTHING IS REPORTED AS PASSED UNLESS QEMU ACTUALLY BOOTED THE IMAGE. If qemu-system-x86_64 or
grub-mkrescue (with xorriso/mtools) is missing, every case is reported "NOT RUN" and the exit
status is 3. A build failure is a failure, never a skip.

usage: qemu_test.py [--cases base,ring3,...] [--out DIR] [--timeout SECONDS]
       qemu_test.py --selftest      # checks the transcript matcher only; boots nothing
Environment: OMNI8_QEMU (default qemu-system-x86_64), OMNI8_GRUB_MKRESCUE (default
grub-mkrescue, then grub2-mkrescue).
Exit status: 0 every selected case passed, 1 a case failed, 2 usage error, 3 NOT RUN.
"""
import os
import re
import shutil
import subprocess
import sys
import tempfile

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(HERE)
EXPECT_DIR = os.path.join(ROOT, "tests", "expected")
PASS_STATUS, FAIL_STATUS = 33, 35  # isa-debug-exit: (value << 1) | 1 for 0x10 and 0x11
CASES = {
    "base": ("base", PASS_STATUS),
    "ring3": ("ring3", PASS_STATUS),
    "ring3-fault": ("ring3-fault", FAIL_STATUS),
    "trap-fault": ("trap-fault", FAIL_STATUS),
    "magic-fault": ("magic-fault", FAIL_STATUS),
    "cpu-fault": ("cpu-fault", FAIL_STATUS),
}
QEMU_ARGS = ["-machine", "pc", "-accel", "tcg", "-cpu", "qemu64", "-m", "128M", "-boot", "d",
             "-display", "none", "-serial", "stdio", "-monitor", "none", "-no-reboot",
             "-device", "isa-debug-exit,iobase=0xf4,iosize=0x04"]


def parse_expected(text):
    """Returns (mode, lines, forbidden). mode is 'exact' or 'contains'."""
    mode, lines, forbid = "exact", [], []
    for raw in text.splitlines():
        if not raw.strip() or raw.startswith("#") and raw.strip() != "# CONTAINS":
            continue
        if raw.strip() == "# CONTAINS":
            mode = "contains"
        elif raw.startswith("FORBID "):
            forbid.append(raw[len("FORBID "):])
        else:
            lines.append(raw)
    return mode, lines, forbid


def line_ok(expected, actual):
    if expected.startswith("~"):
        return re.fullmatch(expected[1:], actual) is not None
    return expected == actual


def compare(transcript, expected_text):
    """Returns a list of problems (empty means the transcript matches)."""
    mode, lines, forbid = parse_expected(expected_text)
    got = [l.rstrip("\r") for l in transcript.splitlines()]
    problems = []
    for f in forbid:
        if any(f == g or g.startswith(f) for g in got):
            problems.append(f"forbidden line present: {f!r}")
    if mode == "exact":
        seq = [g for g in got if not g.startswith("OMNI8 INFO:")]
        for i, e in enumerate(lines):
            if i >= len(seq):
                problems.append(f"transcript ended early: expected line {i + 1} {e!r}")
                break
            if not line_ok(e, seq[i]):
                problems.append(f"line {i + 1}: expected {e!r}, got {seq[i]!r}")
                break
        else:
            if len(seq) > len(lines):
                problems.append(f"unexpected extra line after the expected transcript: {seq[len(lines)]!r}")
    else:
        pos = 0
        for e in lines:
            while pos < len(got) and not line_ok(e, got[pos]):
                pos += 1
            if pos == len(got):
                problems.append(f"expected line not found in order: {e!r}")
                break
            pos += 1
    return problems


def selftest():
    base = open(os.path.join(EXPECT_DIR, "base.txt")).read()
    mode, lines, _ = parse_expected(base)
    good = "\n".join(l.replace("~", "") if not l.startswith("~") else "OMNI8 MEM: map parsed regions=5 usable_ranges=2" for l in lines)
    good = good.replace("OMNI8 GDT: loaded", "OMNI8 INFO: free_frames=1 ram_top=0x8000000\nOMNI8 GDT: loaded")
    checks = []

    def case(name, ok):
        checks.append(ok)
        print(("PASS " if ok else "FAIL ") + name)

    case("matcher accepts a correct transcript (INFO lines ignored)", compare(good, base) == [])
    case("matcher rejects a missing line", compare(good.replace("OMNI8 TIMER: ticks ok\n", ""), base) != [])
    case("matcher rejects a reordered transcript",
         compare(good.replace("OMNI8 GDT: loaded\nOMNI8 TSS: loaded", "OMNI8 TSS: loaded\nOMNI8 GDT: loaded"), base) != [])
    case("matcher rejects extra output after PASS", compare(good + "\nOMNI8 surprise", base) != [])
    case("matcher rejects a wrong regex field", compare(good.replace("regions=5", "regions=x"), base) != [])
    case("matcher rejects an empty transcript", compare("", base) != [])
    fatal = open(os.path.join(EXPECT_DIR, "trap-fault.txt")).read()
    ok = ("OMNI8 IDT: loaded\nOMNI8 FATAL EXCEPTION vector=6 name=#UD invalid opcode err=0x0000000000000000\n"
          "OMNI8   rip=0x0\nOMNI8 FAIL: unexpected exception\n")
    case("fault matcher accepts a correct fatal report", compare(ok, fatal) == [])
    case("fault matcher rejects a transcript that reports PASS", compare(ok + "OMNI8 TEST PASS\n", fatal) != [])
    case("fault matcher rejects the wrong vector", compare(ok.replace("vector=6", "vector=13"), fatal) != [])
    bad = checks.count(False)
    print(f"qemu_test selftest: {len(checks) - bad} passed, {bad} failed (matcher only; no kernel was booted)")
    return 1 if bad else 0


def find_tools():
    qemu = os.environ.get("OMNI8_QEMU") or shutil.which("qemu-system-x86_64")
    grub = os.environ.get("OMNI8_GRUB_MKRESCUE") or shutil.which("grub-mkrescue") or shutil.which("grub2-mkrescue")
    missing = []
    if not qemu:
        missing.append("qemu-system-x86_64")
    if not grub:
        missing.append("grub-mkrescue")
    elif not shutil.which("xorriso"):
        missing.append("xorriso")
    return qemu, grub, missing


def run_case(name, qemu, grub, out, timeout):
    variant, want_status = CASES[name]
    work = os.path.join(out, name)
    os.makedirs(work, exist_ok=True)
    build = subprocess.run([os.path.join(HERE, "build.sh"), "--variant", variant, "--test-exit", "--out", os.path.join(work, "build")],
                           capture_output=True, text=True)
    if build.returncode != 0:
        return "FAIL", f"build failed:\n{build.stderr[-2000:]}"
    elf = os.path.join(work, "build", "kernel.elf")
    stage = os.path.join(work, "iso-root")
    os.makedirs(os.path.join(stage, "boot", "grub"), exist_ok=True)
    shutil.copy(elf, os.path.join(stage, "boot", "omni8.elf"))
    shutil.copy(os.path.join(ROOT, "boot", "grub.cfg"), os.path.join(stage, "boot", "grub", "grub.cfg"))
    iso = os.path.join(work, "omni8.iso")
    mk = subprocess.run([grub, "-o", iso, stage], capture_output=True, text=True)
    if mk.returncode != 0:
        return "FAIL", f"grub-mkrescue failed:\n{mk.stderr[-2000:]}"
    cmd = [qemu, *QEMU_ARGS, "-cdrom", iso]
    try:
        r = subprocess.run(cmd, capture_output=True, text=True, timeout=timeout)
        status, serial, timed_out = r.returncode, r.stdout, False
    except subprocess.TimeoutExpired as ex:
        status, serial, timed_out = None, (ex.stdout or b"").decode("utf-8", "replace") if isinstance(ex.stdout, bytes) else (ex.stdout or ""), True
    with open(os.path.join(work, "serial.txt"), "w") as f:
        f.write(serial)
    with open(os.path.join(work, "command.txt"), "w") as f:
        f.write(" ".join(cmd) + f"\nexit status: {status}\n")
    problems = []
    if timed_out:
        problems.append(f"timed out after {timeout}s")
    elif status != want_status:
        problems.append(f"exit status {status}, expected {want_status}")
    problems += compare(serial, open(os.path.join(EXPECT_DIR, name + ".txt")).read())
    return ("PASS", f"exit status {status}") if not problems else ("FAIL", "; ".join(problems))


def main():
    args = sys.argv[1:]
    if "--selftest" in args:
        return selftest()
    names, out, timeout = list(CASES), None, 60
    i = 0
    while i < len(args):
        if args[i] == "--cases" and i + 1 < len(args):
            names = args[i + 1].split(","); i += 2
        elif args[i] == "--out" and i + 1 < len(args):
            out = args[i + 1]; i += 2
        elif args[i] == "--timeout" and i + 1 < len(args):
            timeout = int(args[i + 1]); i += 2
        else:
            print(__doc__); return 2
    unknown = [n for n in names if n not in CASES]
    if unknown:
        print("unknown case(s): " + ",".join(unknown)); return 2
    qemu, grub, missing = find_tools()
    if missing:
        for n in names:
            print(f"NOT RUN {n}: missing {', '.join(missing)}")
        print("qemu_test: NOT RUN (no kernel was booted; this is not a pass)")
        return 3
    out = out or tempfile.mkdtemp(prefix="omni8-qemu.")
    failed = 0
    for n in names:
        res, detail = run_case(n, qemu, grub, out, timeout)
        print(f"{res} {n}: {detail}")
        failed += res != "PASS"
    print(f"qemu_test: {len(names) - failed} passed, {failed} failed; artifacts in {out}")
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
