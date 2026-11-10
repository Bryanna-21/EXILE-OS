#!/usr/bin/env python3
"""Static consistency checks between application launchers, build targets and default configuration.

Checks (all source-text, nothing is built or executed):
  1. every Base/res/apps/*.af has an Executable=/bin/<X> for which a build target <X> exists in Userland/;
  2. every Icon= named in an .af exists in Base/res/icons;
  3. every "<name>.af" literal in the Taskbar quick-launch defaults exists in Base/res/apps;
  4. every /bin/<X> named in Base/etc/LaunchServer.ini and SystemServerUser.ini has a build target <X> (or is a known utility).

Exit status: 0 all good, 1 a violation was found, 2 the repository layout could not be read.
usage: lint-app-launchers.py [--self-test]
"""
import glob
import os
import re
import sys

ROOT = os.path.abspath(os.path.join(os.path.dirname(os.path.abspath(__file__)), '..'))
TARGET_CALL = re.compile(r'(?:serenity_app|exile_app|serenity_bin|exile_bin|serenity_component|exile_component|add_executable)\s*\(\s*([A-Za-z0-9_]+)')
TARGETS_LIST = re.compile(r'TARGETS\s+([A-Za-z0-9_ ]+)')
# Launched by path but not built from Userland/ target names that this script can see (installed by other means).
KNOWN_EXTERNAL = {'PackageManager'}


def read(path):
    with open(path, encoding='utf-8', errors='replace') as f:
        return f.read()


def build_targets(root):
    targets = set()
    for dirpath, _, files in os.walk(os.path.join(root, 'Userland')):
        for name in files:
            if name == 'CMakeLists.txt' or name.endswith('.cmake'):
                text = read(os.path.join(dirpath, name))
                targets.update(m.group(1) for m in TARGET_CALL.finditer(text))
                for m in TARGETS_LIST.finditer(text):
                    targets.update(m.group(1).split())
    return targets


def check(root):
    problems = []
    apps_dir = os.path.join(root, 'Base', 'res', 'apps')
    if not os.path.isdir(apps_dir):
        raise FileNotFoundError(apps_dir)
    targets = build_targets(root)
    if not targets:
        raise FileNotFoundError('no build targets found under Userland/')
    af_names = set()
    icon_names = set()
    for icons_dir in glob.glob(os.path.join(root, 'Base', 'res', 'icons', '*')):
        if not os.path.isdir(icons_dir):
            continue
        for f in os.listdir(icons_dir):
            icon_names.add(f.rsplit('.', 1)[0])
    for af in sorted(glob.glob(os.path.join(apps_dir, '*.af'))):
        base = os.path.basename(af)
        af_names.add(base)
        text = read(af)
        ex = re.search(r'^Executable=/bin/(\S+)', text, re.M)
        if ex and ex.group(1) not in targets and ex.group(1) not in KNOWN_EXTERNAL:
            problems.append(f'{base}: Executable /bin/{ex.group(1)} has no build target')
        ic = re.search(r'^Icon=(\S+)', text, re.M)
        if ic and ic.group(1) not in icon_names:
            problems.append(f'{base}: Icon {ic.group(1)} not found in Base/res/icons')
    quick = os.path.join(root, 'Userland', 'Services', 'Taskbar', 'QuickLaunchWidget.cpp')
    for m in re.finditer(r'"([A-Za-z0-9_]+\.af)"', read(quick)):
        if m.group(1) not in af_names:
            problems.append(f'Taskbar quick-launch default {m.group(1)} does not exist in Base/res/apps')
    for ini in ('LaunchServer.ini', 'SystemServerUser.ini'):
        for m in re.finditer(r'=\s*/bin/([A-Za-z0-9_]+)\s*$|^Executable=/bin/([A-Za-z0-9_]+)', read(os.path.join(root, 'Base', 'etc', ini)), re.M):
            name = m.group(1) or m.group(2)
            if name not in targets and name not in KNOWN_EXTERNAL:
                problems.append(f'Base/etc/{ini}: /bin/{name} has no build target')
    return problems


def self_test():
    import shutil
    import tempfile
    ok = True
    with tempfile.TemporaryDirectory() as tmp:
        for rel in ('Base/res/apps', 'Base/res/icons/16x16', 'Base/etc', 'Userland/Services/Taskbar', 'Userland/Applications/A'):
            os.makedirs(os.path.join(tmp, rel))
        open(os.path.join(tmp, 'Userland/Applications/A/CMakeLists.txt'), 'w').write('exile_app(A ICON x)\n')
        open(os.path.join(tmp, 'Base/res/apps/A.af'), 'w').write('[App]\nName=A\nExecutable=/bin/A\nIcon=x\n')
        open(os.path.join(tmp, 'Base/res/icons/16x16/x.png'), 'w').write('')
        open(os.path.join(tmp, 'Base/etc/LaunchServer.ini'), 'w').write('[FileType]\ntxt=/bin/A\n')
        open(os.path.join(tmp, 'Base/etc/SystemServerUser.ini'), 'w').write('Executable=/bin/A\n')
        quick = os.path.join(tmp, 'Userland/Services/Taskbar/QuickLaunchWidget.cpp')
        open(quick, 'w').write('x({ 0, "A.af" });\n')
        if check(tmp):
            print('self-test: clean tree reported problems'); ok = False
        mutations = {
            'dead quick-launch default': lambda: open(quick, 'w').write('x({ 0, "Gone.af" });\n'),
            'af without target': lambda: open(os.path.join(tmp, 'Base/res/apps/B.af'), 'w').write('[App]\nExecutable=/bin/B\n'),
            'missing icon': lambda: open(os.path.join(tmp, 'Base/res/apps/A.af'), 'w').write('[App]\nExecutable=/bin/A\nIcon=nope\n'),
            'launch server to nothing': lambda: open(os.path.join(tmp, 'Base/etc/LaunchServer.ini'), 'w').write('[FileType]\ntxt=/bin/Zed\n'),
        }
        for name, mutate in mutations.items():
            shutil.rmtree(os.path.join(tmp, 'Base/res/apps')); os.makedirs(os.path.join(tmp, 'Base/res/apps'))
            open(os.path.join(tmp, 'Base/res/apps/A.af'), 'w').write('[App]\nName=A\nExecutable=/bin/A\nIcon=x\n')
            open(quick, 'w').write('x({ 0, "A.af" });\n')
            open(os.path.join(tmp, 'Base/etc/LaunchServer.ini'), 'w').write('[FileType]\ntxt=/bin/A\n')
            mutate()
            detected = bool(check(tmp))
            print(f'self-test mutation "{name}": {"detected" if detected else "MISSED"}')
            ok = ok and detected
    return 0 if ok else 1


def main():
    if '--self-test' in sys.argv:
        return self_test()
    try:
        problems = check(ROOT)
    except (OSError, FileNotFoundError) as e:
        print(f'lint-app-launchers: cannot read repository layout: {e}', file=sys.stderr)
        return 2
    for p in problems:
        print(p)
    return 1 if problems else 0


if __name__ == '__main__':
    sys.exit(main())
