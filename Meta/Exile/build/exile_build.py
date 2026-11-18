#!/usr/bin/env python3
"""EXILE bare-metal build pipeline (Work Order 1).

Builds the seven Batch 8 kernel variants for the *pinned bare-metal target* and refuses everything else.
It wraps the existing `OMNI-Kernel-Batch-8/scripts/build.sh` (which is left untouched) by always passing
`--target <pin>`, and it independently re-checks the result instead of trusting the wrapper.

Commands
  preflight   check pinned rustc / target / tools / sources; exit 0 PASS, 3 BLOCKED
  build       preflight, then build + inspect each variant; writes build-manifest.json + SHA256SUMS
  iso         build + inspect a GRUB ISO per built variant (needs grub-mkrescue + xorriso)
  all         build then iso
  inspect P   validate an ELF (or a build directory) and say whether it would be ACCEPTED
  verify-manifest M   re-hash every artifact in a manifest and re-apply the acceptance rules

There is deliberately NO host-fallback option. If the pinned compiler or the bare-metal target is missing the
result is BLOCKED with the precise reason and no artifact is produced. An artifact whose provenance shows
anything other than the pinned compiler + pinned target is REJECTED even if it is a structurally valid ELF.

Exit codes: 0 PASS, 3 BLOCKED, 4 FAIL/REJECTED, 2 usage.
Never installs or modifies a toolchain. Standard library only.
"""
import argparse
import hashlib
import json
import os
import re
import shlex
import shutil
import struct
import subprocess
import sys
import time

sys.dont_write_bytecode = True
TOOL_VERSION = "1.0.0"
SCHEMA_VERSION = 1
HERE = os.path.dirname(os.path.abspath(__file__))
REPO = os.path.abspath(os.path.join(HERE, "..", "..", ".."))
EXP = os.path.join("Meta", "Experiments")
B7 = os.path.join(EXP, "OMNI-Kernel-Batch-7")
B8 = os.path.join(EXP, "OMNI-Kernel-Batch-8")
PIN_FILE = os.path.join(EXP, "OMNI-Kernel-Batch-10", "toolchain.pin")
VARIANTS = ["base", "ring3", "preempt", "ring3-fault", "trap-fault", "magic-fault", "cpu-fault"]
HOST_TRIPLE_MARKERS = [b"x86_64-unknown-linux-gnu", b"x86_64-unknown-linux-musl", b"aarch64-unknown-linux-gnu",
                       b"x86_64-apple-darwin", b"x86_64-pc-windows"]
PASS, FAIL, BLOCKED, REJECTED = "PASS", "FAIL", "BLOCKED", "REJECTED"
REQUIRED_SECTIONS = {  # name -> (must have flags, must NOT have flags) using ELF SHF_* bits
    ".text": (0x2 | 0x4, 0x1),   # ALLOC+EXEC, not WRITE
    ".rodata": (0x2, 0x1 | 0x4), # ALLOC, not WRITE, not EXEC
    ".data": (0x2 | 0x1, 0x4),   # ALLOC+WRITE, not EXEC
    ".bss": (0x2 | 0x1, 0x4),
}


# ----------------------------------------------------------------------------- helpers
def sha256_file(path):
    h = hashlib.sha256()
    with open(path, "rb") as f:
        for chunk in iter(lambda: f.read(1 << 20), b""):
            h.update(chunk)
    return h.hexdigest()


def now_iso():
    return time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime())


def read_pin(repo):
    pin = {"need_tool": []}
    p = os.path.join(repo, PIN_FILE)
    if not os.path.isfile(p):
        return None
    with open(p, encoding="utf-8") as f:
        for raw in f.read().splitlines():
            raw = raw.strip()
            if not raw or raw.startswith("#") or "=" not in raw:
                continue
            k, v = raw.split("=", 1)
            if k == "need_tool":
                pin["need_tool"].append(v)
            else:
                pin[k] = v
    return pin


def run(argv, env=None, cwd=None, timeout=900):
    try:
        r = subprocess.run(argv, env=env, cwd=cwd, capture_output=True, text=True, timeout=timeout)
        return r.returncode, (r.stdout or "") + (r.stderr or "")
    except subprocess.TimeoutExpired as ex:
        return 124, f"TIMEOUT after {timeout}s {ex}"
    except OSError as ex:
        return 127, f"cannot execute {argv[0]}: {ex}"


def first_line(text):
    return (text or "").strip().splitlines()[0] if (text or "").strip() else ""


def git(repo, *args):
    rc, out = run(["git", "-C", repo, *args], timeout=60)
    return out.strip() if rc == 0 else ""


