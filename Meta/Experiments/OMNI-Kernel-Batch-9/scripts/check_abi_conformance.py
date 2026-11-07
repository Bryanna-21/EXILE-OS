#!/usr/bin/env python3
"""Cross-language ABI conformance check across Batches 2, 5, 6, 7 and 8.

The Zig batches (2 to 6) and the Rust batches (7, 8) share no code, so their agreement on
numbers cannot be checked by a compiler. This script parses the real source files of each
batch and compares the values that are meant to be shared: wire status codes, syscall
numbers, argument registers, the user-address ceiling and the page size.

It proves the *source text* agrees. It does not execute anything.

usage: check_abi_conformance.py [EXPERIMENTS_DIR]   (default: the directory above this package)
       check_abi_conformance.py --self-test         (proves the checker detects disagreement)
exit status: 0 all checks passed, 1 a check failed, 2 a source file could not be parsed.
"""
import copy
import os
import re
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
DEFAULT_ROOT = os.path.abspath(os.path.join(HERE, "..", ".."))


class ParseError(Exception):
    pass


def read(root, rel):
    try:
        with open(os.path.join(root, rel), encoding="utf-8") as f:
            return f.read()
    except OSError as e:
        raise ParseError(f"{rel}: {e}")


def num(s):
    return int(s.replace("_", ""), 0)


def extract(root):
    d = {}
    # Batch 2: Status enum
    t = read(root, "OMNI-Kernel-Batch-2/src/core/error.zig")
    m = re.search(r"pub const Status = enum\(i32\) \{(.*?)\n\s*_,?\s*\n?\s*\};|pub const Status = enum\(i32\) \{(.*?)\};", t, re.S)
    if not m:
        raise ParseError("Batch 2 Status enum not found")
    body = m.group(1) or m.group(2)
    d["b2_status"] = {n: int(v) for n, v in re.findall(r"^\s*([a-z_]+)\s*=\s*(-?\d+),", body, re.M)}
    if len(d["b2_status"]) < 10:
        raise ParseError("Batch 2 Status enum parsed too few entries")
    # Batch 5: syscall numbers and extra statuses
    t = read(root, "OMNI-Kernel-Batch-5/src/syscall.zig")
    m = re.search(r"pub const Nr = enum\(u32\) \{(.*?)\};", t, re.S)
    if not m:
        raise ParseError("Batch 5 Nr enum not found")
    d["b5_nr"] = {n: num(v) for n, v in re.findall(r"^\s*([a-z_]+)\s*=\s*(0x[0-9a-fA-F_]+|\d+),", m.group(1), re.M)}
    d["b5_fault"] = int(re.search(r"status_fault: i64 = (-?\d+);", t).group(1))
    d["b5_nosys"] = int(re.search(r"status_no_sys: i64 = (-?\d+);", t).group(1))
    # Batch 6: argument registers, user top, page size, syscall number width
    t = read(root, "OMNI-Kernel-Batch-6/src/arch.zig")
    m = re.search(r"\.x86_64 => \|r\| \{.*?\.omni_proposed => \.\{([^}]*)\}", t, re.S)
    if not m:
        raise ParseError("Batch 6 omni_proposed x86-64 register list not found")
    d["b6_args"] = re.findall(r"r\.(\w+)", m.group(1))
    m = re.search(r"\.kernel_existing => \.\{([^}]*)\}", t)
    d["b6_kernel_existing"] = re.findall(r"r\.(\w+)", m.group(1))
    d["b6_nr_reg"] = re.search(r"\.x86_64 => \|r\| \{\s*nr = r\.(\w+);", t).group(1)
    d["b6_nr_max"] = "maxInt(u32)" in t
    t = read(root, "OMNI-Kernel-Batch-6/src/user_memory.zig")
    d["b6_user_top"] = num(re.search(r"user_top_x86_64: u64 = (0x[0-9a-fA-F_]+);", t).group(1))
    d["b6_page"] = num(re.search(r"page_size: u64 = (\d+);", t).group(1))
    # Batch 7
    t = read(root, "OMNI-Kernel-Batch-7/kernel/status.rs")
    d["b7_status"] = {n.lower(): int(v) for n, v in re.findall(r"pub const ([A-Z_]+): i64 = (-?\d+);", t)}
    t = read(root, "OMNI-Kernel-Batch-7/kernel/syscall.rs")
    d["b7_sys"] = {n: num(v) for n, v in re.findall(r"pub const (SYS_[A-Z_]+): u64 = (0x[0-9a-fA-F_]+|\d+);", t)}
    m = re.search(r"pub fn args\(&self\) -> \[u64; 6\] \{\s*\[([^\]]*)\]", t)
    d["b7_args"] = re.findall(r"self\.(\w+)", m.group(1))
    d["b7_nr_max"] = "nr > u32::MAX as u64" in t
    d["b7_nr_reg"] = "let nr = f.rax;" in t
    t = read(root, "OMNI-Kernel-Batch-7/kernel/uaccess.rs")
    d["b7_user_top"] = num(re.search(r"pub const USER_TOP: u64 = (0x[0-9a-fA-F_]+);", t).group(1))
    d["b7_page"] = num(re.search(r"pub const PAGE: u64 = (\d+);", t).group(1))
    # Batch 8
    t = read(root, "OMNI-Kernel-Batch-8/src/paging.rs")
    d["b8_page"] = num(re.search(r"pub const PAGE_SIZE: u64 = (\d+);", t).group(1))
    return d


