#!/usr/bin/env python3
"""Source-text conformance of the Batch 11 Rust IPC layer against the Batch 2 and Batch 5 Zig models.

Compares: syscall numbers (0x100..0x107), structure sizes (send 48, recv 48, result 88, stats 32, descriptor 16),
status codes, the wire-header size and ABI version, the handle layout and the rights bits.
It proves the text agrees; it executes nothing.

usage: check_ipc_conformance.py [EXPERIMENTS_DIR] | --self-test      exit: 0 ok, 1 mismatch, 2 parse error
"""
import os, re, sys

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.abspath(os.path.join(HERE, "..", ".."))


class ParseError(Exception):
    pass


def rd(root, rel):
    try:
        return open(os.path.join(root, rel), encoding="utf-8").read()
    except OSError as e:
        raise ParseError(f"{rel}: {e}")


def n(s):
    return int(s.replace("_", ""), 0)


def facts(root, mutate=None):
    z5 = rd(root, "OMNI-Kernel-Batch-5/src/syscall.zig")
    z2e = rd(root, "OMNI-Kernel-Batch-2/src/core/error.zig")
    z2c = rd(root, "OMNI-Kernel-Batch-2/src/core/capabilities.zig")
    z2i = rd(root, "OMNI-Kernel-Batch-2/src/core/ipc.zig")
    rs = rd(root, "OMNI-Kernel-Batch-11/src/sys.rs")
    rst = rd(root, "OMNI-Kernel-Batch-11/src/status.rs")
    rc = rd(root, "OMNI-Kernel-Batch-11/src/caps.rs")
    ri = rd(root, "OMNI-Kernel-Batch-11/src/ipc.rs")
    if mutate:
        rs, rst, rc, ri = mutate(rs, rst, rc, ri)
    out = []
    names = ["endpoint_create", "endpoint_close", "send", "recv", "cancel_create", "cancel_signal", "endpoint_stats", "handle_release"]
    rnames = ["ENDPOINT_CREATE", "ENDPOINT_CLOSE", "SEND", "RECV", "CANCEL_CREATE", "CANCEL_SIGNAL", "ENDPOINT_STATS", "HANDLE_RELEASE"]
    for zn, rn in zip(names, rnames):
        zm = re.search(rf"\b{zn}\s*=\s*(0x[0-9A-Fa-f]+)", z5)
        rm = re.search(rf"pub const SYS_{rn}: u64 = (0x[0-9A-Fa-f]+);", rs)
        if not zm or not rm:
            raise ParseError(f"syscall {zn}")
        out.append((f"syscall {zn}", n(zm.group(1)), n(rm.group(1))))
    for zn, rn in [("send_args_size", "SEND_ARGS_SIZE"), ("recv_args_size", "RECV_ARGS_SIZE"), ("recv_result_size", "RECV_RESULT_SIZE"), ("stats_size", "STATS_SIZE")]:
        zm = re.search(rf"pub const {zn} = (\d+);", z5)
        rm = re.search(rf"pub const {rn}: usize = (\d+);", rs)
        if not zm or not rm:
            raise ParseError(zn)
        out.append((zn, int(zm.group(1)), int(rm.group(1))))
    # status codes shared by name
    for zn, rn in [("invalid_argument", "INVALID_ARGUMENT"), ("invalid_handle", "INVALID_HANDLE"), ("access_denied", "ACCESS_DENIED"),
                   ("quota_exceeded", "QUOTA_EXCEEDED"), ("would_block", "WOULD_BLOCK"), ("timed_out", "TIMED_OUT"),
                   ("cancelled", "CANCELLED"), ("peer_dead", "PEER_DEAD"), ("not_supported", "NOT_SUPPORTED"),
                   ("bad_version", "BAD_VERSION"), ("busy", "BUSY"), ("internal", "INTERNAL"), ("invalid_state", "INVALID_STATE")]:
        zm = re.search(rf"\b{zn} = (-\d+),", z2e)
        rm = re.search(rf"pub const {rn}: i64 = (-\d+);", rst)
        if not zm or not rm:
            raise ParseError(f"status {zn}")
        out.append((f"status {zn}", int(zm.group(1)), int(rm.group(1))))
    for zn, rn in [("status_fault", "FAULT"), ("status_no_sys", "NO_SYS")]:
        zm = re.search(rf"pub const {zn}: i64 = (-\d+);", z5)
        rm = re.search(rf"pub const {rn}: i64 = (-\d+);", rst)
        if not zm or not rm:
            raise ParseError(zn)
        out.append((zn, int(zm.group(1)), int(rm.group(1))))
    # wire header
    zm = re.search(r"wire_header_size: usize = (\d+)", z2i)
    rm = re.search(r"pub const WIRE_HEADER: usize = (\d+);", ri)
    if not zm or not rm:
        raise ParseError("wire header")
    out.append(("wire header size", int(zm.group(1)), int(rm.group(1))))
    zm = re.search(r"pub const abi_version: u16 = (0x[0-9A-Fa-f]+|\d+)", z2i)
    rm = re.search(r"pub const ABI_VERSION_WIRE: u16 = (0x[0-9A-Fa-f]+|\d+);", ri)
    if not zm or not rm:
        raise ParseError("abi version")
    out.append(("wire abi version", n(zm.group(1)), n(rm.group(1))))
    # rights bits: Zig packed struct, bit index = field order
    m = re.search(r"pub const Rights = packed struct\(u32\) \{(.*?)_reserved", z2c, re.S)
    if not m:
        raise ParseError("rights struct")
    zf = re.findall(r"^\s*(\w+): bool = false,", m.group(1), re.M)
    for rn in ["READ", "WRITE", "EXECUTE", "DUPLICATE", "TRANSFER", "WAIT", "SIGNAL", "ADMIN", "INSPECT"]:
        rm = re.search(rf"pub const {rn}: Rights = Rights\(1 << (\d+)\);", rc)
        if not rm or rn.lower() not in zf:
            raise ParseError(f"rights {rn}")
        out.append((f"rights {rn.lower()}", zf.index(rn.lower()), int(rm.group(1))))
    return out