def source_identity(repo):
    """Source revision + a content digest of exactly the files the kernel build reads."""
    files = []
    for rel in (B7, B8):
        for line in git(repo, "ls-files", rel).splitlines():
            if line.startswith(os.path.join(rel, "docs")) or line.startswith(os.path.join(rel, "tests")):
                continue
            files.append(line)
    h = hashlib.sha256()
    for rel in sorted(files):
        p = os.path.join(repo, rel)
        if os.path.isfile(p):
            h.update(rel.encode() + b"\0" + sha256_file(p).encode() + b"\n")
    dirty = [l for l in git(repo, "status", "--porcelain", "--untracked-files=no").splitlines()]
    return {"git_head": git(repo, "rev-parse", "HEAD"), "git_branch": git(repo, "rev-parse", "--abbrev-ref", "HEAD"),
            "commit_time": git(repo, "log", "-1", "--format=%ct"), "dirty_tracked_files": dirty,
            "build_input_files": len(files), "build_input_digest": h.hexdigest()}


# ----------------------------------------------------------------------------- toolchain discovery
def find_rustc(pin, env):
    """Locate the pinned rustc WITHOUT installing anything. Order: EXILE_RUST_SYSROOT, rustup run, PATH."""
    cands = []
    sysroot = env.get("EXILE_RUST_SYSROOT")
    if sysroot:
        cands.append(("EXILE_RUST_SYSROOT", os.path.join(sysroot, "bin", "rustc")))
    search_path = env.get("PATH")
    rustup = shutil.which("rustup", path=search_path)
    if rustup:
        rc, out = run([rustup, "which", "--toolchain", pin["rustc"], "rustc"], timeout=30)
        if rc == 0 and out.strip():
            cands.append(("rustup toolchain " + pin["rustc"], out.strip().splitlines()[-1]))
    p = shutil.which("rustc", path=search_path)
    if p:
        cands.append(("PATH", p))
    return cands


def rustc_info(path):
    rc, out = run([path, "-vV"], timeout=30)
    if rc != 0:
        return None
    d = {}
    for line in out.splitlines():
        if ": " in line:
            k, v = line.split(": ", 1)
            d[k.strip()] = v.strip()
    d["path"] = path
    d["verbose"] = out
    return d


def preflight(repo, env=None, with_zig=False):
    """Returns dict(status, checks[], toolchain{}, blockers[]). Never installs, never falls back."""
    env = dict(os.environ if env is None else env)
    checks, blockers = [], []

    def chk(cid, ok, detail, blocking=True):
        checks.append({"id": cid, "ok": bool(ok), "detail": detail, "blocking": blocking})
        if blocking and not ok:
            blockers.append({"check": cid, "detail": detail})

    pin = read_pin(repo)
    chk("pin.readable", pin and pin.get("rustc") and pin.get("rust_target"),
        f"{PIN_FILE}: rustc={pin and pin.get('rustc')} target={pin and pin.get('rust_target')}")
    tc = {"pin": pin}
    if not (pin and pin.get("rustc") and pin.get("rust_target")):
        return {"status": BLOCKED, "checks": checks, "toolchain": tc, "blockers": blockers}

    chosen, seen = None, []
    for origin, path in find_rustc(pin, env):
        info = rustc_info(path) if os.path.exists(path) else None
        seen.append(f"{origin}: {path} -> {info.get('release') if info else 'unusable'}")
        if info and info.get("release") == pin["rustc"] and not chosen:
            chosen = (origin, info)
    chk("rustc.pinned", chosen, f"required rustc {pin['rustc']}; candidates: " + ("; ".join(seen) or "none found"))
    if chosen:
        origin, info = chosen
        tc["rustc"] = {"origin": origin, "path": info["path"], "release": info.get("release"), "host": info.get("host"),
                       "commit_hash": info.get("commit-hash"), "llvm": info.get("LLVM version"), "verbose": info["verbose"]}
        rc, sysroot = run([info["path"], "--print", "sysroot"], timeout=30)
        sysroot = sysroot.strip()
        tc["sysroot"] = sysroot
        libdir = os.path.join(sysroot, "lib", "rustlib", pin["rust_target"], "lib")
        have = os.path.isdir(libdir) and any(f.startswith("libcore-") for f in os.listdir(libdir))
        chk("rust.target-installed", have,
            f"target {pin['rust_target']} {'present' if have else 'NOT installed'} in {sysroot} (core library {'found' if have else 'missing'})")
        tc["target"] = pin["rust_target"]
        tc["target_installed"] = bool(have)
    else:
        chk("rust.target-installed", False, f"cannot check target {pin['rust_target']} without rustc {pin['rustc']}")
        tc["target"] = pin["rust_target"]
        tc["target_installed"] = False

    for tool, args in (("gcc", ["--version"]), ("ld", ["--version"]), ("objcopy", ["--version"])):
        p = shutil.which(tool, path=env.get("PATH"))
        if not p:
            chk(f"tool.{tool}", False, f"{tool} not found")
            continue
        rc, out = run([p, *args], timeout=30)
        tc[tool] = {"path": p, "version": first_line(out)}
        chk(f"tool.{tool}", rc == 0, first_line(out))
    zig = shutil.which("zig", path=env.get("PATH")) or env.get("ZIG")
    zig_ver = None
    if zig:
        rc, out = run([zig, "version"], timeout=30)
        zig_ver = out.strip() if rc == 0 else None
    tc["zig"] = {"path": zig, "version": zig_ver, "pinned": pin.get("zig"),
                 "needed_for": "Batches 2-6 regression only; the kernel build does not use zig"}
    chk("zig.pinned", zig_ver == pin.get("zig"), f"zig {zig_ver or 'not found'}; pinned {pin.get('zig')}", blocking=with_zig)
    for rel in (os.path.join(B8, "scripts", "build.sh"), os.path.join(B8, "link", "linker.ld"), os.path.join(B8, "boot", "grub.cfg"),
                os.path.join(B7, "kernel", "lib.rs")):
        chk("source." + os.path.basename(rel), os.path.isfile(os.path.join(repo, rel)), rel)
    status = PASS if not blockers else BLOCKED
    return {"status": status, "checks": checks, "toolchain": tc, "blockers": blockers}


