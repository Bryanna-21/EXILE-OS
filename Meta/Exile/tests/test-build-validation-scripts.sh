#!/usr/bin/env bash
# Tests for Meta/Exile/scripts/{build-kernel,build-image,run-qemu,validate-scheduler-priority}.sh.
#
# What this proves: the scripts' own logic (dependency checks, exit codes, "no silent success", image validation, log judging)
# against STUBBED cmake/ninja/QEMU and SYNTHETIC logs and images. It does NOT build a kernel, boot anything, or test the scheduler.
# Exit: 0 all tests passed, 1 some failed.
set -u
REAL_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"
S="$REAL_ROOT/Meta/Exile/scripts"
TMP="$(mktemp -d)"; trap 'rm -rf "$TMP"' EXIT
pass=0; fail=0; skip=0
ok()   { pass=$((pass + 1)); echo "  PASS  $1"; }
bad()  { fail=$((fail + 1)); echo "  FAIL  $1  ($2)"; }
skp()  { skip=$((skip + 1)); echo "  SKIP  $1  ($2)"; }
expect_rc() { # name expected-rc actual-rc [output-file] [grep-pattern]
    if [ "$2" != "$3" ]; then bad "$1" "exit $3, wanted $2"; return; fi
    if [ -n "${5:-}" ] && ! grep -Eq "$5" "$4"; then bad "$1" "output lacks /$5/"; return; fi
    ok "$1"
}
last_line() { tail -n 1 "$1"; }

echo "== syntax"
for f in build-kernel build-image run-qemu validate-scheduler-priority; do
    if bash -n "$S/$f.sh" 2>"$TMP/syn"; then ok "bash -n $f.sh"; else bad "bash -n $f.sh" "$(head -n1 "$TMP/syn")"; fi
    [ -x "$S/$f.sh" ] && ok "$f.sh is executable" || bad "$f.sh is executable" "mode"
done

# ---------------------------------------------------------------------------------------------------- build-kernel.sh
echo "== build-kernel.sh (stubbed cmake/ninja)"
mk_root() { # dir
    local r="$1"; mkdir -p "$r/Meta/Exile/scripts" "$r/Meta/CMake" "$r/stubs" "$r/Toolchain/Local/x86_64/bin"
    cp "$S/build-kernel.sh" "$r/Meta/Exile/scripts/"; cp "$REAL_ROOT/Meta/find_compiler.sh" "$r/Meta/"
    cp "$REAL_ROOT/Meta/CMake/processor-count.cmake" "$r/Meta/CMake/"; cp "$REAL_ROOT/CMakeLists.txt" "$r/CMakeLists.txt"
    ( cd "$r" && git init -q . && git -c user.email=t@t -c user.name=t commit -q --allow-empty -m init )
    printf '#!/bin/sh\nexit 0\n' > "$r/Toolchain/Local/x86_64/bin/x86_64-serenity-g++"; chmod +x "$r/Toolchain/Local/x86_64/bin/x86_64-serenity-g++"
    # A minimal ELF64 x86-64 executable header. /bin/true is PE on Windows/MSYS2.
    ELF_FIXTURE="$(cygpath -w "$r/elf-kernel")" || return 1
    python3 -c 'import struct,sys; ident=bytes([127])+b"ELF"+bytes([2,1,1,0])+bytes(8); header=struct.pack("<16sHHIQQQIHHHHHH",ident,2,62,1,0,0,0,0,64,56,0,64,0,0); open(sys.argv[1],"wb").write(header)' "$ELF_FIXTURE" || return 1
    [ -s "$r/elf-kernel" ] || { echo "ERROR: ELF fixture was not created" >&2; return 1; }
    local real_cmake; real_cmake="$(command -v cmake)"
    cat > "$r/stubs/cmake" <<STUB
#!/bin/sh
case "\$1" in
  --version|-P) exec "$real_cmake" "\$@" ;;
  --build) mkdir -p "$r/Build/x86_64"; : > "$r/Build/x86_64/build.ninja"; echo "stub cmake --build \$*"; exit 0 ;;
  *) echo "stub cmake \$*"; exit \${STUB_CMAKE_RC:-0} ;;
