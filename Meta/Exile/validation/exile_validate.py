#!/usr/bin/env python3
"""EXILE Kernel Validation Orchestrator.

One command that runs the kernel validation ladder, records what happened, and refuses to call
anything a pass that did not actually run. Standard library only. It never installs or changes
toolchains, never edits tracked files, and writes everything under --out.

Stages (ids are stable; see --list-stages):
  env.*        tool and version detection (rust pin + target, QEMU/GRUB/xorriso, zig, host tools)
  selftest.*   the existing Batch 8 transcript-matcher self-test
  host.*       cargo fmt / clippy / test for OMNI Batches 7-11, ExileKernel portable tests + audit
  static.*     host-clang syntax check of Kernel/ translation units (no build, no link)
  build.*      all seven Batch 8 variants through the existing scripts/build.sh, plus a repro check
  image.*      ELF + Multiboot validation (own parser) and the existing check_image/check_asm
  iso.* boot.* GRUB ISO and a real QEMU boot per variant (serial capture, milestones, panic,
               timeout, unexpected-reset detection)
  zig.*        Batches 2-6 regression through the pinned zig
  integrity.*  tracked files unchanged by the run; SHA-256 manifest of every artifact

Statuses: PASS, FAIL, BLOCKED (a prerequisite tool or stage is missing), NOT_RUN.
Overall: FAIL if any stage failed; BLOCKED if any is blocked; INCOMPLETE if only NOT_RUN remain;
PASS only when every stage passed. Exit codes: 0 PASS, 3 BLOCKED/INCOMPLETE, 4 FAIL, 2 usage.

Evidence classes: environment < static < host-test < build < runtime. Only `runtime` stages that
PASS (a real emulator booted the image and the transcript matched) support a boot claim.
"""
import argparse
import datetime
import hashlib
import importlib.util
import json
import os
import platform
import re
import shlex
import shutil
import signal
import struct
import subprocess
import sys
import time

sys.dont_write_bytecode = True  # importing qemu_test must not leave __pycache__ in the tree

TOOL_VERSION = "1.0.0"
SCHEMA_VERSION = 1
PASS, FAIL, BLOCKED, NOT_RUN = "PASS", "FAIL", "BLOCKED", "NOT_RUN"
HERE = os.path.dirname(os.path.abspath(__file__))
REPO = os.path.abspath(os.path.join(HERE, "..", "..", ".."))
EXP = os.path.join("Meta", "Experiments")
B8 = os.path.join(EXP, "OMNI-Kernel-Batch-8")
VARIANTS = ["base", "ring3", "preempt", "ring3-fault", "trap-fault", "magic-fault", "cpu-fault"]
EXPECT_PASS_STATUS, EXPECT_FAIL_STATUS = 33, 35
CARGO_BATCHES = [7, 8, 9, 10, 11]
SCHEDULER_TUS = ["Kernel/Tasks/Scheduler.cpp", "Kernel/Tasks/FinalizerTask.cpp", "Kernel/Tasks/Thread.cpp",
                 "Kernel/Boot/Exile/BootGuardian.cpp"]
PANIC_RE = re.compile(r"(?i)(kernel panic|panicked at|OMNI8 PANIC|OMNI8 FATAL EXCEPTION|triple fault|double fault|\bBUG:)")
QEMU_MIN = (6, 2)


# ----------------------------------------------------------------------------- generic helpers
def sha256_file(path):
    h = hashlib.sha256()
    with open(path, "rb") as f:
        for chunk in iter(lambda: f.read(1 << 20), b""):
            h.update(chunk)
    return h.hexdigest()