# ----------------------------------------------------------------------------- ELF inspection
def parse_elf(data):
    """Minimal ELF64 reader: header, sections (name, type, flags, addr, off, size). Raises ValueError."""
    if len(data) < 64 or data[:4] != b"\x7fELF":
        raise ValueError("not an ELF file")
    if data[4] != 2 or data[5] != 1:
        raise ValueError(f"not ELF64 little-endian (class={data[4]} data={data[5]})")
    e_type, e_machine = struct.unpack_from("<HH", data, 16)
    e_entry = struct.unpack_from("<Q", data, 24)[0]
    e_shoff = struct.unpack_from("<Q", data, 40)[0]
    e_shentsize, e_shnum, e_shstrndx = struct.unpack_from("<HHH", data, 58)
    secs = []
    if e_shoff and e_shentsize == 64 and e_shoff + e_shnum * 64 <= len(data):
        raw = []
        for i in range(e_shnum):
            name, typ, flags, addr, off, size = struct.unpack_from("<IIQQQQ", data, e_shoff + i * 64)
            raw.append((name, typ, flags, addr, off, size))
        stroff = raw[e_shstrndx][4] if e_shstrndx < len(raw) else 0

        def nm(o):
            end = data.find(b"\0", stroff + o)
            return data[stroff + o:end].decode("latin1")
        secs = [{"name": nm(r[0]), "type": r[1], "flags": r[2], "addr": r[3], "offset": r[4], "size": r[5]} for r in raw]
    return {"type": e_type, "machine": e_machine, "entry": e_entry, "sections": secs}


def inspect_elf(path, repo=REPO):
    """Structural validation. Returns (ok, [(name, ok, detail)], summary). Proves file properties only."""
    sys.path.insert(0, os.path.join(repo, "Meta", "Exile", "validation"))
    import exile_validate as ev  # shared Multiboot/ELF validator, not duplicated
    with open(path, "rb") as f:
        data = f.read()
    checks = list(ev.validate_elf_multiboot(data))
    summary = {"bytes": len(data), "sha256": hashlib.sha256(data).hexdigest()}
    try:
        e = parse_elf(data)
    except ValueError as ex:
        checks.append(("ELF section table readable", False, str(ex)))
        return False, checks, summary
    secs = {s["name"]: s for s in e["sections"]}
    summary.update({"entry": hex(e["entry"]), "machine": e["machine"], "elf_type": e["type"],
                    "sections": [{"name": s["name"], "addr": hex(s["addr"]), "size": s["size"], "flags": s["flags"]}
                                 for s in e["sections"] if s["name"] and not s["name"].startswith(".debug")]})
    checks.append(("architecture is x86-64 (EM_X86_64)", e["machine"] == 62, f"e_machine={e['machine']}"))
    checks.append(("type is EXEC, not DYN/REL", e["type"] == 2, f"e_type={e['type']}"))
    for name, (must, mustnot) in REQUIRED_SECTIONS.items():
        s = secs.get(name)
        ok = bool(s) and (s["flags"] & must) == must and not (s["flags"] & mustnot)
        checks.append((f"section {name} present with correct permissions", ok,
                       "missing" if not s else f"flags=0x{s['flags']:x} size={s['size']}"))
    text = secs.get(".text")
    in_text = bool(text) and text["addr"] <= e["entry"] < text["addr"] + text["size"]
    checks.append(("entry point lies inside .text", in_text, f"entry=0x{e['entry']:x}"))
    bad = [n for n in secs if n in (".interp", ".dynamic", ".dynsym", ".got.plt", ".tdata", ".tbss")]
    checks.append(("no dynamic-linking or TLS sections", not bad, ",".join(bad) or "none"))
    return all(c[1] for c in checks), checks, summary


