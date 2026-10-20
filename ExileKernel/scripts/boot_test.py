#!/usr/bin/env python3
import argparse, os, pathlib, shutil, subprocess
r=pathlib.Path(__file__).resolve().parents[1]
p=argparse.ArgumentParser(); p.add_argument('--output',default=str(r/'build/boot')); a=p.parse_args()
base=pathlib.Path(a.output).resolve(); base.mkdir(parents=True,exist_ok=True)
qemu=os.environ.get('EUK_QEMU','qemu-system-x86_64')
grub=os.environ.get('EUK_GRUB','grub-mkrescue')
for fault,expected,code in [(None,'EUK BOOT_OK',33),('magic','EUK FAIL: boot magic',35),('cpu','EUK FAIL: CPU features',35),('map_missing','EUK FAIL: memory map missing',35),('map_truncated','EUK FAIL: memory map invalid',35),('handoff','EUK FAIL: handoff bounds',35),('map_length','EUK FAIL: memory map bounds',35),('table_dirty','EUK BOOT_OK',33)]:
    case=base/(fault or 'normal'); case.mkdir(exist_ok=True)
    cmd=['python3',r/'scripts/build.py','--output',case,'--test-exit']
    if fault: cmd += ['--fault',fault]
    subprocess.run(cmd,check=True)
    stage=case/'iso-root'; (stage/'boot/grub').mkdir(parents=True,exist_ok=True)
    shutil.copyfile(case/'euk.elf',stage/'boot/euk.elf')
    (stage/'boot/grub/grub.cfg').write_text('set timeout=0\nset default=0\nmenuentry "EUK isolated test" {\n multiboot /boot/euk.elf\n boot\n}\n')
    subprocess.run([grub,'-o',case/'euk.iso',stage],check=True,stdout=subprocess.DEVNULL)
    for machine in (['pc','q35'] if fault is None else ['q35']):
        cmd=[qemu,'-machine',machine,'-accel','tcg','-cpu','qemu64','-m','128M','-cdrom',str(case/'euk.iso'),'-boot','d','-display','none','-serial','stdio','-monitor','none','-no-reboot','-device','isa-debug-exit,iobase=0xf4,iosize=0x04']
        print('+',' '.join(cmd),flush=True)
        result=subprocess.run(cmd,capture_output=True,text=True,timeout=25)
        (case/f'{machine}.log').write_text(result.stdout+result.stderr)
        assert result.returncode==code, (fault,machine,result.returncode,result.stdout,result.stderr)
        assert expected in result.stdout, (fault,result.stdout)
        if code == 35: assert 'EUK BOOT_OK' not in result.stdout
        print(f'PASS {fault or "normal"}/{machine}: exit={result.returncode}; {result.stdout.strip()}',flush=True)
