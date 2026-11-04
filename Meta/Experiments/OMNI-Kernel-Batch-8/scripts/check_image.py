#!/usr/bin/env python3
"""Static validation of a built Batch 8 kernel image (ELF64, plus optionally the ELF32 copy).

This proves properties of the file only. It does not boot anything.
Usage: check_image.py KERNEL_ELF [--elf32 KERNEL32_ELF]
Exit status: 0 all checks passed, 1 a check failed, 2 usage or unreadable input.
"""
import struct
import sys

MB_MAGIC = 0x1BADB002
MB_FLAGS = 3
PF_X, PF_W, PF_R = 1, 2, 4
SHF_WRITE, SHF_ALLOC, SHF_EXECINSTR = 1, 2, 4
PT_LOAD, PT_DYNAMIC, PT_INTERP, PT_TLS = 1, 2, 3, 7
REQUIRED = [
    "_start", "omni8_main", "omni8_trap_dispatch", "omni8_isr_table", "omni8_isr_user_test",
    "omni8_boot_stack_top", "omni8_probe_resume", "omni8_probe_sp", "omni8_enter_user",
    "__text_start", "__text_end", "__rodata_start", "__rodata_end", "__data_start",
    "__data_end", "__image_end",
]

results = []


def check(name, ok, detail=""):
    results.append((name, ok, detail))
    print(("PASS " if ok else "FAIL ") + name + (f" ({detail})" if detail else ""))


def read(path):
    with open(path, "rb") as f:
        return f.read()