def provenance(build_dir, pin):
    """Independent evidence of what actually compiled the code, from the produced rlibs/staticlib bytes
    (the kernel ELF discards .comment, so it cannot carry this itself)."""
    ev = {"rlib_rustc_versions": [], "rlib_has_pinned_target": None, "host_triples_found": [], "files": []}
    for fn in ("libomni_kernel7.rlib", "libomni8.rlib"):
        p = os.path.join(build_dir, fn)
        if not os.path.isfile(p):
            continue
        with open(p, "rb") as f:
            b = f.read()
        ev["files"].append({"file": fn, "sha256": hashlib.sha256(b).hexdigest()})
        for m in re.finditer(rb"rustc version (\d+\.\d+\.\d+)", b):
            v = m.group(1).decode()
            if v not in ev["rlib_rustc_versions"]:
                ev["rlib_rustc_versions"].append(v)
        has = pin["rust_target"].encode() in b
        ev["rlib_has_pinned_target"] = has if ev["rlib_has_pinned_target"] is None else (ev["rlib_has_pinned_target"] and has)
    p = os.path.join(build_dir, "libomni8_kernel.a")
    if os.path.isfile(p):
        with open(p, "rb") as f:
            b = f.read()
        ev["files"].append({"file": "libomni8_kernel.a", "sha256": hashlib.sha256(b).hexdigest()})
        ev["host_triples_found"] = [m.decode() for m in HOST_TRIPLE_MARKERS if m in b]
    return ev


def judge_provenance(info_text, prov, pin):
    """Rules deciding ACCEPTED vs REJECTED. Returns list of rejection reasons (empty = acceptable)."""
    why = []
    m = re.search(r"target=(\S+)", info_text or "")
    tgt = m.group(1) if m else None
    if tgt != pin["rust_target"]:
        why.append(f"build-info target is {tgt!r}, not the pinned {pin['rust_target']!r}")
    if "host-target fallback" in (info_text or "") or tgt == "host-fallback":
        why.append("build-info records a host-target fallback build")
    rm = re.search(r"^rustc: rustc (\S+)", info_text or "", re.M)
    if not rm or rm.group(1) != pin["rustc"]:
        why.append(f"build-info rustc is {rm.group(1) if rm else None!r}, not the pinned {pin['rustc']!r}")
    vs = prov.get("rlib_rustc_versions") or []
    if not vs:
        why.append("no rustc version string found inside the produced rlibs (cannot attribute the compiler)")
    elif vs != [pin["rustc"]]:
        why.append(f"rlibs were compiled by rustc {vs}, not the pinned {pin['rustc']}")
    if prov.get("rlib_has_pinned_target") is not True:
        why.append(f"produced rlibs do not carry the pinned target triple {pin['rust_target']}")
    if prov.get("host_triples_found"):
        why.append("host target triple(s) embedded in the kernel static library: " + ", ".join(prov["host_triples_found"]))
    return why


# ----------------------------------------------------------------------------- build
def build_env(repo, pre, source):
    env = dict(os.environ)
    rustc_dir = os.path.dirname(pre["toolchain"]["rustc"]["path"])
    env["PATH"] = rustc_dir + os.pathsep + env.get("PATH", "")
    env["SOURCE_DATE_EPOCH"] = source["commit_time"] or "0"
    env["TZ"], env["LC_ALL"] = "UTC", "C"
    for k in ("RUSTFLAGS", "CARGO_ENCODED_RUSTFLAGS", "RUSTC_WRAPPER", "RUSTC_BOOTSTRAP"):
        env.pop(k, None)  # nothing may silently change codegen or unlock unstable features
    return env