esac
STUB
    cat > "$r/stubs/ninja" <<STUB
#!/bin/sh
echo "stub ninja \$*"; echo "SDE=\${SOURCE_DATE_EPOCH:-unset} TZ=\${TZ:-unset}" > "$r/ninja-env.txt"
[ "\${STUB_NINJA_RC:-0}" = 0 ] || { echo "FAILED: stub compile error"; exit \${STUB_NINJA_RC}; }
d="$r/Build/x86_64"
[ "\${STUB_NO_KERNEL:-0}" = 1 ] && exit 0
mkdir -p "\$d/Kernel/CMakeFiles/Kernel.dir/Tasks"; cp "$r/elf-kernel" "\$d/Kernel/Kernel"
[ "\${STUB_NO_SCHED_OBJ:-0}" = 1 ] || : > "\$d/Kernel/CMakeFiles/Kernel.dir/Tasks/Scheduler.cpp.o"
: > "\$d/Kernel/CMakeFiles/Kernel.dir/Tasks/FinalizerTask.cpp.o"
STUB
    chmod +x "$r/stubs/cmake" "$r/stubs/ninja"
}
R="$TMP/rootA"; mk_root "$R" || { echo "ERROR: failed to create build-kernel test fixture" >&2; exit 1; }; B="$R/Meta/Exile/scripts/build-kernel.sh"
run_bk() { ( cd "$R" && PATH="$R/stubs:$PATH" EXILE_VALIDATION_OUT="$R/out" bash "$B" "$@" ) >"$TMP/o" 2>&1; echo $?; }
rc=$(run_bk --check-only);                 expect_rc "check-only passes with tools and toolchain" 0 "$rc" "$TMP/o" '^RESULT PASS step=check-only'
rc=$(run_bk --bogus);                      expect_rc "unknown option is a usage error" 1 "$rc" "$TMP/o" '^RESULT USAGE'
rc=$(run_bk --arch sparc);                 expect_rc "unsupported arch is a usage error" 1 "$rc" "$TMP/o" '^RESULT USAGE'
rc=$(run_bk --jobs zero);                  expect_rc "non-numeric --jobs is a usage error" 1 "$rc" "$TMP/o" '^RESULT USAGE'
rm "$R/Build/x86_64/build.ninja" 2>/dev/null
rc=$(run_bk);                              expect_rc "successful build" 0 "$rc" "$TMP/o" '^RESULT PASS step=kernel-build'
[ -s "$R/out/kernel-build-summary.txt" ] && grep -q 'kernel sha256:' "$R/out/kernel-build-summary.txt" && ok "summary records the kernel checksum" || bad "summary records the kernel checksum" "missing"
rc=$(STUB_NINJA_RC=1 run_bk);              expect_rc "ninja failure => exit 4, no success line" 4 "$rc" "$TMP/o" '^RESULT FAIL step=kernel-build'
grep -q '^RESULT PASS' "$TMP/o" && bad "failed build never prints PASS" "PASS printed" || ok "failed build never prints PASS"
rm -rf "$R/Build"; rc=$(STUB_NO_KERNEL=1 run_bk); expect_rc "ninja ok but no kernel binary => exit 4" 4 "$rc" "$TMP/o" 'missing or empty'
rm -rf "$R/Build"; rc=$(STUB_NO_SCHED_OBJ=1 run_bk); expect_rc "no Scheduler object => exit 4" 4 "$rc" "$TMP/o" 'Scheduler'
rm -rf "$R/Build"; rc=$(run_bk --reproducible); expect_rc "--reproducible build passes" 0 "$rc" "$TMP/o" '^RESULT PASS'
grep -Eq '^SDE=[0-9]+ TZ=UTC$' "$R/ninja-env.txt" && ok "--reproducible exports SOURCE_DATE_EPOCH and TZ=UTC to the build" || bad "--reproducible exports env" "$(cat "$R/ninja-env.txt")"
rm "$R/Toolchain/Local/x86_64/bin/x86_64-serenity-g++"
rc=$(run_bk --check-only);                 expect_rc "missing cross compiler => BLOCKED exit 3" 3 "$rc" "$TMP/o" '^RESULT BLOCKED step=cross-toolchain'
mkdir -p "$TMP/nonin"; for t in git cmake python3 sha256sum file; do ln -sf "$(command -v $t)" "$TMP/nonin/$t"; done
rc=$( cd "$R" && PATH="$TMP/nonin" /bin/bash "$B" --check-only >"$TMP/o" 2>&1; echo $? )
expect_rc "missing ninja => exit 2" 2 "$rc" "$TMP/o" 'missing: ninja'

