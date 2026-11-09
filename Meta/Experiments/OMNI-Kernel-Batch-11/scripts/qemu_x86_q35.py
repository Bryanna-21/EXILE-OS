#!/usr/bin/env python3
"""Run the Batch 8 QEMU boot cases on the `q35` machine instead of `pc`.

Reuses Batch 8's runner unchanged except for the machine type. NOTHING IS REPORTED AS PASSED UNLESS QEMU BOOTED THE IMAGE;
without qemu-system-x86_64 and grub-mkrescue every case is "NOT RUN" (exit 3), as in the Batch 8 runner.
usage: qemu_x86_q35.py [--cases base,ring3] [--out DIR] [--timeout S]
"""
import importlib.util, os, sys

HERE = os.path.dirname(os.path.abspath(__file__))
SRC = os.path.join(HERE, "..", "..", "OMNI-Kernel-Batch-8", "scripts", "qemu_test.py")
spec = importlib.util.spec_from_file_location("b8_qemu_test", SRC)
m = importlib.util.module_from_spec(spec)
spec.loader.exec_module(m)
i = m.QEMU_ARGS.index("-machine")
assert m.QEMU_ARGS[i + 1] == "pc", "Batch 8 runner changed; update this wrapper"
m.QEMU_ARGS[i + 1] = "q35"
sys.exit(m.main())