def build_variant(repo, pin, variant, out_dir, env, profile="release", test_exit=True):
    script = os.path.join(repo, B8, "scripts", "build.sh")
    shutil.rmtree(out_dir, ignore_errors=True)
    argv = ["bash", script, "--variant", variant, "--profile", profile, "--target", pin["rust_target"], "--out", out_dir]
    if test_exit:
        argv.insert(4, "--test-exit")
    t0 = time.time()
    rc, text = run(argv, env=env, timeout=1200)
    os.makedirs(out_dir, exist_ok=True)
    log = os.path.join(out_dir, "build.log")
    with open(log, "w", encoding="utf-8", errors="replace") as f:
        f.write("$ " + " ".join(shlex.quote(a) for a in argv) + f"\n# rc={rc}\n" + text)
    rec = {"variant": variant, "command": " ".join(shlex.quote(a) for a in argv), "rc": rc, "seconds": round(time.time() - t0, 1),
           "log": os.path.relpath(log, os.path.dirname(out_dir)), "status": FAIL, "reasons": [], "artifacts": []}
    elf = os.path.join(out_dir, "kernel.elf")
    info_p = os.path.join(out_dir, "build-info.txt")
    info = open(info_p, errors="replace").read() if os.path.isfile(info_p) else ""
    rec["build_info"] = info
    if rc != 0 or not os.path.isfile(elf):
        rec["reasons"].append(f"build.sh rc={rc}; kernel.elf {'present' if os.path.isfile(elf) else 'missing'}")
        return rec
    ok, checks, summary = inspect_elf(elf, repo)
    prov = provenance(out_dir, pin)
    reasons = judge_provenance(info, prov, pin)
    rec["compiler_warnings"] = len(re.findall(r"(?m)^warning", text))
    rec.update({"elf": summary, "elf_checks": [{"name": n, "ok": o, "detail": d} for n, o, d in checks], "provenance": prov})
    img = os.path.join(repo, B8, "scripts")
    for tool in ("check_image.py", "check_asm.py"):
        rc2, out2 = run([sys.executable, "-B", os.path.join(img, tool), elf], timeout=300)
        rec[tool] = {"rc": rc2}
        if rc2 != 0:
            reasons.append(f"{tool} failed (rc={rc2})")
    if not ok:
        reasons += ["ELF check failed: " + c[0] for c in checks if not c[1]]
    for fn in ("kernel.elf", "kernel32.elf", "build-info.txt", "build.log", "libomni8.rlib", "libomni_kernel7.rlib", "libomni8_kernel.a"):
        p = os.path.join(out_dir, fn)
        if os.path.isfile(p):
            rec["artifacts"].append({"path": os.path.relpath(p, os.path.dirname(out_dir)), "sha256": sha256_file(p), "bytes": os.path.getsize(p)})
    rec["reasons"] = reasons
    rec["status"] = PASS if not reasons else REJECTED
    return rec


def write_manifest(out, manifest):
    p = os.path.join(out, "build-manifest.json")
    with open(p, "w", encoding="utf-8") as f:
        json.dump(manifest, f, indent=2, sort_keys=True)
        f.write("\n")
    lines = []
    for root, _, files in os.walk(out):
        for fn in sorted(files):
            if fn == "SHA256SUMS":
                continue
            fp = os.path.join(root, fn)
            lines.append(f"{sha256_file(fp)}  {os.path.relpath(fp, out)}")
    with open(os.path.join(out, "SHA256SUMS"), "w", encoding="utf-8") as f:
        f.write("\n".join(sorted(lines, key=lambda l: l.split('  ', 1)[1])) + "\n")
    return p


def overall(manifest):
    vs = manifest["variants"]
    if manifest["preflight"]["status"] != PASS:
        return BLOCKED
    if any(v["status"] == FAIL for v in vs):
        return FAIL
    if any(v["status"] == REJECTED for v in vs):
        return REJECTED
    if len(vs) != len(manifest["requested_variants"]) or not vs:
        return FAIL
    return PASS