def now_iso():
    return datetime.datetime.now(datetime.timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")


def parse_version(text):
    """First dotted version in text as a tuple of ints, or None."""
    m = re.search(r"(\d+)\.(\d+)(?:\.(\d+))?", text or "")
    return tuple(int(x) for x in m.groups() if x is not None) if m else None


def version_str(t):
    return ".".join(str(x) for x in t) if t else None


def load_module(path, name):
    spec = importlib.util.spec_from_file_location(name, path)
    mod = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(mod)
    return mod


def read_pin(repo):
    """toolchain.pin of Batch 10: name=value lines, need_tool repeated."""
    pin = {"need_tool": []}
    p = os.path.join(repo, EXP, "OMNI-Kernel-Batch-10", "toolchain.pin")
    if not os.path.isfile(p):
        return pin
    with open(p, encoding="utf-8") as fh:
        pin_lines = fh.read().splitlines()
    for raw in pin_lines:
        raw = raw.strip()
        if not raw or raw.startswith("#") or "=" not in raw:
            continue
        k, v = raw.split("=", 1)
        if k == "need_tool":
            pin["need_tool"].append(v)
        else:
            pin[k] = v
    return pin


def probe_tool(name, version_args, env_override=None, timeout=20):
    path = (os.environ.get(env_override) if env_override else None) or shutil.which(name)
    if path and not os.path.exists(path):
        path = shutil.which(path)
    info = {"name": name, "found": bool(path), "path": path, "version_text": None, "version": None}
    if not path:
        return info
    try:
        r = subprocess.run([path, *version_args], capture_output=True, text=True, timeout=timeout)
        text = (r.stdout or r.stderr or "").strip().splitlines()
        info["version_text"] = text[0] if text else ""
        info["version"] = version_str(parse_version(info["version_text"]))
    except (OSError, subprocess.TimeoutExpired) as e:
        info["version_text"] = f"version probe failed: {e}"
    return info


def validate_build_info(info, variant, pin):
    """Return metadata defects; an ELF alone never proves a pinned build."""
    if not isinstance(info, str) or not info.strip():
        return ["build-info.txt missing or empty"]

    fields = {}
    for line in info.splitlines():
        if line.startswith("variant="):
            for item in line.split():
                if "=" in item:
                    key, value = item.split("=", 1)
                    fields[key] = value

    errors = []
    expected = {
        "variant": variant,
        "test_exit": "1",
        "profile": "release",
        "target": pin.get("rust_target", "x86_64-unknown-none"),
    }
    for key, value in expected.items():
        actual = fields.get(key)
        if actual != value:
            errors.append(
                f"build metadata {key}={actual!r}; expected {value!r}"
            )

    compiler = re.search(r"(?m)^rustc:\s+rustc\s+(\S+)", info)
    expected_compiler = pin.get("rustc")
    if not compiler:
        errors.append("build metadata has no rustc version")
    elif expected_compiler and compiler.group(1) != expected_compiler:
        errors.append(
            f"rustc version {compiler.group(1)!r}; expected {expected_compiler!r}"
        )

    if "host-fallback" in info or "host-target fallback" in info:
        errors.append("host-target fallback is forbidden")

    return errors


# ----------------------------------------------------------------------------- ELF / Multiboot
def validate_elf_multiboot(data):
    """Independent static validation of a Multiboot1 ELF64 kernel. Returns [(name, ok, detail)].

    Proves properties of the file only; it does not boot anything."""
    out = []

    def chk(name, ok, detail=""):
        out.append((name, bool(ok), detail))
        return ok

    if not chk("ELF magic", data[:4] == b"\x7fELF", f"first bytes {data[:4]!r}"):
        return out
    if not chk("ELF64 little-endian", len(data) >= 64 and data[4] == 2 and data[5] == 1,
               f"class={data[4] if len(data) > 4 else None} data={data[5] if len(data) > 5 else None}"):
        return out
    e_type, e_machine = struct.unpack_from("<HH", data, 16)
    e_entry, e_phoff = struct.unpack_from("<QQ", data, 24)
    e_phentsize, e_phnum = struct.unpack_from("<HH", data, 54)
    chk("type EXEC", e_type == 2, f"e_type={e_type}")
    chk("machine x86-64", e_machine == 62, f"e_machine={e_machine}")
    ok_ph = (
        e_phnum > 0 and e_phentsize == 56 and e_phoff >= 64
        and e_phoff <= len(data) and e_phnum <= (len(data) - e_phoff) // 56
    )
    if not chk("program headers readable", ok_ph, f"phnum={e_phnum} phentsize={e_phentsize} phoff={e_phoff}"):
        return out
    segs = []
    segment_errors = []
    max_u64 = (1 << 64) - 1
    for i in range(e_phnum):
        p_type, p_flags, p_off, p_vaddr, p_paddr, p_filesz, p_memsz, p_align = struct.unpack_from(
            "<IIQQQQQQ", data, e_phoff + i * 56
        )
        seg = {
            "type": p_type, "flags": p_flags, "off": p_off,
            "vaddr": p_vaddr, "filesz": p_filesz,
            "memsz": p_memsz, "align": p_align,
        }
        segs.append(seg)
        if p_type == 1:
            if p_filesz > p_memsz:
                segment_errors.append(f"PT_LOAD {i}: p_filesz exceeds p_memsz")
            if p_off > len(data) or p_filesz > len(data) - p_off:
                segment_errors.append(f"PT_LOAD {i}: file range exceeds ELF size")
            if p_memsz > max_u64 - p_vaddr:
                segment_errors.append(f"PT_LOAD {i}: virtual address range overflows")
            if p_align not in (0, 1):
                if p_align & (p_align - 1):
                    segment_errors.append(f"PT_LOAD {i}: alignment is not a power of two")
                elif (p_vaddr - p_off) % p_align:
                    segment_errors.append(f"PT_LOAD {i}: address and offset are incongruent")
    chk("PT_LOAD segment bounds and sizes valid", not segment_errors,
        "; ".join(segment_errors) if segment_errors else "all PT_LOAD ranges valid")
    loads = [s for s in segs if s["type"] == 1]
    chk("has PT_LOAD", bool(loads), f"{len(loads)} loadable segment(s)")
    chk("no interpreter/dynamic segment", not any(s["type"] in (2, 3) for s in segs), "PT_DYNAMIC/PT_INTERP absent")
    wx = [s for s in loads if s["flags"] & 1 and s["flags"] & 2]
    chk("no writable+executable segment", not wx, f"{len(wx)} W+X segment(s)")
    entry_seg = [s for s in loads if s["vaddr"] <= e_entry < s["vaddr"] + s["memsz"]]
    chk("entry point inside an executable PT_LOAD", entry_seg and entry_seg[0]["flags"] & 1,
        f"entry=0x{e_entry:x}")
    # Multiboot 1 header: 4-aligned, fully inside the first 8192 bytes of the file.
    found = None
    for off in range(0, min(8192, len(data) - 11), 4):
        magic, flags, checksum = struct.unpack_from("<III", data, off)
        if magic == 0x1BADB002:
            found = (off, flags, checksum)
            break
    if not chk("Multiboot1 magic within first 8192 bytes, 4-aligned", found, "not found" if not found else f"offset={found[0]}"):
        return out
    off, flags, checksum = found
    chk("Multiboot1 checksum", (0x1BADB002 + flags + checksum) & 0xFFFFFFFF == 0, f"flags=0x{flags:x}")
    chk("Multiboot1 requests memory map (flag bit 1)", flags & 2, f"flags=0x{flags:x}")
    chk("no unsupported required flags (bits 0-15 within 0x7)", (flags & 0xFFFF) & ~0x7 == 0, f"flags=0x{flags:x}")
    in_load = any(s["off"] <= off and off + 12 <= s["off"] + s["filesz"] for s in loads)
    chk("Multiboot header inside a loaded segment", in_load, f"offset={off}")
    return out


# ----------------------------------------------------------------------------- boot classification
def milestones(expected_text, serial, qt):
    """Ordered milestone table from the variant's expected transcript: which expected lines were reached."""
    mode, lines, _ = qt.parse_expected(expected_text)
    got = [l.rstrip("\r") for l in serial.splitlines() if not l.startswith("OMNI8 INFO:")]
    pos, table = 0, []
    for e in lines:
        hit = None
        j = pos
        while j < len(got):
            if qt.line_ok(e, got[j]):
                hit = j
                break
            j += 1
        if hit is None:
            table.append({"milestone": e, "reached": False})
        else:
            table.append({"milestone": e, "reached": True})
            pos = hit + 1
    return table


def classify_boot(variant, exit_status, timed_out, serial, qemu_log, expected_text, qt):
    """Pure verdict function (unit-tested). Returns dict(status, reasons, flags, milestones)."""
    want = EXPECT_PASS_STATUS if variant in ("base", "ring3", "preempt") else EXPECT_FAIL_STATUS
    reasons, flags = [], {"timeout": False, "panic": False, "unexpected_exit": False, "reset": False}
    triple = bool(re.search(r"(?i)triple fault", qemu_log or "")) or bool(re.search(r"(?i)triple fault", serial or ""))
    resets = len(re.findall(r"CPU Reset", qemu_log or ""))
    if timed_out:
        flags["timeout"] = True
        reasons.append("timeout: QEMU was still running at the deadline (hang)")
    elif exit_status not in (EXPECT_PASS_STATUS, EXPECT_FAIL_STATUS):
        flags["unexpected_exit"] = True
        reasons.append(
            f"unexpected QEMU exit status {exit_status} "
            "(not the isa-debug-exit 33/35 convention)"
        )
    elif exit_status != want:
        reasons.append(f"exit status {exit_status}, expected {want}")

    # A single startup CPU Reset is normal. Repeated resets or any triple
    # fault are failures, regardless of whether QEMU timed out or exited.
    if triple or resets > 1:
        flags["reset"] = True
        reasons.append(
            "triple fault reported" if triple
            else f"unexpected repeated CPU resets ({resets})"
        )
    if want == EXPECT_PASS_STATUS and PANIC_RE.search(serial or ""):
        flags["panic"] = True
        reasons.append("panic/fatal exception in a variant that must pass: " + PANIC_RE.search(serial).group(0))
    reasons += qt.compare(serial or "", expected_text)
    table = milestones(expected_text, serial or "", qt)
    return {"status": PASS if not reasons else FAIL, "reasons": reasons, "flags": flags,
            "want_exit": want, "exit_status": exit_status, "milestones": table,
            "resets_in_log": resets}


def run_qemu(cmd, serial_path, timeout):
    """Run QEMU with a hard deadline and terminate its process tree on timeout."""
    popen_options = {}
    if os.name == "nt":
        popen_options["creationflags"] = subprocess.CREATE_NEW_PROCESS_GROUP
    else:
        popen_options["start_new_session"] = True

    p = subprocess.Popen(
        cmd, stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL,
        stderr=subprocess.PIPE, **popen_options
    )
    try:
        _, err = p.communicate(timeout=timeout)
        return p.returncode, False, (err or b"").decode("utf-8", "replace")
    except subprocess.TimeoutExpired:
        if os.name == "nt":
            # taskkill /T terminates descendants as well as the direct child.
            try:
                subprocess.run(
                    ["taskkill", "/PID", str(p.pid), "/T", "/F"],
                    stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL,
                    stderr=subprocess.DEVNULL, check=False,
                )
            except OSError:
                pass
            if p.poll() is None:
                p.kill()
        else:
            try:
                os.killpg(p.pid, signal.SIGKILL)
            except ProcessLookupError:
                pass
            except PermissionError:
                if p.poll() is None:
                    p.kill()
        _, err = p.communicate()
        return None, True, (err or b"").decode("utf-8", "replace")


# ----------------------------------------------------------------------------- orchestrator
class Orchestrator:
    def __init__(self, repo, out, opts):
        self.repo, self.out, self.opts = repo, os.path.abspath(out), opts
        self.logs = os.path.join(self.out, "logs")
        self.work = os.path.join(self.out, "work")
        self.art = os.path.join(self.out, "artifacts")
        self.reports = os.path.join(self.out, "reports")
        for d in (self.logs, self.work, self.art, self.reports):
            os.makedirs(d, exist_ok=True)
        self.stages = {}
        self.order = []
        self.tools = {}
        self.pin = read_pin(repo)
        self.qt = None
        self.toolchain_class = "unknown"
        self.facts = {}

    # -- stage bookkeeping
    def selected(self, sid):
        only, skip = self.opts.only, self.opts.skip
        if only and not any(sid == o or sid.startswith(o + ".") or sid.startswith(o) for o in only):
            return False
        if skip and any(sid == s or sid.startswith(s + ".") or sid.startswith(s) for s in skip):
            return False
        return True

    def stage(self, sid, title, evidence, required=True):
        s = {"id": sid, "title": title, "evidence": evidence, "required": required, "status": NOT_RUN,
             "detail": "", "command": None, "rc": None, "duration_s": None, "log": None, "artifacts": [],
             "blocked_by": [], "data": {}}
        self.stages[sid] = s
        self.order.append(sid)
        return s

    def finish(self, s, status, detail, **kw):
        s["status"], s["detail"] = status, detail
        s.update(kw)
        print(f"[{status:8}] {s['id']}: {detail.splitlines()[0] if detail else ''}", flush=True)
        return s

    def skip_unselected(self, s):
        if not self.selected(s["id"]):
            self.finish(s, NOT_RUN, "not selected (--only/--skip)")
            return True
        return False

    def need(self, s, *ids):
        """Propagate prerequisite state: BLOCKED if a prerequisite is blocked, NOT_RUN if it failed."""
        bad = [i for i in ids if self.stages[i]["status"] != PASS]
        if not bad:
            return False
        s["blocked_by"] = bad
        states = {self.stages[i]["status"] for i in bad}
        status = BLOCKED if states <= {BLOCKED, NOT_RUN} and BLOCKED in states else NOT_RUN
        self.finish(s, status, "prerequisite stage(s) not passed: " + ", ".join(f"{i}={self.stages[i]['status']}" for i in bad))
        return True

    def run(self, s, argv, cwd=None, env=None, timeout=600, log_name=None):
        log = os.path.join(self.logs, (log_name or s["id"]) + ".log")
        s["command"] = " ".join(shlex.quote(a) for a in argv) + (f"   (cwd={cwd})" if cwd else "")
        s["log"] = os.path.relpath(log, self.out)
        e = dict(os.environ)
        e.update(env or {})
        t0 = time.time()
        try:
            r = subprocess.run(argv, cwd=cwd, env=e, capture_output=True, text=True, timeout=timeout)
            rc, text = r.returncode, (r.stdout or "") + (r.stderr or "")
        except subprocess.TimeoutExpired as ex:
            rc, text = 124, f"TIMEOUT after {timeout}s\n" + str(ex.stdout or "") + str(ex.stderr or "")
        except OSError as ex:
            rc, text = 127, f"cannot execute: {ex}"
        s["rc"], s["duration_s"] = rc, round(time.time() - t0, 2)
        with open(log, "w", encoding="utf-8", errors="replace") as f:
            f.write(f"$ {s['command']}\n# rc={rc}\n{text}")
        return rc, text

    def add_artifact(self, s, path):
        s["artifacts"].append(os.path.relpath(path, self.out))

    # -- stages
    def stage_env(self):
        pin = self.pin
        s = self.stage("env.rust-toolchain", "Rust toolchain matches the pin and has the bare-metal target", "environment")
        if not self.skip_unselected(s):
            rustc = probe_tool("rustc", ["--version"])
            cargo = probe_tool("cargo", ["--version"])
            self.tools.update(rustc=rustc, cargo=cargo)
            target = pin.get("rust_target", "x86_64-unknown-none")
            have_target = False
            if rustc["found"]:
                try:
                    sysroot = subprocess.run([rustc["path"], "--print", "sysroot"], capture_output=True, text=True, timeout=20).stdout.strip()
                    have_target = os.path.isdir(os.path.join(sysroot, "lib", "rustlib", target, "lib"))
                except (OSError, subprocess.TimeoutExpired):
                    pass
            want = pin.get("rustc")
            problems = []
            if not rustc["found"]:
                problems.append("rustc not found")
            elif want and rustc["version"] != want:
                problems.append(f"rustc {rustc['version']} != pinned {want}")
            if not cargo["found"]:
                problems.append("cargo not found")
            if not have_target:
                problems.append(f"target {target} not installed")
            s["data"] = {"rustc": rustc, "cargo": cargo, "pinned_rustc": want, "target": target, "target_installed": have_target}
            pinned = not problems
            self.toolchain_class = "pinned-bare-metal" if pinned else ("unavailable" if not rustc["found"] else "unpinned")
            self.finish(s, PASS if pinned else BLOCKED,
                        "pinned toolchain and target present" if pinned else "; ".join(problems) +
                        " (kernel builds are blocked unless the pinned toolchain and target are available)")

        s = self.stage("env.runtime-tools", "QEMU, GRUB and xorriso available for real boots", "environment")
        if not self.skip_unselected(s):
            qemu = probe_tool("qemu-system-x86_64", ["--version"], "EXILE_VALIDATE_QEMU")
            grub = probe_tool("grub-mkrescue", ["--version"], "EXILE_VALIDATE_GRUB_MKRESCUE")
            if not grub["found"]:
                grub = probe_tool("grub2-mkrescue", ["--version"])
            qemu_aarch64 = probe_tool("qemu-system-aarch64", ["--version"])
            xorriso = probe_tool("xorriso", ["-version"])
            mformat = probe_tool("mformat", ["--version"])
            self.tools.update(qemu=qemu, qemu_aarch64=qemu_aarch64,
                              grub=grub, xorriso=xorriso, mformat=mformat)
            if qemu["found"]:
                qemu["sha256"] = sha256_file(qemu["path"])
            problems = []
            if not qemu["found"]:
                problems.append("qemu-system-x86_64 not found")
            else:
                v = parse_version(qemu["version_text"])
                if v is None or v[:2] < QEMU_MIN:
                    problems.append(f"QEMU {version_str(v)} older than {version_str(QEMU_MIN)}")
            if "qemu-system-aarch64" in pin.get("need_tool", []) and not qemu_aarch64["found"]:
                problems.append("qemu-system-aarch64 not found (required by toolchain.pin)")
            if not grub["found"]:
                problems.append("grub-mkrescue not found")
            if not xorriso["found"]:
                problems.append("xorriso not found")
            s["data"] = {"qemu": qemu, "qemu_aarch64": qemu_aarch64,
                         "grub": grub, "xorriso": xorriso, "mformat": mformat}
            self.finish(s, BLOCKED if problems else PASS, "; ".join(problems) if problems else "required QEMU, GRUB and xorriso tools found")

        s = self.stage("env.zig", "Zig matches the pin (Batches 2-6)", "environment")
        if not self.skip_unselected(s):
            zig = probe_tool("zig", ["version"], "ZIG")
            self.tools["zig"] = zig
            want = pin.get("zig")
            s["data"] = {"zig": zig, "pinned": want}
            if not zig["found"]:
                self.finish(s, BLOCKED, "zig not found")
            elif want and zig["version"] != want:
                self.finish(s, BLOCKED, f"zig {zig['version']} != pinned {want}")
            else:
                self.finish(s, PASS, f"zig {zig['version']}")

        s = self.stage("env.host-tools", "Host tools used by the stages (python3, git, gcc, ld, objcopy, clang)", "environment")
        if not self.skip_unselected(s):
            found = {}
            for n in ("python3", "git", "gcc", "ld", "objcopy", "clang"):
                found[n] = probe_tool(n, ["--version"])
            self.tools.update({k: v for k, v in found.items() if k not in self.tools})
            missing = [n for n, i in found.items() if not i["found"]]
            s["data"] = found
            self.finish(s, BLOCKED if missing else PASS, ("missing: " + ", ".join(missing)) if missing else "all present")

    def stage_selftest(self):
        s = self.stage("selftest.qemu-matcher", "Existing Batch 8 transcript matcher self-test (boots nothing)", "static")
        if self.skip_unselected(s):
            return
        rc, text = self.run(s, [sys.executable, os.path.join(self.repo, B8, "scripts", "qemu_test.py"), "--selftest"])
        m = re.search(r"(\d+) passed, (\d+) failed", text)
        self.finish(s, PASS if rc == 0 and m and m.group(2) == "0" else FAIL, m.group(0) if m else f"rc={rc}")

    def stage_host(self):
        cargo = (self.tools.get("cargo") or {}).get("path") or shutil.which("cargo")
        tdir = os.path.join(self.work, "cargo-target")
        for n in CARGO_BATCHES:
            d = os.path.join(self.repo, EXP, f"OMNI-Kernel-Batch-{n}")
            for kind in ("fmt", "clippy", "test"):
                s = self.stage(f"host.b{n}.{kind}", f"Batch {n}: cargo {kind}", "host-test")
                if self.skip_unselected(s):
                    continue
                if not cargo or not os.path.isdir(d):
                    self.finish(s, BLOCKED, "cargo or the batch directory is missing")
                    continue
                env = {"CARGO_TARGET_DIR": tdir}
                if kind == "fmt":
                    rc, text = self.run(s, [cargo, "fmt", "--check"], cwd=d, env=env)
                    self.finish(s, PASS if rc == 0 else FAIL, "formatted" if rc == 0 else "formatting differences")
                elif kind == "clippy":
                    rc, text = self.run(s, [cargo, "clippy", "--offline", "--all-targets", "--", "-D", "warnings"], cwd=d, env=env, timeout=900)
                    errs = [l for l in text.splitlines() if l.startswith(("error", "warning"))][:3]
                    note = ""
                    if rc != 0 and self.toolchain_class != "pinned-bare-metal":
                        s["data"]["toolchain_note"] = ("clippy ran with a non-pinned rustc; lints added after the pin may be the cause. "
                                                       "Re-run with the pinned toolchain before treating this as a code defect.")
                        note = " [non-pinned rustc: re-check with the pin]"
                    self.finish(s, PASS if rc == 0 else FAIL, ("no warnings" if rc == 0 else "; ".join(errs) or f"rc={rc}") + note)
                else:
                    rc, text = self.run(s, [cargo, "test", "--offline"], cwd=d, env=env, timeout=900)
                    passed = sum(int(x) for x in re.findall(r"test result: \w+\. (\d+) passed", text))
                    failed = sum(int(x) for x in re.findall(r"; (\d+) failed", text))
                    s["data"] = {"passed": passed, "failed": failed}
                    ok = rc == 0 and failed == 0 and passed > 0
                    self.finish(s, PASS if ok else FAIL, f"{passed} passed, {failed} failed" + ("" if passed else " (no tests ran)"))
        s = self.stage("host.euk.tests", "ExileKernel (EUK) portable tests + dependency audit", "host-test")
        if not self.skip_unselected(s):
            src = os.path.join(self.repo, "ExileKernel")
            rustc = (self.tools.get("rustc") or {}).get("path") or shutil.which("rustc")
            if not os.path.isdir(src) or not rustc:
                self.finish(s, BLOCKED, "ExileKernel or rustc missing")
            else:
                copy = os.path.join(self.work, "ExileKernel")
                shutil.rmtree(copy, ignore_errors=True)
                shutil.copytree(src, copy, ignore=shutil.ignore_patterns("build", "__pycache__"))
                rc, text = self.run(s, [sys.executable, "scripts/test.py"], cwd=copy, env={"EUK_HOST_RUSTC": rustc}, log_name="host.euk.tests")
                passed = sum(int(x) for x in re.findall(r"test result: \w+\. (\d+) passed", text))
                rc2, text2 = self.run(s, [sys.executable, "scripts/audit.py"], cwd=copy, log_name="host.euk.audit")
                ok = rc == 0 and rc2 == 0 and passed > 0
                s["data"] = {"passed": passed, "audit_rc": rc2, "note": "run on a copy so the work tree is untouched; bare-metal build needs the pinned toolchain"}
                self.finish(s, PASS if ok else FAIL, f"{passed} tests passed; audit rc={rc2}")

    def stage_static(self):
        s = self.stage("static.kernel-syntax", "Kernel/ syntax + type check with host clang (static only; no build)", "static")
        if self.skip_unselected(s):
            return
        mode = self.opts.kernel_syntax
        script = os.path.join(self.repo, EXP, "OMNI-Kernel-Batch-11", "scripts", "kernel_syntax_check.sh")
        if mode == "skip":
            self.finish(s, NOT_RUN, "disabled with --kernel-syntax skip")
            return
        if not shutil.which("clang") and not shutil.which("clang++"):
            self.finish(s, BLOCKED, "clang not found")
            return
        argv = ["bash", script, self.repo] + ([] if mode == "full" else SCHEDULER_TUS)
        rc, text = self.run(s, argv, timeout=3000 if mode == "full" else 600)
        m = re.search(r"(\d+) passed, (\d+) failed, (\d+) not checked", text)
        if not m:
            self.finish(s, FAIL, f"could not parse the checker output (rc={rc})")
            return
        p, f, nc = map(int, m.groups())
        s["data"] = {"passed": p, "failed": f, "not_checked": nc, "mode": mode}
        self.finish(s, PASS if rc == 0 and f == 0 and p > 0 else FAIL, f"{p} passed, {f} failed, {nc} not checked ({mode}); static only")

    def build_variant(self, variant, dest, s):
        script = os.path.join(self.repo, B8, "scripts", "build.sh")
        return self.run(s, ["bash", script, "--variant", variant, "--test-exit",
                            "--target", self.pin.get("rust_target", "x86_64-unknown-none"),
                            "--out", dest], timeout=900,
                        env={"SOURCE_DATE_EPOCH": "0", "TZ": "UTC", "LC_ALL": "C"}, log_name=s["id"])

    def stage_build(self):
        for v in VARIANTS:
            s = self.stage(f"build.{v}", f"Batch 8 variant '{v}' builds", "build")
            if self.skip_unselected(s):
                continue
            if self.need(s, "env.rust-toolchain"):
                continue

            dest = os.path.join(self.work, f"build-{v}")
            shutil.rmtree(dest, ignore_errors=True)
            rc, output = self.build_variant(v, dest, s)
            elf = os.path.join(dest, "kernel.elf")
            info_path = os.path.join(dest, "build-info.txt")

            if rc != 0 or not os.path.isfile(elf):
                self.finish(s, FAIL,
                            f"build.sh rc={rc}; kernel.elf "
                            f"{'missing' if not os.path.isfile(elf) else 'present'}")
                continue

            info = ""
            if os.path.isfile(info_path):
                with open(info_path, encoding="utf-8", errors="replace") as f:
                    info = f.read()

            metadata_errors = validate_build_info(info, v, self.pin)
            if metadata_errors:
                self.finish(s, FAIL, "; ".join(metadata_errors))
                continue

            adir = os.path.join(self.art, v)
            os.makedirs(adir, exist_ok=True)
            for name in ("kernel.elf", "kernel32.elf", "build-info.txt"):
                source = os.path.join(dest, name)
                if os.path.isfile(source):
                    shutil.copy2(source, adir)
                    self.add_artifact(s, os.path.join(adir, name))

            warnings = len(re.findall(r"(?m)^warning", output))
            s["data"] = {
                "sha256": sha256_file(elf),
                "bytes": os.path.getsize(elf),
                "target": self.pin.get("rust_target"),
                "compiler": self.pin.get("rustc"),
                "compiler_warnings": warnings,
            }
            if self.opts.deny_warnings and warnings:
                self.finish(s, FAIL,
                            f"{warnings} compiler warning(s) and --deny-warnings is set")
            else:
                self.finish(
                    s, PASS,
                    f"verified pinned build: {s['data']['bytes']} bytes; "
                    f"target={s['data']['target']}; compiler={s['data']['compiler']}; "
                    f"warnings={warnings}"
                )

        s = self.stage("build.reproducible",
                       "Two pinned builds of 'base' are byte-identical", "build")
        if self.skip_unselected(s):
            return
        if self.need(s, "env.rust-toolchain", "build.base"):
            return

        dest = os.path.join(self.work, "build-base-repro")
        shutil.rmtree(dest, ignore_errors=True)
        rc, output = self.build_variant("base", dest, s)
        elf = os.path.join(dest, "kernel.elf")
        info_path = os.path.join(dest, "build-info.txt")

        if rc != 0 or not os.path.isfile(elf):
            self.finish(s, FAIL, f"second build failed rc={rc}")
            return

        info = ""
        if os.path.isfile(info_path):
            with open(info_path, encoding="utf-8", errors="replace") as f:
                info = f.read()
        metadata_errors = validate_build_info(info, "base", self.pin)
        if metadata_errors:
            self.finish(s, FAIL,
                        "second build metadata rejected: " + "; ".join(metadata_errors))
            return

        first = self.stages["build.base"]["data"].get("sha256")
        second = sha256_file(elf)
        s["data"] = {"first": first, "second": second}
        self.finish(
            s, PASS if first == second else FAIL,
            "identical SHA-256 " + second[:16]
            if first == second else f"differ: {first[:16]} vs {second[:16]}"
        )

    def stage_image(self):
        scripts = os.path.join(self.repo, B8, "scripts")
        for v in VARIANTS:
            s = self.stage(f"image.{v}", f"'{v}': ELF + Multiboot validation", "static")
            if self.skip_unselected(s) or self.need(s, f"build.{v}"):
                continue
            elf = os.path.join(self.art, v, "kernel.elf")
            checks = validate_elf_multiboot(open(elf, "rb").read())
            bad = [c for c in checks if not c[1]]
            s["data"] = {"checks": [{"name": n, "ok": ok, "detail": d} for n, ok, d in checks]}
            rc1, _ = self.run(s, [sys.executable, os.path.join(scripts, "check_image.py"), elf], log_name=f"image.{v}.check_image")
            rc2, _ = self.run(s, [sys.executable, os.path.join(scripts, "check_asm.py"), elf], log_name=f"image.{v}.check_asm")
            s["data"]["check_image_rc"], s["data"]["check_asm_rc"] = rc1, rc2
            ok = not bad and rc1 == 0 and rc2 == 0
            self.finish(s, PASS if ok else FAIL,
                        f"{len(checks)} own checks passed; check_image rc={rc1}; check_asm rc={rc2}" if ok
                        else "failed: " + ", ".join([b[0] for b in bad] + ([f"check_image rc={rc1}"] if rc1 else []) + ([f"check_asm rc={rc2}"] if rc2 else [])))

    def stage_boot(self):
        qt_path = os.path.join(self.repo, B8, "scripts", "qemu_test.py")
        self.qt = load_module(qt_path, "omni8_qemu_test")
        qemu_args = [a for a in self.qt.QEMU_ARGS]
        # Serial goes to a file instead of stdio so nothing depends on pipe buffering.
        i = qemu_args.index("-serial")
        qemu_args[i + 1] = "@SERIAL@"
        for v in VARIANTS:
            si = self.stage(f"iso.{v}", f"'{v}': GRUB ISO", "build")
            sb = self.stage(f"boot.{v}", f"'{v}': boots under QEMU and matches the expected transcript", "runtime")
            if not self.skip_unselected(si):
                if not self.need(si, "env.runtime-tools", f"image.{v}"):
                    self.make_iso(v, si)
            if self.skip_unselected(sb):
                continue
            if self.need(sb, "env.runtime-tools", f"iso.{v}"):
                continue
            self.boot(v, sb, qemu_args)

    def make_iso(self, v, s):
        grub = self.tools["grub"]["path"]
        stage = os.path.join(self.work, f"iso-root-{v}")
        shutil.rmtree(stage, ignore_errors=True)
        os.makedirs(os.path.join(stage, "boot", "grub"))
        shutil.copy(os.path.join(self.art, v, "kernel.elf"), os.path.join(stage, "boot", "omni8.elf"))
        shutil.copy(os.path.join(self.repo, B8, "boot", "grub.cfg"), os.path.join(stage, "boot", "grub", "grub.cfg"))
        iso = os.path.join(self.art, v, "omni8.iso")
        rc, _ = self.run(s, [grub, "-o", iso, stage])
        if rc != 0 or not os.path.isfile(iso):
            self.finish(s, FAIL, f"grub-mkrescue rc={rc}")
            return
        s["data"] = {"sha256": sha256_file(iso), "bytes": os.path.getsize(iso)}
        self.add_artifact(s, iso)
        self.finish(s, PASS, f"omni8.iso {s['data']['bytes']} bytes")

    def boot(self, v, s, qemu_args):
        qemu = self.tools["qemu"]
        adir = os.path.join(self.art, v)
        serial, qlog = os.path.join(adir, "serial.txt"), os.path.join(adir, "qemu-debug.log")
        iso = os.path.join(adir, "omni8.iso")
        args = [serial if a == "@SERIAL@" else a for a in qemu_args]
        args = [("file:" + serial) if a == serial else a for a in args]
        cmd = [qemu["path"], *args, "-d", "cpu_reset,guest_errors", "-D", qlog, "-cdrom", iso]
        s["command"] = " ".join(shlex.quote(a) for a in cmd)
        for p in (serial, qlog):
            if os.path.exists(p):
                os.remove(p)
        t0 = time.time()
        status, timed_out, stderr = run_qemu(cmd, serial, self.opts.timeout)
        s["rc"], s["duration_s"] = status, round(time.time() - t0, 2)
        text = open(serial, errors="replace").read() if os.path.isfile(serial) else ""
        ql = open(qlog, errors="replace").read() if os.path.isfile(qlog) else ""
        expected = open(os.path.join(self.repo, B8, "tests", "expected", v + ".txt")).read()
        res = classify_boot(v, status, timed_out, text, ql, expected, self.qt)
        with open(os.path.join(self.logs, f"boot.{v}.log"), "w", encoding="utf-8") as f:
            f.write(f"$ {s['command']}\n# exit status: {status}  timed_out: {timed_out}\n# qemu stderr:\n{stderr}\n# serial:\n{text}\n")
        s["log"] = f"logs/boot.{v}.log"
        for p in (serial, qlog):
            if os.path.isfile(p):
                self.add_artifact(s, p)
        reached = sum(1 for m in res["milestones"] if m["reached"])
        s["data"] = {k: res[k] for k in ("flags", "want_exit", "exit_status", "milestones", "resets_in_log")}
        s["data"]["qemu_version"] = qemu["version_text"]
        s["data"]["milestones_reached"] = f"{reached}/{len(res['milestones'])}"
        if res["status"] == PASS:
            self.finish(s, PASS, f"exit {status}, transcript matched, milestones {reached}/{len(res['milestones'])}")
        else:
            self.finish(s, FAIL, "; ".join(res["reasons"][:3]) + f" (milestones reached {reached}/{len(res['milestones'])})")

    def stage_zig(self):
        s = self.stage("zig.regression", "Batches 2-6 regression (`zig build test`, as Batch 8 verify.sh runs it)", "host-test")
        if self.skip_unselected(s) or self.need(s, "env.zig"):
            return
        zig = self.tools["zig"]["path"]
        d = os.path.join(self.repo, EXP, "OMNI-Kernel-Batch-6")
        rc, text = self.run(s, [zig, "build", "test", "--summary", "all", "--cache-dir", os.path.join(self.work, "zig-cache"),
                                "--global-cache-dir", os.path.join(self.work, "zig-global")], cwd=d, timeout=1800)
        m = re.findall(r"Build Summary[^\n]*", text)
        self.finish(s, PASS if rc == 0 else FAIL, (m[-1] if m else f"rc={rc}"))

    # -- integrity + reports
    def git(self, *args):
        try:
            return subprocess.run(["git", "-C", self.repo, *args], capture_output=True, text=True, timeout=60).stdout
        except (OSError, subprocess.TimeoutExpired):
            return ""

    def git_state(self):
        return {"head": self.git("rev-parse", "HEAD").strip(), "branch": self.git("rev-parse", "--abbrev-ref", "HEAD").strip(),
                "tracked_changes": [l for l in self.git("status", "--porcelain", "--untracked-files=no").splitlines()]}

    def finish_run(self, before):
        s = self.stage("integrity.tracked-files", "Validation left tracked files exactly as they were", "static")
        after = self.git_state()
        if not before["head"]:
            self.finish(s, BLOCKED, "not a git work tree; cannot verify")
        else:
            same = before["tracked_changes"] == after["tracked_changes"] and before["head"] == after["head"]
            s["data"] = {"before": before["tracked_changes"], "after": after["tracked_changes"]}
            self.finish(s, PASS if same else FAIL, "tracked files unchanged" if same else "tracked files changed during validation")
        return after

    def summary(self):
        counts = {k: 0 for k in (PASS, FAIL, BLOCKED, NOT_RUN)}
        for sid in self.order:
            counts[self.stages[sid]["status"]] += 1
        req = [self.stages[i]["status"] for i in self.order if self.stages[i]["required"]]
        overall = FAIL if FAIL in req else BLOCKED if BLOCKED in req else "INCOMPLETE" if NOT_RUN in req else PASS
        boots = [self.stages[f"boot.{v}"]["status"] for v in VARIANTS if f"boot.{v}" in self.stages]
        runtime = len(boots) == len(VARIANTS) and all(b == PASS for b in boots)
        limits = []
        if not runtime:
            limits.append("NO runtime claim is supported: not every variant booted under QEMU and matched its transcript.")
        if self.toolchain_class != "pinned-bare-metal":
            limits.append(f"Builds used toolchain class '{self.toolchain_class}', not the pinned bare-metal toolchain; "
                          "results are not evidence for the pinned compiler.")
        limits.append("Static (syntax/ELF/Multiboot) and host-test results are not runtime evidence.")
        return {"overall": overall, "counts": counts, "runtime_validated": runtime, "toolchain_class": self.toolchain_class,
                "limits": limits}

    def write_reports(self, git_before, git_after, started):
        summ = self.summary()
        artifacts = []
        for base in (self.logs, self.art):
            for root, _, files in os.walk(base):
                for f in sorted(files):
                    p = os.path.join(root, f)
                    artifacts.append({"path": os.path.relpath(p, self.out), "sha256": sha256_file(p), "bytes": os.path.getsize(p)})
        report = {
            "schema_version": SCHEMA_VERSION, "tool": "exile_validate.py", "tool_version": TOOL_VERSION,
            "started": started, "finished": now_iso(),
            "host": {"platform": platform.platform(), "python": sys.version.split()[0], "machine": platform.machine()},
            "git": {"head": git_before["head"], "branch": git_before["branch"], "tracked_changes_before": git_before["tracked_changes"],
                    "tracked_changes_after": git_after["tracked_changes"]},
            "options": {k: v for k, v in vars(self.opts).items()}, "pin": self.pin, "tools": self.tools,
            "summary": summ, "stages": [self.stages[i] for i in self.order], "artifacts": artifacts,
        }
        jp = os.path.join(self.reports, "validation-report.json")
        with open(jp, "w", encoding="utf-8") as f:
            json.dump(report, f, indent=2, sort_keys=True)
            f.write("\n")
        mp = os.path.join(self.reports, "validation-report.md")
        with open(mp, "w", encoding="utf-8") as f:
            f.write(self.markdown(report))
        manifest = os.path.join(self.out, "MANIFEST.sha256")
        lines = []
        for root, _, files in os.walk(self.out):
            if os.path.relpath(root, self.out).split(os.sep)[0] == "work":
                continue
            for fn in sorted(files):
                p = os.path.join(root, fn)
                if p != manifest:
                    lines.append(f"{sha256_file(p)}  {os.path.relpath(p, self.out)}")
        with open(manifest, "w", encoding="utf-8") as f:
            f.write("\n".join(sorted(lines, key=lambda l: l.split("  ", 1)[1])) + "\n")
        return report

    def markdown(self, r):
        s = r["summary"]
        out = [f"# EXILE Kernel Validation Report", "",
               f"- Overall: **{s['overall']}**   (PASS {s['counts'][PASS]}, FAIL {s['counts'][FAIL]}, BLOCKED {s['counts'][BLOCKED]}, NOT_RUN {s['counts'][NOT_RUN]})",
               f"- Runtime validated (all 7 variants booted under QEMU and matched): **{'YES' if s['runtime_validated'] else 'NO'}**",
               f"- Toolchain class: `{s['toolchain_class']}`",
               f"- Git: `{r['git']['head'][:12]}` on `{r['git']['branch']}`; tracked changes before/after: {len(r['git']['tracked_changes_before'])}/{len(r['git']['tracked_changes_after'])}",
               f"- Started {r['started']}, finished {r['finished']}; tool v{r['tool_version']}", "", "## What this report can and cannot claim", ""]
        out += [f"- {l}" for l in s["limits"]]
        out += ["", "## Stages", "", "| Stage | Evidence | Status | Detail |", "|---|---|---|---|"]
        for st in r["stages"]:
            d = (st["detail"] or "").splitlines()[0].replace("|", "\\|") if st["detail"] else ""
            out.append(f"| `{st['id']}` | {st['evidence']} | **{st['status']}** | {d} |")
        boots = [st for st in r["stages"] if st["id"].startswith("boot.") and st["data"].get("milestones")]
        if boots:
            out += ["", "## Boot milestones", ""]
            for st in boots:
                out += [f"### {st['id']} (exit {st['data'].get('exit_status')}, expected {st['data'].get('want_exit')}, reached {st['data'].get('milestones_reached')})", ""]
                out += [f"- [{'x' if m['reached'] else ' '}] `{m['milestone']}`" for m in st["data"]["milestones"]]
                out.append("")
        blocked = [st for st in r["stages"] if st["status"] == BLOCKED]
        if blocked:
            out += ["## Blockers", ""] + [f"- `{st['id']}`: {st['detail']}" for st in blocked] + [""]
        out += ["## Reproduce", "", "```", "python3 Meta/Exile/validation/exile_validate.py --out <dir>", "```", "",
                f"Artifacts are listed with SHA-256 in `MANIFEST.sha256` and in the JSON report ({len(r['artifacts'])} files).", ""]
        return "\n".join(out)

    def run_all(self):
        started = now_iso()
        before = self.git_state()
        self.stage_env()
        self.stage_selftest()
        if not self.opts.no_host_tests:
            self.stage_host()
        else:
            for n in CARGO_BATCHES:
                for kind in ("fmt", "clippy", "test"):
                    s = self.stage(f"host.b{n}.{kind}", f"Batch {n}: cargo {kind}", "host-test")
                    self.finish(s, NOT_RUN, "disabled with --no-host-tests")
            s = self.stage("host.euk.tests", "ExileKernel (EUK) portable tests + dependency audit", "host-test")
            self.finish(s, NOT_RUN, "disabled with --no-host-tests")
        self.stage_static()
        self.stage_build()
        self.stage_image()
        self.stage_boot()
        self.stage_zig()
        after = self.finish_run(before)
        return self.write_reports(before, after, started)


def stage_ids():
    ids = ["env.rust-toolchain", "env.runtime-tools", "env.zig", "env.host-tools", "selftest.qemu-matcher"]
    for n in CARGO_BATCHES:
        ids += [f"host.b{n}.{k}" for k in ("fmt", "clippy", "test")]
    ids += ["host.euk.tests", "static.kernel-syntax"] + [f"build.{v}" for v in VARIANTS] + ["build.reproducible"]
    ids += [f"image.{v}" for v in VARIANTS]
    for v in VARIANTS:
        ids += [f"iso.{v}", f"boot.{v}"]
    return ids + ["zig.regression", "integrity.tracked-files"]


def main(argv=None):
    ap = argparse.ArgumentParser(description="EXILE Kernel Validation Orchestrator", formatter_class=argparse.RawDescriptionHelpFormatter,
                                 epilog="Exit: 0 PASS, 3 BLOCKED/INCOMPLETE, 4 FAIL, 2 usage.")
    ap.add_argument("--repo", default=REPO, help="repository root (default: the checkout containing this script)")
    ap.add_argument("--out", help="output directory (default: <repo>/Build/exile-validation/<UTC timestamp>)")
    ap.add_argument("--only", default="", help="comma-separated stage ids or prefixes to run (others are reported NOT_RUN)")
    ap.add_argument("--skip", default="", help="comma-separated stage ids or prefixes to skip (reported NOT_RUN)")
    ap.add_argument("--timeout", type=int, default=90, help="per-variant QEMU deadline in seconds (default 90)")
    ap.add_argument("--kernel-syntax", choices=["scheduler", "full", "skip"], default="scheduler")
    ap.add_argument("--deny-warnings", action="store_true", help="fail a build stage that emits compiler warnings")
    ap.add_argument("--no-host-tests", action="store_true", help="do not run cargo fmt/clippy/test and EUK tests")
    ap.add_argument("--list-stages", action="store_true")
    a = ap.parse_args(argv)
    if a.list_stages:
        print("\n".join(stage_ids()))
        return 0
    a.only = [x for x in a.only.split(",") if x]
    a.skip = [x for x in a.skip.split(",") if x]
    ids = stage_ids()
    for sel in a.only + a.skip:
        if not any(i == sel or i.startswith(sel) for i in ids):
            print(f"unknown stage selector: {sel} (see --list-stages)", file=sys.stderr)
            return 2
    repo = os.path.abspath(a.repo)
    if not os.path.isdir(os.path.join(repo, B8)):
        print(f"{repo} does not contain {B8}", file=sys.stderr)
        return 2
    out = a.out or os.path.join(repo, "Build", "exile-validation", time.strftime("%Y%m%dT%H%M%SZ", time.gmtime()))
    orch = Orchestrator(repo, out, a)
    report = orch.run_all()
    s = report["summary"]
    print(f"\nOVERALL {s['overall']}  runtime_validated={s['runtime_validated']}  counts={s['counts']}")
    print(f"reports: {os.path.join(out, 'reports')}   manifest: {os.path.join(out, 'MANIFEST.sha256')}")
    return {PASS: 0, FAIL: 4}.get(s["overall"], 3)


if __name__ == "__main__":
    sys.exit(main())