def checks(d):
    out = []

    def chk(name, ok, detail=""):
        out.append((name, bool(ok), detail))

    # status codes shared by Batch 2 and Batch 7
    zig_names = {"ok": 0, **d["b2_status"]}
    for name, val in sorted(d["b7_status"].items()):
        if name in ("fault", "no_sys"):
            continue
        chk(f"Batch 7 status {name}={val} equals Batch 2 status", zig_names.get(name) == val, f"batch2={zig_names.get(name)}")
    chk("Batch 7 FAULT equals Batch 5 status_fault", d["b7_status"].get("fault") == d["b5_fault"], f"{d['b7_status'].get('fault')} vs {d['b5_fault']}")
    chk("Batch 7 NO_SYS equals Batch 5 status_no_sys", d["b7_status"].get("no_sys") == d["b5_nosys"], f"{d['b7_status'].get('no_sys')} vs {d['b5_nosys']}")
    chk("Batch 5 extra statuses do not collide with Batch 2 statuses",
        d["b5_fault"] not in d["b2_status"].values() and d["b5_nosys"] not in d["b2_status"].values())
    # syscall numbers
    b5 = d["b5_nr"]
    b7 = d["b7_sys"]
    chk("Batch 5 syscall numbers are unique", len(set(b5.values())) == len(b5), str(b5))
    chk("Batch 7 syscall numbers are unique", len(set(b7.values())) == len(b7))
    chk("Batch 5 and Batch 7 syscall number ranges are disjoint", not (set(b5.values()) & set(b7.values())),
        f"b5={sorted(map(hex, b5.values()))} b7={sorted(map(hex, b7.values()))}")
    chk("every Batch 7 syscall number lies above every Batch 5 number (no interleaving)", min(b7.values()) > max(b5.values()))
    # calling convention
    chk("Batch 7 argument registers equal Batch 6 omni_proposed", d["b7_args"] == d["b6_args"], f"{d['b7_args']} vs {d['b6_args']}")
    chk("Batch 7 and Batch 6 both take the syscall number from rax", d["b6_nr_reg"] == "rax" and d["b7_nr_reg"])
    chk("Batch 7 and Batch 6 both reject numbers wider than 32 bits", d["b6_nr_max"] and d["b7_nr_max"])
    chk("Batch 6 models the existing Kernel/ convention as a different one (known divergence, undecided)",
        d["b6_kernel_existing"] != d["b6_args"], str(d["b6_kernel_existing"]))
    # address space
    chk("user-address ceiling: Batch 6 x86-64 equals Batch 7", d["b6_user_top"] == d["b7_user_top"], f"{d['b6_user_top']:#x}")
    chk("page size: Batch 6, 7 and 8 agree", d["b6_page"] == d["b7_page"] == d["b8_page"] == 4096)
    return out


def run(root):
    try:
        d = extract(root)
    except (ParseError, AttributeError) as e:
        print(f"PARSE-ERROR {e}")
        return 2
    res = checks(d)
    for n, ok, det in res:
        print(("PASS " if ok else "FAIL ") + n + (f"  [{det}]" if det and not ok else ""))
    bad = sum(1 for r in res if not r[1])
    print(f"check_abi_conformance: {len(res) - bad} passed, {bad} failed (source text only; nothing executed)")
    return 1 if bad else 0


def self_test(root):
    d = extract(root)
    base = checks(d)
    assert all(ok for _, ok, _ in base), "baseline must pass before mutation testing"
    mutations = {
        "status code changed": lambda x: x["b7_status"].__setitem__("busy", -99),
        "FAULT changed": lambda x: x["b7_status"].__setitem__("fault", -31),
        "syscall numbers collide": lambda x: x["b7_sys"].__setitem__("SYS_YIELD", 0x100),
        "argument order changed": lambda x: x["b7_args"].reverse(),
        "user top changed": lambda x: x.__setitem__("b7_user_top", 0x0001_0000_0000_0000),
        "page size changed": lambda x: x.__setitem__("b8_page", 8192),
        "nr width check removed": lambda x: x.__setitem__("b7_nr_max", False),
    }
    bad = 0
    for name, mutate in mutations.items():
        m = copy.deepcopy(d)
        mutate(m)
        detected = any(not ok for _, ok, _ in checks(m))
        print(("PASS " if detected else "FAIL ") + f"checker detects: {name}")
        bad += not detected
    print(f"self-test: {len(mutations) - bad} of {len(mutations)} mutations detected")
    return 1 if bad else 0


if __name__ == "__main__":
    a = sys.argv[1:]
    if a[:1] == ["--self-test"]:
        sys.exit(self_test(DEFAULT_ROOT))
    sys.exit(run(os.path.abspath(a[0]) if a else DEFAULT_ROOT))