def cmd_build(args):
    repo = os.path.abspath(args.repo)
    out = os.path.abspath(args.out)
    os.makedirs(out, exist_ok=True)
    variants = args.variants.split(",") if args.variants else VARIANTS
    bad = [v for v in variants if v not in VARIANTS]
    if bad:
        print("unknown variant(s): " + ",".join(bad), file=sys.stderr)
        return 2
    pre = preflight(repo, with_zig=args.with_zig)
    source = source_identity(repo)
    manifest = {"schema_version": SCHEMA_VERSION, "tool": "exile_build.py", "tool_version": TOOL_VERSION, "created": now_iso(),
                "source": source, "requested_variants": variants, "preflight": {k: pre[k] for k in ("status", "checks", "blockers")},
                "toolchain": pre["toolchain"], "variants": [], "iso": [], "notes": []}
    manifest["config"] = {"profile": args.profile, "test_exit": True, "target": (pre["toolchain"].get("pin") or {}).get("rust_target"),
                          "build_script": B8 + "/scripts/build.sh", "host_fallback_allowed": False}
    if pre["status"] != PASS:
        manifest["status"] = BLOCKED
        manifest["accepted"] = False
        manifest["notes"].append("Preflight failed: nothing was built. There is no host-fallback; fix the blockers and re-run.")
        write_manifest(out, manifest)
        for b in pre["blockers"]:
            print(f"BLOCKED {b['check']}: {b['detail']}")
        print(f"\nOVERALL BLOCKED  manifest: {os.path.join(out, 'build-manifest.json')}")
        return 3
    pin = pre["toolchain"]["pin"]
    env = build_env(repo, pre, source)
    manifest["config"]["env"] = {k: env[k] for k in ("SOURCE_DATE_EPOCH", "TZ", "LC_ALL")}
    for v in variants:
        rec = build_variant(repo, pin, v, os.path.join(out, "variants", v), env, args.profile)
        manifest["variants"].append(rec)
        print(f"[{rec['status']:8}] {v}: " + ("; ".join(rec["reasons"][:2]) or f"kernel.elf {rec['elf']['bytes']} bytes sha256 {rec['elf']['sha256'][:16]}"))
    if args.repro and manifest["variants"] and manifest["variants"][0]["status"] == PASS:
        rec2 = build_variant(repo, pin, variants[0], os.path.join(out, "repro", variants[0]), env, args.profile)
        same = rec2.get("elf", {}).get("sha256") == manifest["variants"][0]["elf"]["sha256"]
        manifest["reproducibility"] = {"variant": variants[0], "first": manifest["variants"][0]["elf"]["sha256"],
                                       "second": rec2.get("elf", {}).get("sha256"), "identical": same}
        print(f"[{'PASS' if same else 'FAIL':8}] reproducible rebuild of {variants[0]}")
    manifest["status"] = overall(manifest)
    if args.repro and not manifest.get("reproducibility", {}).get("identical", False) and manifest["status"] == PASS:
        manifest["status"] = FAIL
    manifest["accepted"] = manifest["status"] == PASS
    write_manifest(out, manifest)
    print(f"\nOVERALL {manifest['status']}  manifest: {os.path.join(out, 'build-manifest.json')}")
    return {PASS: 0, BLOCKED: 3}.get(manifest["status"], 4)


# ----------------------------------------------------------------------------- ISO
def parse_xorriso_find(text):
    """Paths from `xorriso -indev X -find` output (lines like: '/boot/omni8.elf')."""
    return [l.strip().strip("'") for l in text.splitlines() if l.strip().startswith(("/", "'/"))]


def make_iso(repo, variant_dir, iso_path, tools):
    stage = iso_path + ".root"
    shutil.rmtree(stage, ignore_errors=True)
    os.makedirs(os.path.join(stage, "boot", "grub"))
    shutil.copy(os.path.join(variant_dir, "kernel.elf"), os.path.join(stage, "boot", "omni8.elf"))
    shutil.copy(os.path.join(repo, B8, "boot", "grub.cfg"), os.path.join(stage, "boot", "grub", "grub.cfg"))
    env = dict(os.environ, SOURCE_DATE_EPOCH=os.environ.get("SOURCE_DATE_EPOCH", "0"), LC_ALL="C", TZ="UTC")
    rc, out = run([tools["grub-mkrescue"], "-o", iso_path, stage], env=env, timeout=600)
    shutil.rmtree(stage, ignore_errors=True)
    return rc, out


