#!/usr/bin/env python3
import argparse, os, pathlib, shutil, subprocess
r=pathlib.Path(__file__).resolve().parents[1]
p=argparse.ArgumentParser(); p.add_argument('--output',default=str(r/'build/boot')); a=p.parse_args()
base=pathlib.Path(a.output).resolve(); base.mkdir(parents=True,exist_ok=True)
qemu=os.environ.get('EUK_QEMU','qemu-system-x86_64')
grub=os.environ.get('EUK_GRUB','grub-mkrescue')
for fault,expected,code in [(None,'EUK BOOT_OK',33),('magic','EUK FAIL: boot magic',35),('cpu','EUK FAIL: CPU features',35),('map_missing','EUK FAIL: memory map missing',35),('map_truncated','EUK FAIL: memory map invalid',35),('handoff','EUK FAIL: handoff bounds',35),('map_length','EUK FAIL: memory map bounds',35),('table_dirty','EUK BOOT_OK',33),('traps','EUK: breakpoint returned',33),('irq','EUK: irq selftest ok',33),('trap_ud','EUK TRAP #UD',35),('trap_df','EUK TRAP #DF',35)]:
    case=base/(fault or 'normal'); case.mkdir(exist_ok=True)
    cmd=['python3',r/'scripts/build.py','--output',case,'--test-exit']
    if fault == 'traps':
        cmd += ['--traps']
    elif fault == 'irq':
        cmd += ['--irq']
    elif fault:
        cmd += ['--fault',fault]
    subprocess.run(cmd,check=True)
    stage=case/'iso-root'; (stage/'boot/grub').mkdir(parents=True,exist_ok=True)
    shutil.copyfile(case/'euk.elf',stage/'boot/euk.elf')
    (stage/'boot/grub/grub.cfg').write_text('set timeout=0\nset default=0\nmenuentry "EUK isolated test" {\n multiboot /boot/euk.elf\n boot\n}\n')
    subprocess.run([grub,'-o',case/'euk.iso',stage],check=True,stdout=subprocess.DEVNULL)
    for machine in (['pc','q35'] if fault in (None,'traps','irq') else ['q35']):
        cmd=[qemu,'-machine',machine,'-accel','tcg','-cpu','qemu64','-m','128M','-cdrom',str(case/'euk.iso'),'-boot','d','-display','none','-serial','stdio','-monitor','none','-no-reboot','-device','isa-debug-exit,iobase=0xf4,iosize=0x04']
        print('+',' '.join(cmd),flush=True)
        result=subprocess.run(cmd,capture_output=True,text=True,timeout=25)
        (case/f'{machine}.log').write_text(result.stdout+result.stderr)
        assert result.returncode==code, (fault,machine,result.returncode,result.stdout,result.stderr)
        assert expected in result.stdout, (fault,result.stdout)
        if code == 35: assert 'EUK BOOT_OK' not in result.stdout
        if fault == 'irq':
            for line in ('EUK: PIC remapped to 0x20','EUK: MADT','EUK: unhandled irq contained','EUK: PIC timer ok','EUK: bounded memory map parsed','EUK BOOT_OK'):
                assert line in result.stdout, (fault, machine, line, result.stdout)
            assert 'EUK TRAP' not in result.stdout, result.stdout
        if fault in ('traps','irq','trap_ud','trap_df'):
            assert 'EUK: GDT/TSS/IDT loaded' in result.stdout, result.stdout
            assert 'nested trap' not in result.stdout, result.stdout
        print(f'PASS {fault or "normal"}/{machine}: exit={result.returncode}; {result.stdout.strip()}',flush=True)

# Mission D3: boot the real scheduler on bare metal. The serial log must contain
# these lines in this order; the demo exits through isa-debug-exit (33 = success).
D3_LINES = [
    'EUK D3: idle and two workers created',
    'EUK D3 T1 start',
    'EUK D3 T2 start',
    'EUK D3 T1 resumed after yield',
    'EUK D3 T2 resumed after yield',
    'EUK D3 T2 woke T1',
    'EUK D3 exit-path: self-destroy refused (still on cpu)',
    'EUK D3 T1 woken from block',
    'EUK D3 T1 sleeping 3 ticks',
    'EUK D3 idle reaped id=3 status=5',
    'EUK D3 T1 awake after sleep',
    'EUK D3 exit-path: self-destroy refused (still on cpu)',
    'EUK D3 idle reaped id=2 status=0',
    'EUK D3 stacks_in_use=1 live=1 created=3 destroyed=2',
    'EUK D3 context_switches=8',
    'EUK D3 DEMO_OK',
]
case=base/'d3_demo'; case.mkdir(exist_ok=True)
subprocess.run(['python3',r/'scripts/build.py','--output',case,'--test-exit','--demo'],check=True)
stage=case/'iso-root'; (stage/'boot/grub').mkdir(parents=True,exist_ok=True)
shutil.copyfile(case/'euk.elf',stage/'boot/euk.elf')
(stage/'boot/grub/grub.cfg').write_text('set timeout=0\nset default=0\nmenuentry "EUK D3 demo" {\n multiboot /boot/euk.elf\n boot\n}\n')
subprocess.run([grub,'-o',case/'euk.iso',stage],check=True,stdout=subprocess.DEVNULL)
for machine in ['pc','q35']:
    cmd=[qemu,'-machine',machine,'-accel','tcg','-cpu','qemu64','-m','128M','-cdrom',str(case/'euk.iso'),'-boot','d','-display','none','-serial','stdio','-monitor','none','-no-reboot','-device','isa-debug-exit,iobase=0xf4,iosize=0x04']
    print('+',' '.join(cmd),flush=True)
    result=subprocess.run(cmd,capture_output=True,text=True,timeout=40)
    (case/f'{machine}.log').write_text(result.stdout+result.stderr)
    assert result.returncode==33, ('d3_demo',machine,result.returncode,result.stdout,result.stderr)
    pos=0
    for line in D3_LINES:
        found=result.stdout.find(line,pos)
        assert found>=0, ('d3_demo missing or out of order',machine,line,result.stdout)
        pos=found+len(line)
    assert 'EUK FAIL' not in result.stdout, result.stdout
    print(f'PASS d3_demo/{machine}: exit={result.returncode}; {len(D3_LINES)} ordered serial checkpoints',flush=True)