# ---------------------------------------------------------------------------------------------------- build-image.sh
echo "== build-image.sh (synthetic images)"
IMG="$TMP/img"; mkdir -p "$IMG"
run_bi() { bash "$S/build-image.sh" "$@" >"$TMP/o" 2>&1; echo $?; }
if command -v mke2fs >/dev/null 2>&1; then
    truncate -s 20M "$IMG/raw.img"; mke2fs -q -t ext2 -F "$IMG/raw.img" >/dev/null 2>&1
    rc=$(run_bi --bootloader qemu --validate-only "$IMG/raw.img");  expect_rc "raw ext2 image validates" 0 "$rc" "$TMP/o" '^RESULT PASS step=validate'
    head -c 20971520 /dev/zero > "$IMG/zero.img"
    rc=$(run_bi --bootloader qemu --validate-only "$IMG/zero.img"); expect_rc "all-zero image is rejected" 5 "$rc" "$TMP/o" 'RESULT FAIL'
    head -c 4096 /dev/urandom > "$IMG/small.img"
    rc=$(run_bi --bootloader qemu --validate-only "$IMG/small.img"); expect_rc "tiny image is rejected" 5 "$rc" "$TMP/o" 'RESULT FAIL'
    rc=$(run_bi --bootloader qemu --validate-only "$IMG/missing.img"); expect_rc "missing image is rejected" 5 "$rc" "$TMP/o" 'RESULT FAIL'
    # MBR disk: partition 1 starts at LBA 2048 and holds ext2
    python3 -I - "$IMG/mbr.img" <<'PY'
import struct, sys
size = 24 * 1024 * 1024; start = 2048; sectors = size // 512 - start
disk = bytearray(size)
disk[446 + 4] = 0x83; disk[446 + 8:446 + 12] = struct.pack("<I", start); disk[446 + 12:446 + 16] = struct.pack("<I", sectors)
disk[510:512] = b"\x55\xaa"
open(sys.argv[1], "wb").write(disk)
PY
    mke2fs -q -t ext2 -F -E offset=$((2048 * 512)) "$IMG/mbr.img" $(( (24 * 1024 * 1024 - 2048 * 512) / 1024 ))k >/dev/null 2>&1
    rc=$(run_bi --bootloader grub --validate-only "$IMG/mbr.img");  expect_rc "MBR disk with ext2 partition validates" 0 "$rc" "$TMP/o" 'ext2 root at byte offset 1048576'
    python3 -I - "$IMG/mbr.img" "$IMG/mbr-bad.img" <<'PY'