def inspect_iso(iso_path, expect_elf_sha, tools, workdir):
    """Reads the ISO back with xorriso and checks content + boot catalog. Returns (ok, checks)."""
    checks = []
    rc, out = run([tools["xorriso"], "-indev", iso_path, "-find"], timeout=120)
    paths = parse_xorriso_find(out) if rc == 0 else []
    for need in ("/boot/omni8.elf", "/boot/grub/grub.cfg"):
        checks.append({"name": f"ISO contains {need}", "ok": need in paths, "detail": f"{len(paths)} paths listed (rc={rc})"})
    checks.append({"name": "ISO contains GRUB core image (i386-pc)", "ok": any(p.startswith("/boot/grub/i386-pc") for p in paths), "detail": ""})
    rc, out = run([tools["xorriso"], "-indev", iso_path, "-report_el_torito", "plain"], timeout=120)
    checks.append({"name": "El Torito BIOS boot entry present", "ok": rc == 0 and bool(re.search(r"(?i)\bBIOS\b", out)), "detail": first_line(out)})
    ext = os.path.join(workdir, "extracted-omni8.elf")
    if os.path.exists(ext):
        os.remove(ext)
    rc, out = run([tools["xorriso"], "-osirrox", "on", "-indev", iso_path, "-extract", "/boot/omni8.elf", ext], timeout=120)
    got = sha256_file(ext) if os.path.isfile(ext) else None
    checks.append({"name": "kernel inside the ISO is byte-identical to the built kernel.elf", "ok": got == expect_elf_sha,
                   "detail": f"iso={str(got)[:16]} built={expect_elf_sha[:16]}"})
    return all(c["ok"] for c in checks), checks


def cmd_iso(args, manifest=None):
    repo = os.path.abspath(args.repo)
    out = os.path.abspath(args.out)
    mp = os.path.join(out, "build-manifest.json")
    if manifest is None:
        if not os.path.isfile(mp):
            print("no build-manifest.json in --out; run `build` first", file=sys.stderr)
            return 2
        with open(mp) as f:
            manifest = json.load(f)
    tools = {t: shutil.which(t) for t in ("grub-mkrescue", "xorriso")}
    missing = [t for t, p in tools.items() if not p]
    manifest["iso"] = []
    if manifest.get("status") != PASS:
        manifest["notes"].append("ISO stage not run: the build did not pass acceptance (no ISO is made from a rejected or missing kernel).")
        write_manifest(out, manifest)
        print("ISO: BLOCKED — build not accepted")
        return 3
    if missing:
        manifest["notes"].append("ISO stage BLOCKED: missing " + ", ".join(missing) + " (also needs GRUB i386-pc modules).")
        manifest["iso"] = [{"variant": v["variant"], "status": BLOCKED, "reason": "missing " + ", ".join(missing)} for v in manifest["variants"]]
        write_manifest(out, manifest)
        print("ISO: BLOCKED — missing " + ", ".join(missing))
        return 3
    worst = PASS
    for v in manifest["variants"]:
        vdir = os.path.join(out, "variants", v["variant"])
        iso = os.path.join(out, "iso", v["variant"] + ".iso")
        os.makedirs(os.path.dirname(iso), exist_ok=True)
        rc, log = make_iso(repo, vdir, iso, tools)
        rec = {"variant": v["variant"], "status": FAIL, "path": os.path.relpath(iso, out), "grub_rc": rc}
        if rc == 0 and os.path.isfile(iso):
            ok, checks = inspect_iso(iso, v["elf"]["sha256"], tools, os.path.join(out, "iso"))
            rec.update({"sha256": sha256_file(iso), "bytes": os.path.getsize(iso), "checks": checks, "status": PASS if ok else FAIL})
        else:
            rec["reason"] = first_line(log)
        manifest["iso"].append(rec)
        print(f"[{rec['status']:8}] iso {v['variant']}")
        if rec["status"] != PASS:
            worst = FAIL
    ex = os.path.join(out, "iso", "extracted-omni8.elf")
    if os.path.exists(ex):
        os.remove(ex)
    manifest["notes"].append("ISO reproducibility is NOT verified; boot of the ISO is NOT part of this tool (see the validation orchestrator).")
    write_manifest(out, manifest)
    return 0 if worst == PASS else 4


# ----------------------------------------------------------------------------- inspect / verify
def cmd_inspect(args):
    repo = os.path.abspath(args.repo)
    p = args.path
    d = p if os.path.isdir(p) else os.path.dirname(os.path.abspath(p))
    elf = os.path.join(p, "kernel.elf") if os.path.isdir(p) else p
    ok, checks, summary = inspect_elf(elf, repo)
    for n, o, det in checks:
        print(("PASS " if o else "FAIL ") + n + (f" ({det})" if det else ""))
    pin = read_pin(repo)
    info_p = os.path.join(d, "build-info.txt")
    info = open(info_p, errors="replace").read() if os.path.isfile(info_p) else None
    if info is None:
        print("REJECTED: no build-info.txt next to the ELF, so its toolchain provenance is unknown")
        return 4
    why = judge_provenance(info, provenance(d, pin), pin)
    for w in why:
        print("REJECTED: " + w)
    verdict = PASS if ok and not why else (REJECTED if ok else FAIL)
    print(f"\nELF structure: {'valid' if ok else 'INVALID'}; acceptance: {'ACCEPTED' if verdict == PASS else verdict}")
    return 0 if verdict == PASS else 4


