#!/usr/bin/env python3
import pathlib
import struct
import subprocess
import sys

path = pathlib.Path(sys.argv[1])
binary = path.read_bytes()

assert binary[:6] == b'\x7fELF\x02\x01', 'Not little-endian ELF64'
assert struct.unpack_from('<HH', binary, 16) == (2, 62), 'Not static x86-64 executable'

entry = struct.unpack_from('<Q', binary, 24)[0]
assert 0x100000 <= entry < 0x1000000

magic = struct.pack('<I', 0x1badb002)
offset = binary[:8192].find(magic)
assert offset >= 0 and offset % 4 == 0, 'Multiboot header must be within first 8 KiB'

magic_value, flags, checksum = struct.unpack_from('<III', binary, offset)
assert (magic_value + flags + checksum) & 0xffffffff == 0

headers = subprocess.check_output(['readelf', '-lW', path], text=True)
assert 'INTERP' not in headers and 'DYNAMIC' not in headers

undefined_symbols = subprocess.check_output(['nm', '-u', path], text=True)
assert not undefined_symbols.strip(), 'Undefined symbols'

print('ELF64 / Multiboot / entry / static linkage checks PASS')