import sys
d = bytearray(open(sys.argv[1], "rb").read()); d[510:512] = b"\0\0"; open(sys.argv[2], "wb").write(d)
PY
    rc=$(run_bi --bootloader grub --validate-only "$IMG/mbr-bad.img"); expect_rc "missing MBR signature is rejected" 5 "$rc" "$TMP/o" '0x55AA'
    if command -v debugfs >/dev/null 2>&1; then
        printf 'kernel-bytes-A' > "$TMP/kernel-a"; printf 'kernel-bytes-B' > "$TMP/kernel-b"
        debugfs -w -R "mkdir /boot" "$IMG/mbr.img?offset=1048576" >/dev/null 2>&1
        debugfs -w -R "write $TMP/kernel-a /boot/Kernel" "$IMG/mbr.img?offset=1048576" >/dev/null 2>&1
        rc=$(EXILE_KERNEL_FOR_COMPARE="$TMP/kernel-a" run_bi --bootloader grub --validate-only "$IMG/mbr.img")
        expect_rc "kernel inside image equals the built kernel" 0 "$rc" "$TMP/o" 'identical to'
        rc=$(EXILE_KERNEL_FOR_COMPARE="$TMP/kernel-b" run_bi --bootloader grub --validate-only "$IMG/mbr.img")
        expect_rc "kernel inside image differing from the built kernel is rejected" 5 "$rc" "$TMP/o" 'differs from'
    else skp "kernel-in-image comparison" "debugfs not installed"; fi
else skp "image validation tests" "mke2fs not installed"; fi
rc=$(run_bi --bootloader nonsense); expect_rc "unknown bootloader is a usage error" 1 "$rc" "$TMP/o" '^RESULT USAGE'
# Prove missing host tools are BLOCKED (2), then isolate missing build artifacts (3).
IMG_STUBS="$TMP/image-stubs"; mkdir -p "$IMG_STUBS"
for t in ninja python3 sha256sum; do
    ln -sf "$(command -v "$t")" "$IMG_STUBS/$t"
done
rc=$(PATH="$IMG_STUBS:$PATH" run_bi)
expect_rc "missing mke2fs => host-tools BLOCKED exit 2" 2 "$rc" "$TMP/o" 'missing: .*mke2fs'
printf '#!/bin/sh\\nexit 0\\n' > "$IMG_STUBS/mke2fs"; chmod +x "$IMG_STUBS/mke2fs"
rc=$(PATH="$IMG_STUBS:$PATH" run_bi)
expect_rc "missing build prerequisites => BLOCKED exit 3" 3 "$rc" "$TMP/o" 'is not configured'

# ---------------------------------------------------------------------------------------------------- run-qemu.sh
echo "== run-qemu.sh (stub QEMU and stub launcher)"
Q="$TMP/qemu"; mkdir -p "$Q/bin" "$Q/root/Build/x86_64/Kernel" "$Q/root/Meta/Exile/scripts"
cp "$S/run-qemu.sh" "$Q/root/Meta/Exile/scripts/"
cp /bin/true "$Q/root/Build/x86_64/Kernel/Kernel"; head -c 1048576 /dev/zero > "$Q/root/Build/x86_64/_disk_image"
mkq() { printf '#!/bin/sh\necho "QEMU emulator version %s"\n' "$1" > "$Q/bin/qemu-system-x86_64"; chmod +x "$Q/bin/qemu-system-x86_64"; }
mkq 8.2.2
cat > "$Q/launcher.sh" <<'L'
#!/bin/sh
# Stub for Meta/run.sh: behaviour chosen by STUB_SCENARIO
echo "env: RUN=$EXILE_RUN CPUS=$EXILE_CPUS RAM=$EXILE_RAM_SIZE CMD=$EXILE_KERNEL_CMDLINE QEMU_BIN=${EXILE_QEMU_BIN:-} IMG=$EXILE_DISK_IMAGE"
command -v qemu-system-x86_64-exile-diag >/dev/null 2>&1 && echo "shim-present"
case "$STUB_SCENARIO" in
  ok)      echo "booting"; echo "SCHED-PRIO-TEST: cpus=1 high_priority=50 low_priority=30 high_min_count=9 low_count=0" > "$EXILE_BUILD_DIR/debug.log"; echo "SCHED-PRIO-TEST: result=pass" >> "$EXILE_BUILD_DIR/debug.log"; echo "FINALIZER-TEST: result=pass" >> "$EXILE_BUILD_DIR/debug.log"; sleep 30 ;;
  panic)   echo "KERNEL PANIC: boom"; sleep 30 ;;
  hang)    echo "booting"; sleep 30 ;;
  early)   echo "qemu died"; exit 1 ;;