def verify_manifest(path, repo=REPO):
    """Re-applies the acceptance rules to a manifest and re-hashes artifacts. Returns (ok, [problems])."""
    problems = []
    with open(path) as f:
        m = json.load(f)
    base = os.path.dirname(os.path.abspath(path))
    if m.get("schema_version") != SCHEMA_VERSION:
        problems.append("unsupported schema_version")
    if m.get("status") != PASS or not m.get("accepted"):
        problems.append(f"manifest status is {m.get('status')}, not PASS")
    pin = (m.get("toolchain") or {}).get("pin") or read_pin(repo)
    if m.get("config", {}).get("host_fallback_allowed"):
        problems.append("manifest claims host fallback was allowed")
    got = {v["variant"] for v in m.get("variants", [])}
    if got != set(m.get("requested_variants", [])) or not got:
        problems.append("not every requested variant is present")
    for v in m.get("variants", []):
        if v.get("status") != PASS:
            problems.append(f"variant {v.get('variant')} status {v.get('status')}")
        for a in v.get("artifacts", []):
            fp = os.path.join(base, a["path"])
            if not os.path.isfile(fp):
                problems.append(f"missing artifact {a['path']}")
            elif sha256_file(fp) != a["sha256"]:
                problems.append(f"hash mismatch {a['path']}")
        if pin:
            vd = os.path.join(base, "variants", v["variant"])
            info = v.get("build_info", "")
            why = judge_provenance(info, provenance(vd, pin), pin) if os.path.isdir(vd) else ["variant directory missing"]
            problems += [f"{v.get('variant')}: {w}" for w in why]
    sums = os.path.join(base, "SHA256SUMS")
    if os.path.isfile(sums):
        with open(sums) as f:
            for line in f.read().splitlines():
                digest, rel = line.split("  ", 1)
                fp = os.path.join(base, rel)
                if rel != "build-manifest.json" and (not os.path.isfile(fp) or sha256_file(fp) != digest):
                    problems.append(f"SHA256SUMS mismatch {rel}")
    else:
        problems.append("SHA256SUMS missing")
    return not problems, problems


def cmd_verify(args):
    ok, problems = verify_manifest(args.manifest, os.path.abspath(args.repo))
    for p in problems:
        print("PROBLEM: " + p)
    print("manifest " + ("VERIFIED" if ok else "NOT VERIFIED"))
    return 0 if ok else 4


def cmd_preflight(args):
    pre = preflight(os.path.abspath(args.repo), with_zig=args.with_zig)
    for c in pre["checks"]:
        print(("PASS    " if c["ok"] else ("BLOCKED " if c["blocking"] else "info    ")) + f"{c['id']}: {c['detail']}")
    print(f"\nPREFLIGHT {pre['status']}")
    return 0 if pre["status"] == PASS else 3


def main(argv=None):
    ap = argparse.ArgumentParser(description="EXILE bare-metal build pipeline (no host fallback)", formatter_class=argparse.RawDescriptionHelpFormatter,
                                 epilog="Exit: 0 PASS, 3 BLOCKED, 4 FAIL/REJECTED, 2 usage. Pinned toolchain: " + PIN_FILE)
    ap.add_argument("--repo", default=REPO)
    sub = ap.add_subparsers(dest="cmd", required=True)
    for name in ("preflight", "build", "iso", "all"):
        s = sub.add_parser(name)
        s.add_argument("--with-zig", action="store_true", help="make the pinned zig a blocking requirement")
        if name != "preflight":
            s.add_argument("--out", required=True)
        if name in ("build", "all"):
            s.add_argument("--variants", help="comma-separated subset (default: all seven)")
            s.add_argument("--profile", choices=["release", "debug"], default="release")
            s.add_argument("--repro", action="store_true", help="rebuild the first variant and require identical SHA-256")
    s = sub.add_parser("inspect")
    s.add_argument("path")
    s = sub.add_parser("verify-manifest")
    s.add_argument("manifest")
    a = ap.parse_args(argv)
    if a.cmd == "preflight":
        return cmd_preflight(a)
    if a.cmd == "build":
        return cmd_build(a)
    if a.cmd == "iso":
        return cmd_iso(a)
    if a.cmd == "all":
        rc = cmd_build(a)
        return rc if rc != 0 else cmd_iso(a)
    if a.cmd == "inspect":
        return cmd_inspect(a)
    return cmd_verify(a)


if __name__ == "__main__":
    sys.exit(main())