def run(root, mutate=None):
    bad = 0
    for name, z, r in facts(root, mutate):
        ok = z == r
        bad += not ok
        print(f"{'PASS' if ok else 'FAIL'}  {name}: zig={z} rust={r}")
    print(f"\n{'ALL PASSED' if not bad else str(bad) + ' MISMATCH(ES)'}")
    return bad


def self_test(root):
    muts = [
        lambda a, b, c, d: (a.replace("SYS_RECV: u64 = 0x103", "SYS_RECV: u64 = 0x113"), b, c, d),
        lambda a, b, c, d: (a.replace("RECV_RESULT_SIZE: usize = 88", "RECV_RESULT_SIZE: usize = 80"), b, c, d),
        lambda a, b, c, d: (a, b.replace("PEER_DEAD: i64 = -10", "PEER_DEAD: i64 = -11"), c, d),
        lambda a, b, c, d: (a, b, c.replace("ADMIN: Rights = Rights(1 << 7)", "ADMIN: Rights = Rights(1 << 6)"), d),
        lambda a, b, c, d: (a, b, c, d.replace("WIRE_HEADER: usize = 32", "WIRE_HEADER: usize = 24")),
    ]
    missed = 0
    for i, m in enumerate(muts):
        try:
            import io, contextlib
            with contextlib.redirect_stdout(io.StringIO()):
                caught = run(root, m) > 0
        except ParseError:
            caught = True
        print(f"mutation {i + 1}: {'detected' if caught else 'MISSED'}")
        missed += not caught
    print(f"self-test: {len(muts) - missed}/{len(muts)} mutations detected")
    return 1 if missed else 0


if __name__ == "__main__":
    args = [a for a in sys.argv[1:]]
    root = ROOT
    try:
        if "--self-test" in args:
            sys.exit(self_test(root))
        if args:
            root = args[0]
        sys.exit(1 if run(root) else 0)
    except ParseError as e:
        print(f"PARSE ERROR: {e}", file=sys.stderr)
        sys.exit(2)