esac
L
chmod +x "$Q/launcher.sh"
run_rq() { ( cd "$Q/root" && PATH="$Q/bin:$PATH" EXILE_RUN_SH="$Q/launcher.sh" EXILE_VALIDATION_OUT="$Q/out" STUB_SCENARIO="$SCEN" bash "$Q/root/Meta/Exile/scripts/run-qemu.sh" "$@" ) >"$TMP/o" 2>&1; echo $?; }
SCEN=ok;    rc=$(run_rq --profile scheduler --timeout 20); expect_rc "expected output seen => PASS" 0 "$rc" "$TMP/o" '^RESULT PASS step=boot'
grep -q 'RUN=ci' "$Q/out/qemu-serial.log" && ok "boot mode selects the ci machine" || bad "boot mode selects the ci machine" "$(grep env: "$Q/out/qemu-serial.log")"
grep -q 'system_mode=self-test' "$TMP/o" && ok "profile scheduler boots in self-test mode" || bad "profile scheduler self-test" "no cmdline"
grep -q 'SCHED-PRIO-TEST' "$Q/out/qemu-debug.log" && ok "debug log captured" || bad "debug log captured" "empty"
SCEN=panic; rc=$(run_rq --expect 'never' --timeout 20);        expect_rc "panic marker => FAIL exit 6" 6 "$rc" "$TMP/o" 'panic marker'
SCEN=hang;  rc=$(run_rq --expect 'never' --timeout 3);         expect_rc "timeout => FAIL exit 6" 6 "$rc" "$TMP/o" 'timeout after 3s'
SCEN=early; rc=$(run_rq --expect 'never' --timeout 20);        expect_rc "QEMU exits early => exit 4" 4 "$rc" "$TMP/o" 'exited before'
SCEN=ok;    rc=$(run_rq);                                      expect_rc "no --expect in boot mode => usage error" 1 "$rc" "$TMP/o" 'not proof'
SCEN=ok;    rc=$(run_rq --smp --expect x);                     expect_rc "--smp with one CPU => usage error" 1 "$rc" "$TMP/o" 'needs --cpus'
SCEN=ok;    rc=$(run_rq --profile scheduler --cpus 2 --smp --timeout 20); expect_rc "--smp boot passes with 2 CPUs" 0 "$rc" "$TMP/o" '^RESULT PASS'
grep -q 'smp=on' "$TMP/o" && ok "--smp adds smp=on to the kernel command line" || bad "--smp adds smp=on" "missing"
SCEN=ok;    rc=$(run_rq --mode debug --profile scheduler --timeout 20); expect_rc "debug mode boots" 0 "$rc" "$TMP/o" '^RESULT PASS'
grep -q 'shim-present' "$Q/out/qemu-serial.log" && ok "debug mode puts the QEMU diagnostics shim on PATH" || bad "debug shim" "not on PATH"
mkq 5.2.0; SCEN=ok; rc=$(run_rq --expect x);                   expect_rc "QEMU older than 6.2 => BLOCKED exit 2" 2 "$rc" "$TMP/o" 'older than'
rm "$Q/bin/qemu-system-x86_64"; rc=$(PATH="/usr/bin:/bin" run_rq --expect x); expect_rc "no QEMU => BLOCKED exit 2" 2 "$rc" "$TMP/o" '^RESULT BLOCKED step=qemu'
mkq 8.2.2; rm "$Q/root/Build/x86_64/Kernel/Kernel"; rc=$(run_rq --expect x); expect_rc "kernel missing => BLOCKED exit 3" 3 "$rc" "$TMP/o" 'kernel not built'

