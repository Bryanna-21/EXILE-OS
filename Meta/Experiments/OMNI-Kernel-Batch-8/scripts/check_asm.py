#!/usr/bin/env python3
"""Static checks on the assembly and on the instructions in a built image.

Proves properties of the encoded instructions; it does not execute them.
Usage: check_asm.py KERNEL_ELF
Exit status: 0 all passed, 1 a check failed, 2 usage or tool failure.
"""
import os
import re
import subprocess
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
results = []


def check(name, ok, detail=""):
    results.append(ok)
    print(("PASS " if ok else "FAIL ") + name + (f" ({detail})" if detail else ""))


def objdump(path, *flags):
    return subprocess.run(["objdump", *flags, path], capture_output=True, text=True, check=True).stdout


def function_body(listing, name):
    m = re.search(r"^[0-9a-f]+ <" + re.escape(name) + r">:\n((?:[ \t]+[0-9a-f]+:.*\n|\n)*)", listing, re.M)
    return m.group(1) if m else None


def main():
    if len(sys.argv) != 2:
        print(__doc__)
        return 2
    path = sys.argv[1]
    try:
        gen = subprocess.run([sys.executable, os.path.join(HERE, "gen_isr.py"), "--check"], capture_output=True, text=True)
        check("isr.S matches scripts/gen_isr.py", gen.returncode == 0, gen.stdout.strip() or gen.stderr.strip())
        text = objdump(path, "-d", "-M", "att", "--no-show-raw-insn")
    except (OSError, subprocess.CalledProcessError) as ex:
        print(f"check_asm: tool failure: {ex}")
        return 2

    common = function_body(text, "isr_common")
    check("isr_common present", common is not None)
    if common:
        for ins in ("fxsave", "fxrstor", "iretq", "cld", "call"):
            check(f"isr_common uses {ins}", re.search(r"\b" + ins + r"\b", common) is not None)
        pushes = len(re.findall(r"\bpush\s+%r(?:ax|bx|cx|dx|si|di|bp|8|9|1[0-5])\b", common))
        pops = len(re.findall(r"\bpop\s+%r(?:ax|bx|cx|dx|si|di|bp|8|9|1[0-5])\b", common))
        check("isr_common saves and restores the same 15 registers", pushes == 15 and pops == 15, f"push={pushes} pop={pops}")
        check("isr_common aligns the stack for fxsave (and $-16)", re.search(r"and\s+\$0xfffffffffffffff0,%rsp", common) is not None)
    ents = re.findall(r"^[0-9a-f]+ <omni8_isr_(\d+)>:", text, re.M)
    check("48 exception/IRQ stubs present", sorted(map(int, ents)) == list(range(48)), f"{len(ents)} stubs")
    check("user-test (0x81) stub present", "<omni8_isr_user_test>:" in text)

    # No instruction may use state the kernel does not enable.
    check("no AVX/AVX-512 registers", re.search(r"%[yz]mm\d+", text) is None)
    check("no x87/MMX state in kernel code", re.search(r"\b(fld|fst|fild|fistp|emms)\w*\b", text) is None)
    # Red zone: nothing may be addressed below rsp.
    rz = re.findall(r"^\s+[0-9a-f]+:\s+.*-0x[0-9a-f]+\(%rsp\)", text, re.M)
    check("no red-zone addressing (negative %rsp offsets)", not rz, f"{len(rz)} hits")
    check("SYSCALL entry stub (Batch 7) uses swapgs and sysretq",
          "swapgs" in text and "sysretq" in text)
    check("context switch routine present", "<omni_ctx_switch>:" in text or re.search(r"<omni_[a-z_]*switch[a-z_]*>:", text) is not None)

    nop = function_body(text, "omni8_probe_nop")
    check("probe nop routine is a bare ret", nop is not None and nop.strip().endswith("ret") and len([l for l in nop.splitlines() if l.strip()]) == 1)
    for p, ins in (("read", "mov"), ("write", "mov"), ("exec", "call"), ("ud", "ud2"), ("bp", "int3"), ("de", "div")):
        b = function_body(text, f"omni8_probe_{p}")
        check(f"probe {p} contains its trapping instruction", b is not None and re.search(r"\b" + ins, b) is not None)
    eu = function_body(text, "omni8_enter_user")
    check("omni8_enter_user is mov %rdi,%rsp; iretq", eu is not None and "iretq" in eu and re.search(r"mov\s+%rdi,%rsp", eu) is not None)

    # The 32-bit bootstrap must not contain 64-bit-only encodings before long mode: checked by
    # the assembler (.code32), so here we only confirm the long-mode switch sequence exists.
    boot = objdump(path, "-d", "-M", "i386", "--no-show-raw-insn", "--start-address=0x10000c", "--stop-address=0x100300")
    order = [boot.find(k) for k in ("lgdt", "mov    %eax,%cr4", "mov    %eax,%cr3", "wrmsr", "mov    %eax,%cr0", "ljmp   $0x8,")]
    check("boot code: lgdt, CR4, CR3, EFER, CR0 (paging), then far jump to selector 0x8, in that order",
          all(o >= 0 for o in order) and order == sorted(order), str(order))

    bad = results.count(False)
    print(f"check_asm: {len(results) - bad} passed, {bad} failed")
    return 1 if bad else 0


if __name__ == "__main__":
    sys.exit(main())