def parse64(d):
    if d[:4] != b"\x7fELF":
        raise ValueError("not an ELF file")
    cls, enc = d[4], d[5]
    if cls != 2 or enc != 1:
        raise ValueError("not ELF64 little-endian")
    (etype, machine, _v, entry, phoff, shoff, _fl, _eh, phentsize, phnum, shentsize, shnum, shstrndx) = struct.unpack_from(
        "<HHIQQQIHHHHHH", d, 16)
    phs = []
    for i in range(phnum):
        t, fl, off, va, pa, fsz, msz, al = struct.unpack_from("<IIQQQQQQ", d, phoff + i * phentsize)
        phs.append(dict(type=t, flags=fl, off=off, va=va, pa=pa, fsz=fsz, msz=msz, align=al))
    secs = []
    for i in range(shnum):
        nm, ty, fl, addr, off, sz, link, info, al, es = struct.unpack_from("<IIQQQQIIQQ", d, shoff + i * shentsize)
        secs.append(dict(name_off=nm, type=ty, flags=fl, addr=addr, off=off, size=sz, link=link, info=info, entsize=es))
    strtab = secs[shstrndx]
    for s in secs:
        o = strtab["off"] + s["name_off"]
        s["name"] = d[o:d.index(b"\0", o)].decode()
    syms = []
    for s in secs:
        if s["type"] == 2:  # SHT_SYMTAB
            st = secs[s["link"]]
            for i in range(s["size"] // 24):
                nm, info, other, shndx, val, sz = struct.unpack_from("<IBBHQQ", d, s["off"] + i * 24)
                o = st["off"] + nm
                syms.append(dict(name=d[o:d.index(b"\0", o)].decode(), shndx=shndx, value=val, info=info))
    return dict(type=etype, machine=machine, entry=entry, phs=phs, secs=secs, syms=syms)


def main():
    args = sys.argv[1:]
    if not args:
        print(__doc__)
        return 2
    path, elf32 = args[0], None
    if "--elf32" in args:
        elf32 = args[args.index("--elf32") + 1]
    try:
        d = read(path)
        e = parse64(d)
    except (OSError, ValueError, struct.error) as ex:
        print(f"check_image: cannot read {path}: {ex}")
        return 2

    check("ELF64 little-endian x86-64 executable", e["type"] == 2 and e["machine"] == 62)
    loads = [p for p in e["phs"] if p["type"] == PT_LOAD]
    check("has PT_LOAD segments", len(loads) >= 3, f"{len(loads)} segments")
    check("no dynamic/interp/TLS segments", not any(p["type"] in (PT_DYNAMIC, PT_INTERP, PT_TLS) for p in e["phs"]))
    check("no segment is both writable and executable", not any(p["flags"] & PF_W and p["flags"] & PF_X for p in loads))
    check("exactly one executable segment", sum(1 for p in loads if p["flags"] & PF_X) == 1)
    check("all segments page-aligned in file and memory", all(p["va"] % 4096 == 0 and p["off"] % 4096 == 0 for p in loads))
    check("segments start at or above 1 MiB", all(p["va"] >= 0x100000 for p in loads))
    check("virtual == physical load addresses", all(p["va"] == p["pa"] for p in loads))
    end = max(p["va"] + p["msz"] for p in loads)
    check("image ends below 2 MiB", end <= 0x200000, f"end={end:#x}")
    ordered = sorted(loads, key=lambda p: p["va"])
    check("segments do not overlap", all(a["va"] + a["msz"] <= b["va"] for a, b in zip(ordered, ordered[1:])))
    ent = e["entry"]
    seg = next((p for p in loads if p["va"] <= ent < p["va"] + p["msz"]), None)
    check("entry point lies in the executable segment", seg is not None and bool(seg["flags"] & PF_X), f"entry={ent:#x}")
    check("entry point fits in 32 bits (Multiboot enters in protected mode)", ent < (1 << 32))

    # Multiboot v1 header
    found = None
    for off in range(0, min(len(d), 8192) - 11, 4):
        magic, flags, csum = struct.unpack_from("<III", d, off)
        if magic == MB_MAGIC:
            found = (off, flags, csum)
            break
    check("Multiboot v1 header within the first 8 KiB", found is not None)
    if found:
        off, flags, csum = found
        check("Multiboot checksum valid", (MB_MAGIC + flags + csum) & 0xFFFFFFFF == 0)
        check("Multiboot flags = 3 (page-align modules, memory info)", flags == MB_FLAGS, f"flags={flags:#x}")
        check("Multiboot header 4-byte aligned and inside a loaded segment",
              off % 4 == 0 and any(p["off"] <= off < p["off"] + p["fsz"] for p in loads))

    # sections
    alloc = [s for s in e["secs"] if s["flags"] & SHF_ALLOC and s["size"]]
    check("no allocated section is both writable and executable",
          not any(s["flags"] & SHF_WRITE and s["flags"] & SHF_EXECINSTR for s in alloc))
    names = {s["name"] for s in e["secs"]}
    check("no .interp/.dynamic/.plt sections", not (names & {".interp", ".dynamic", ".plt", ".dynsym"}))

    # symbols
    byname = {s["name"]: s for s in e["syms"] if s["name"]}
    undef = [s["name"] for s in e["syms"] if s["name"] and s["shndx"] == 0]
    check("no undefined symbols", not undef, ",".join(undef[:8]))
    missing = [n for n in REQUIRED if n not in byname]
    check("required symbols present", not missing, ",".join(missing))
    if not missing:
        v = lambda n: byname[n]["value"]
        check("section boundary symbols are page-aligned",
              all(v(n) % 4096 == 0 for n in ("__text_start", "__text_end", "__rodata_start", "__rodata_end", "__data_start", "__data_end", "__image_end")))
        check("sections are ordered text < rodata < data < image_end",
              v("__text_start") < v("__text_end") <= v("__rodata_start") < v("__rodata_end") <= v("__data_start") < v("__data_end") <= v("__image_end"))
        check("text starts at 1 MiB", v("__text_start") == 0x100000)
        check("_start is the ELF entry point", v("_start") == ent)
        check("boot stack top lies in the data/bss region", v("__data_start") < v("omni8_boot_stack_top") <= v("__data_end"))
        check("page-aligned data/bss segment starts at __data_start",
              any(p["va"] == v("__data_start") and p["flags"] & PF_W and not p["flags"] & PF_X for p in loads))
        check("text segment covers exactly [__text_start,__text_end)",
              any(p["va"] == v("__text_start") and p["flags"] & PF_X and p["va"] + p["msz"] == v("__text_end") for p in loads))
        check("rodata segment is read-only",
              any(p["va"] == v("__rodata_start") and p["flags"] == PF_R for p in loads))

    if elf32:
        try:
            d32 = read(elf32)
        except OSError as ex:
            check("ELF32 copy readable", False, str(ex))
        else:
            ok = d32[:4] == b"\x7fELF" and d32[4] == 1 and d32[5] == 1
            check("ELF32 copy is ELF32 little-endian", ok)
            if ok:
                etype, machine = struct.unpack_from("<HH", d32, 16)
                entry32 = struct.unpack_from("<I", d32, 24)[0]
                check("ELF32 copy is an i386 executable with the same entry", etype == 2 and machine == 3 and entry32 == ent, f"entry={entry32:#x}")
                found32 = any(struct.unpack_from("<I", d32, o)[0] == MB_MAGIC for o in range(0, 8192 - 3, 4))
                check("ELF32 copy has the Multiboot header in the first 8 KiB", found32)

    bad = [r for r in results if not r[1]]
    print(f"check_image: {len(results) - len(bad)} passed, {len(bad)} failed")
    return 1 if bad else 0


if __name__ == "__main__":
    sys.exit(main())