# ---------------------------------------------------------------------------------------------------- validate-scheduler-priority.sh
echo "== validate-scheduler-priority.sh (real host test, SYNTHETIC guest logs)"
V="$S/validate-scheduler-priority.sh"; L="$TMP/logs"; mkdir -p "$L"
GOOD1="$L/good1.log"; cat > "$GOOD1" <<'G'
SCHED-PRIO-TEST: cpus=1 high_priority=50 low_priority=30 high_min_count=90000000 low_count=0
SCHED-PRIO-TEST: result=pass
FINALIZER-TEST: cpus=1 started=20 reaped=20 elapsed_ms=140
FINALIZER-TEST: result=pass
G
run_v() { bash "$V" --out "$TMP/vout" "$@" >"$TMP/o" 2>&1; echo $?; }
if command -v g++ >/dev/null 2>&1 || command -v clang++ >/dev/null 2>&1; then
    rc=$(run_v --host-only);                        expect_rc "host-only: stages 4-7 are NOT RUN" 0 "$rc" "$TMP/o" 'NOT_RUN=4'
    rc=$(run_v --guest-log "$GOOD1" --skip-smp);    expect_rc "good guest log: stages 1-6 PASS, 7 NOT RUN" 0 "$rc" "$TMP/o" 'PASS=6 FAIL=0 BLOCKED=0 NOT_RUN=1'
    sed 's/low_count=0/low_count=88000000/;s/result=pass/result=fail/' "$GOOD1" > "$L/equal.log"
    rc=$(run_v --guest-log "$L/equal.log" --skip-smp); expect_rc "equal CPU share (priorities not effective) => FAIL" 4 "$rc" "$TMP/o" 'low priority limited under contention.*FAIL'
    sed '0,/SCHED-PRIO-TEST: result=pass/s//SCHED-PRIO-TEST: result=fail/' "$GOOD1" > "$L/guest-fail.log"
    rc=$(run_v --guest-log "$L/guest-fail.log" --skip-smp); expect_rc "guest result=fail overrides healthy-looking counters" 4 "$rc" "$TMP/o" 'PASS=4 FAIL=2 BLOCKED=0 NOT_RUN=1'
    sed 's/reaped=20/reaped=3/' "$GOOD1" > "$L/fin.log"
    rc=$(run_v --guest-log "$L/fin.log" --skip-smp);   expect_rc "finalizer reaped < started => FAIL" 4 "$rc" "$TMP/o" 'finalizer reaps under contention.*FAIL'
    { echo "KERNEL PANIC: x"; cat "$GOOD1"; } > "$L/panic.log"
    rc=$(run_v --guest-log "$L/panic.log" --skip-smp); expect_rc "panic in log => FAIL" 4 "$rc" "$TMP/o" 'panic marker'
    echo "boot output without test verdicts" > "$L/none.log"
    rc=$(run_v --guest-log "$L/none.log" --skip-smp);  expect_rc "no verdict lines => FAIL, never PASS" 4 "$rc" "$TMP/o" 'no SCHED-PRIO-TEST verdict'
    rc=$(run_v --guest-log "$GOOD1" --smp-log "$GOOD1"); expect_rc "SMP log taken with 1 CPU is rejected" 4 "$rc" "$TMP/o" '2-CPU smp=on repeat.*FAIL'
    sed 's/cpus=1/cpus=2/g' "$GOOD1" > "$L/smp.log"
    rc=$(run_v --guest-log "$GOOD1" --smp-log "$L/smp.log"); expect_rc "valid 2-CPU log: stage 7 PASS" 0 "$rc" "$TMP/o" 'PASS=7 FAIL=0'
    rc=$(run_v);                                    expect_rc "no QEMU here => guest stages BLOCKED, exit 3" 3 "$rc" "$TMP/o" 'BLOCKED=4'
    rc=$(run_v --guest-log /nonexistent);           expect_rc "missing log file is a usage error" 1 "$rc" "$TMP/o" '^RESULT USAGE'
else skp "validate-scheduler-priority.sh tests" "no host C++ compiler"; fi

echo
echo "SUMMARY: $pass passed, $fail failed, $skip skipped"
[ "$fail" -eq 0 ]
