#!/usr/bin/env python3
"""EXILE virtual-disk GPT builder and validator (WO6).

SAFETY: operates ONLY on regular files. Block devices, character devices, directories, symlinks
to them, and paths under /dev are refused. There is no flag to override this. The tool never
touches host boot configuration.

SCOPE: writes a protective MBR + primary/backup GPT with the EXILE layout. It does NOT format
filesystems, install a bootloader, or copy a kernel. A disk produced here is a partition-table
skeleton, not an installed system. See Documentation/Architecture/Platform/BOOT_AND_INSTALL.md.
"""
import argparse, json, os, stat, struct, sys, uuid, zlib

SECTOR = 512
GPT_SIG = b"EFI PART"
ESP_GUID = uuid.UUID("C12A7328-F81F-11D2-BA4B-00A0C93EC93B")
LINUX_FS = uuid.UUID("0FC63DAF-8483-4772-8E79-3D69D8477DE4")
# Layout: name, type guid, size in MiB (None = rest of disk)
LAYOUT = [
    ("EXILE-ESP", ESP_GUID, 64),
    ("EXILE-ROOT-A", LINUX_FS, 256),
    ("EXILE-ROOT-B", LINUX_FS, 256),
    ("EXILE-STATE", LINUX_FS, None),
]
ALIGN = 2048  # 1 MiB in sectors
MIN_DISK = 1 + 1 + 32 + (64 + 256 + 256 + 16) * 2048 + 33 + ALIGN * 2


class DiskError(Exception):
    pass


def crc(b):
    return zlib.crc32(b) & 0xFFFFFFFF


def check_target(path, create_size=None):
    """Refuse anything that is not a regular file (or a not-yet-existing path in a real dir)."""
    rp = os.path.realpath(path)
    if rp.startswith("/dev/") or rp == "/dev":
        raise DiskError(f"refusing device path: {rp}")
    if os.path.lexists(path):
        st = os.stat(path)
        if not stat.S_ISREG(st.st_mode):
            raise DiskError(f"refusing non-regular file (mode {oct(st.st_mode)}): {path}")
    elif create_size is None:
        raise DiskError(f"no such file: {path}")
    return rp


