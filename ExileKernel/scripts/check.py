#!/usr/bin/env python3
import pathlib, struct, subprocess, sys
p=pathlib.Path(sys.argv[1]); b=p.read_bytes()
assert b[:6]==b'\x7fELF\x02\x01', 'Not little-endian ELF64'
assert struct.unpack_from('<HH',b,16)==(2,62), 'Not static x86-64 executable'
entry=struct.unpack_from('<Q',b,24)[0]
assert 0x100000 <= entry < 0x1000000
magic=struct.pack('<I',0x1badb002)
i=b[:8192].find(magic)
assert i>=0 and i%4==0, 'Multiboot header must be within first 8 KiB'
m,f,c=struct.unpack_from('<III',b,i); assert (m+f+c)&0xffffffff==0
headers=subprocess.check_output(['readelf','-lW',p],text=True)
assert 'INTERP' not in headers and 'DYNAMIC' not in headers
assert not subprocess.check_output(['nm','-u',p],text=True).strip(), 'Undefined symbols'
print('ELF64 / Multiboot / entry / static linkage checks PASS')