def plan(total_sectors):
    if total_sectors * SECTOR < MIN_DISK * SECTOR:
        raise DiskError("disk too small for EXILE layout")
    first = ALIGN
    last_usable = total_sectors - 34
    parts, cur = [], first
    for name, tg, mib in LAYOUT:
        if mib is None:
            end = ((last_usable + 1) // ALIGN) * ALIGN - 1
        else:
            end = cur + mib * 2048 - 1
        if end > last_usable or end < cur:
            raise DiskError("layout does not fit")
        parts.append(dict(name=name, type=tg, first=cur, last=end))
        cur = end + 1
        cur = ((cur + ALIGN - 1) // ALIGN) * ALIGN
    return parts, first, last_usable


def entry_bytes(p, guid):
    name = p["name"].encode("utf-16-le").ljust(72, b"\0")
    return p["type"].bytes_le + guid.bytes_le + struct.pack("<QQQ", p["first"], p["last"], 0) + name


def header(total, cur, other, first, last, disk_guid, entries_lba, ecrc):
    h = bytearray(92)
    h[0:8] = GPT_SIG
    struct.pack_into("<IIIIQQQQ", h, 8, 0x00010000, 92, 0, 0, cur, other, first, last)
    h[56:72] = disk_guid.bytes_le
    struct.pack_into("<QIII", h, 72, entries_lba, 128, 128, ecrc)
    struct.pack_into("<I", h, 16, crc(bytes(h)))
    return bytes(h).ljust(SECTOR, b"\0")


def build(path, size_mib, seed=None):
    if size_mib * 2048 < MIN_DISK:
        raise DiskError("size too small")
    check_target(path, create_size=size_mib)
    total = size_mib * 2048
    parts, first, last = plan(total)
    rnd = uuid.UUID(int=int.from_bytes(__import__("hashlib").sha256(seed.encode()).digest()[:16], "big"), version=4) if seed else uuid.uuid4()
    disk_guid = rnd
    guids = [uuid.UUID(int=(disk_guid.int + i + 1) & ((1 << 128) - 1), version=4) for i in range(len(parts))]
    ents = bytearray(128 * 128)
    for i, p in enumerate(parts):
        ents[i * 128:(i + 1) * 128] = entry_bytes(p, guids[i])
    ecrc = crc(bytes(ents))
    mbr = bytearray(SECTOR)
    mbr[446:462] = bytes([0, 0, 2, 0, 0xEE, 0xFF, 0xFF, 0xFF]) + struct.pack("<II", 1, min(total - 1, 0xFFFFFFFF))
    mbr[510:512] = b"\x55\xAA"
    with open(path, "wb") as f:  # regular file only (checked above)
        f.truncate(total * SECTOR)
        f.write(mbr)
        f.write(header(total, 1, total - 1, first, last, disk_guid, 2, ecrc).ljust(SECTOR, b"\0"))
        f.write(bytes(ents))
        f.seek((total - 33) * SECTOR)
        f.write(bytes(ents))
        f.write(header(total, total - 1, 1, first, last, disk_guid, total - 33, ecrc))
    return [dict(name=p["name"], first=p["first"], last=p["last"], sectors=p["last"] - p["first"] + 1) for p in parts]


def validate(path):
    """Return list of problems (empty = valid). Independent of build(): parses raw bytes."""
    check_target(path)
    probs = []
    with open(path, "rb") as f:
        data_len = os.fstat(f.fileno()).st_size
        total = data_len // SECTOR
        f.seek(0)
        mbr = f.read(SECTOR)
        if mbr[510:512] != b"\x55\xAA":
            probs.append("MBR signature missing")
        if mbr[450] != 0xEE:
            probs.append("protective MBR partition type is not 0xEE")

        def read_hdr(lba):
            f.seek(lba * SECTOR)
            h = f.read(92)
            if len(h) < 92 or h[:8] != GPT_SIG:
                return None, "bad signature"
            c = struct.unpack_from("<I", h, 16)[0]
            hh = bytearray(h)
            hh[16:20] = b"\0\0\0\0"
            if crc(bytes(hh)) != c:
                return None, "header CRC mismatch"
            cur, other, fu, lu = struct.unpack_from("<QQQQ", h, 24)
            el, n, esz, ecrc = struct.unpack_from("<QIII", h, 72)
            return dict(cur=cur, other=other, first=fu, last=lu, el=el, n=n, esz=esz, ecrc=ecrc, guid=h[56:72]), None

        res = {}
        for name, lba in (("primary", 1), ("backup", total - 1)):
            h, e = read_hdr(lba)
            if e:
                probs.append(f"{name} GPT: {e}")
                continue
            if h["cur"] != lba:
                probs.append(f"{name} GPT: current LBA {h['cur']} != {lba}")
            if h["esz"] != 128 or h["n"] > 128 or h["n"] == 0:
                probs.append(f"{name} GPT: bad entry geometry")
                continue
            f.seek(h["el"] * SECTOR)
            raw = f.read(h["n"] * h["esz"])
            if len(raw) != h["n"] * h["esz"] or crc(raw) != h["ecrc"]:
                probs.append(f"{name} GPT: entry array CRC mismatch")
                continue
            res[name] = (h, raw)
        if "primary" in res and "backup" in res and res["primary"][1] != res["backup"][1]:
            probs.append("primary and backup entry arrays differ")
        if "primary" in res:
            h, raw = res["primary"]
            spans = []
            for i in range(h["n"]):
                e = raw[i * 128:(i + 1) * 128]
                if e[:16] == b"\0" * 16:
                    continue
                a, b = struct.unpack_from("<QQ", e, 32)
                if a < h["first"] or b > h["last"] or b < a:
                    probs.append(f"entry {i}: outside usable range or inverted")
                if a % ALIGN:
                    probs.append(f"entry {i}: not 1 MiB aligned")
                spans.append((a, b, i))
            spans.sort()
            for x, y in zip(spans, spans[1:]):
                if y[0] <= x[1]:
                    probs.append(f"entries {x[2]} and {y[2]} overlap")
    return probs


def main(argv=None):
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    sub = ap.add_subparsers(dest="cmd", required=True)
    b = sub.add_parser("build")
    b.add_argument("image")
    b.add_argument("--size-mib", type=int, default=1024)
    b.add_argument("--seed", help="deterministic GUIDs (reproducible images)")
    v = sub.add_parser("validate")
    v.add_argument("image")
    a = ap.parse_args(argv)
    try:
        if a.cmd == "build":
            print(json.dumps(build(a.image, a.size_mib, a.seed), indent=1))
            return 0
        probs = validate(a.image)
        print(json.dumps({"valid": not probs, "problems": probs}, indent=1))
        return 0 if not probs else 4
    except DiskError as e:
        print(f"REFUSED: {e}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    sys.exit(main())
