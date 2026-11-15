#!/usr/bin/env bash
# Validate the scheduler priority fix (Kernel/Tasks/SchedulerPriority.h, commit 4313f3e9) at two separate levels.
#
#   HOST   Compiles Kernel/Tasks/SchedulerPriority.h with the kernel's own THREAD_PRIORITY_* values and checks the mapping
#          (Meta/Exile/tests/test-sched-priority-map.sh). This proves arithmetic. It says nothing about the running kernel.
#   GUEST  Boots the kernel in QEMU in self-test mode and judges the verdict lines printed by Tests/Kernel/TestSchedulerPriority
#          and Tests/Kernel/TestFinalizerUnderLoad. A host pass never turns a guest stage into a pass.
#
# Every stage ends in exactly one of: PASS, FAIL, NOT RUN (not attempted by request), BLOCKED (attempted or required, but a
# prerequisite is missing; the reason says which).
#
#   stage  what                                                    level
#   1      priority -> bucket mapping                              host
#   2      HIGH < NORMAL < LOW bucket ordering                     host
#   3      all 32 ready-queue buckets used                         host
#   4      high-priority threads run under CPU contention          guest (1 CPU)
#   5      low-priority thread is limited under contention         guest (1 CPU)
#   6      finalizer reaps exited processes under CPU contention   guest (1 CPU)
#   7      the same on 2 CPUs with smp=on                          guest (2 CPUs)
#
# Options
#   --host-only            Run stages 1-3 only; stages 4-7 are NOT RUN.
#   --guest-log FILE       Judge stages 4-6 from an existing serial/debug log instead of booting. Use for logs produced
#                          elsewhere (for example the CI artifact). Stage 7 needs --smp-log.
#   --smp-log FILE         Judge stage 7 from an existing log taken with 2+ CPUs and smp=on.
#   --skip-smp             Do not attempt stage 7 (reported as NOT RUN).
#   --out DIR              Results directory (default Build/m1-validation).
# Exit codes: 0 no stage FAILED and none BLOCKED unless --host-only; 1 usage; 4 a stage FAILED; 3 at least one required stage
# is BLOCKED; 2 the host test could not run (no compiler). The last line is a RESULT summary.
set -u -o pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"
SCRIPTS="$ROOT/Meta/Exile/scripts"
OUT="${EXILE_VALIDATION_OUT:-$ROOT/Build/m1-validation}"
HOST_ONLY=0; SKIP_SMP=0; GUEST_LOG=""; SMP_LOG=""

usage() { sed -n '2,/^set -u/p' "$0" | sed '$d' | sed 's/^# \{0,1\}//'; }
while [ $# -gt 0 ]; do
    case "$1" in
        --host-only) HOST_ONLY=1; shift ;;
        --guest-log) GUEST_LOG="${2:-}"; shift 2 || { echo "RESULT USAGE step=args reason=--guest-log needs a file"; exit 1; } ;;
        --smp-log) SMP_LOG="${2:-}"; shift 2 || { echo "RESULT USAGE step=args reason=--smp-log needs a file"; exit 1; } ;;
        --skip-smp) SKIP_SMP=1; shift ;;
        --out) OUT="${2:-}"; shift 2 || { echo "RESULT USAGE step=args reason=--out needs a value"; exit 1; } ;;
        -h|--help) usage; exit 0 ;;
        *) usage >&2; echo "RESULT USAGE step=args reason=unknown argument: $1"; exit 1 ;;
    esac
done
mkdir -p "$OUT" || { echo "RESULT USAGE step=setup reason=cannot create $OUT"; exit 1; }
for f in "$GUEST_LOG" "$SMP_LOG"; do [ -z "$f" ] || [ -f "$f" ] || { echo "RESULT USAGE step=args reason=no such log: $f"; exit 1; }; done

STATE=(); WHY=(); NAME=(
    "priority -> bucket mapping" "HIGH < NORMAL < LOW bucket ordering" "all 32 buckets used"
    "high priority runs under contention" "low priority limited under contention" "finalizer reaps under contention" "2-CPU smp=on repeat"
)
set_stage() { STATE[$1]="$2"; WHY[$1]="$3"; }   # index (0-based) state reason

# ------------------------------------------------------------------ HOST stages 1-3
HOST_LOG="$OUT/host-test.log"
bash "$ROOT/Meta/Exile/tests/test-sched-priority-map.sh" >"$HOST_LOG" 2>&1; host_rc=$?
if [ "$host_rc" -eq 2 ]; then
    for i in 0 1 2; do set_stage $i BLOCKED "no C++ compiler for the host test"; done
elif [ "$host_rc" -ne 0 ]; then
    tail -n 20 "$HOST_LOG" >&2
    for i in 0 1 2; do set_stage $i FAIL "host test failed (see $HOST_LOG)"; done
else
    mapped=$(grep -c '^PASS: priority [0-9]* -> bucket [0-9]*$' "$HOST_LOG")
    [ "$mapped" -eq 5 ] && set_stage 0 PASS "99/50/30/10/1 -> 0/15/21/28/31 ($mapped endpoint checks)" || set_stage 0 FAIL "expected 5 endpoint checks, saw $mapped"
    if grep -q '^PASS: mapping is non-increasing in priority$' "$HOST_LOG" \
        && grep -q '^PASS: priority 50 -> bucket 15$' "$HOST_LOG" && grep -q '^PASS: priority 30 -> bucket 21$' "$HOST_LOG" \
        && grep -q '^PASS: priority 10 -> bucket 28$' "$HOST_LOG"; then
        set_stage 1 PASS "HIGH(50)->15 < NORMAL(30)->21 < LOW(10)->28, mapping non-increasing"
    else set_stage 1 FAIL "ordering checks missing from $HOST_LOG"; fi
    grep -q '^PASS: all 32 buckets are used$' "$HOST_LOG" && set_stage 2 PASS "32 distinct buckets over priorities 1..99" || set_stage 2 FAIL "distribution check missing from $HOST_LOG"
fi

# ------------------------------------------------------------------ GUEST stages 4-7
# judge_log FILE EXPECT_CPUS_MIN EXPECT_CPUS_MAX -> prints "state|reason" for the three guest checks as three lines
judge_log() {
    local log_path="$1"
    if command -v cygpath >/dev/null 2>&1; then
        log_path="$(cygpath -w "$log_path")" || return 1
    fi
    python3 -I - "$log_path" "$2" "$3" <<'PY'
import re, sys
text = open(sys.argv[1], encoding="utf-8", errors="replace").read()
cmin, cmax = int(sys.argv[2]), int(sys.argv[3])
PANIC = ("KERNEL PANIC", "Kernel panic", "ASSERTION FAILED", "VERIFICATION FAILED", "Unhandled exception", "Double fault")
def out(state, reason): print(f"{state}|{reason}")
panic = next((m for m in PANIC if m in text), None)
prio = re.findall(r"SCHED-PRIO-TEST: cpus=(\d+) high_priority=(\d+) low_priority=(\d+) high_min_count=(\d+) low_count=(\d+)", text)
prio_res = re.findall(r"SCHED-PRIO-TEST: result=(pass|fail)", text)
fin = re.findall(r"FINALIZER-TEST: cpus=(\d+) started=(\d+) reaped=(\d+) elapsed_ms=(\d+)", text)
fin_res = re.findall(r"FINALIZER-TEST: result=(pass|fail)", text)
def judge_prio():
    if panic: return ("FAIL", f"panic marker in log: {panic}")
    if not prio or not prio_res: return ("FAIL", "no SCHED-PRIO-TEST verdict in the log (test did not run, hung, or the boot never reached it)")
    if prio_res[-1] != "pass": return ("FAIL", "guest reported SCHED-PRIO-TEST result=fail")
    cpus, hp, lp, hmin, low = (int(x) for x in prio[-1])
    if not (cmin <= cpus <= cmax): return ("FAIL", f"log was taken with {cpus} CPUs, expected {cmin}..{cmax}")
    return (hmin, low, cpus, hp, lp)
r = judge_prio()
if isinstance(r[0], str):
    out(*r); out(*r)
else:
    hmin, low, cpus, hp, lp = r
    out("PASS" if hmin > 0 else "FAIL", f"high_min_count={hmin} with {cpus} CPU(s), priority {hp} vs {lp}")
    ratio = (low / hmin) if hmin else float("inf")
    note = "fully starved (strict priority, no aging)" if low == 0 else f"low got {ratio:.4%} of a high thread's count"
    out("PASS" if low * 10 <= hmin else "FAIL", f"{note}; limit is 10%")
# finalizer
if panic: out("FAIL", f"panic marker in log: {panic}")
elif not fin or not fin_res: out("FAIL", "no FINALIZER-TEST verdict in the log")
else:
    c, started, reaped, ms = (int(x) for x in fin[-1])
    if not (cmin <= c <= cmax): out("FAIL", f"log was taken with {c} CPUs, expected {cmin}..{cmax}")
    elif started == 0 or reaped != started: out("FAIL", f"reaped {reaped} of {started} children in {ms} ms: finalizer starved or stuck")
    elif fin_res[-1] != "pass": out("FAIL", "numbers look fine but the guest reported result=fail")
    else: out("PASS", f"{reaped}/{started} children reaped in {ms} ms with {c} CPU(s) busy")
PY
}

apply_guest() { # base-index log cmin cmax
    local base="$1" log="$2" cmin="$3" cmax="$4" lines
    mapfile -t lines < <(judge_log "$log" "$cmin" "$cmax")
    local k
    for k in 0 1 2; do
        local line="${lines[$k]:-FAIL|judge produced no output}"
        set_stage $((base + k)) "${line%%|*}" "${line#*|}"
    done
}

if [ "$HOST_ONLY" = 1 ]; then
    for i in 3 4 5 6; do set_stage $i "NOT RUN" "--host-only"; done
else
    if [ -n "$GUEST_LOG" ]; then
        cp "$GUEST_LOG" "$OUT/guest-scheduler-test.log" 2>/dev/null
        apply_guest 3 "$GUEST_LOG" 1 1
    else
        GLOG="$OUT/guest-run"; mkdir -p "$GLOG"
        bash "$SCRIPTS/run-qemu.sh" --profile scheduler --cpus 1 --out "$GLOG" | tee "$OUT/guest-run-qemu.txt" | tail -n 3
        rc=${PIPESTATUS[0]}
        if [ "$rc" -eq 0 ]; then
            cat "$GLOG/qemu-serial.log" "$GLOG/qemu-debug.log" > "$OUT/guest-scheduler-test.log"
            apply_guest 3 "$OUT/guest-scheduler-test.log" 1 1
        elif [ "$rc" -eq 2 ] || [ "$rc" -eq 3 ]; then
            for i in 3 4 5; do set_stage $i BLOCKED "$(tail -n 1 "$OUT/guest-run-qemu.txt" | sed 's/^RESULT [A-Z]* //')"; done
        else
            for i in 3 4 5; do set_stage $i FAIL "boot did not reach the tests: $(tail -n 1 "$OUT/guest-run-qemu.txt" | sed 's/^RESULT [A-Z]* //')"; done
        fi
    fi
    # stage 7 (SMP)
    if [ "$SKIP_SMP" = 1 ]; then set_stage 6 "NOT RUN" "--skip-smp"
    elif [ -n "$SMP_LOG" ]; then
        mapfile -t sl < <(judge_log "$SMP_LOG" 2 64)
        worst=PASS; why=""
        for line in "${sl[@]}"; do [ "${line%%|*}" = PASS ] || worst=FAIL; why="$why${line#*|}; "; done
        set_stage 6 "$worst" "$why"
    elif [ -n "$GUEST_LOG" ]; then set_stage 6 "NOT RUN" "no --smp-log supplied"
    else
        SLOG="$OUT/smp-run"; mkdir -p "$SLOG"
        bash "$SCRIPTS/run-qemu.sh" --profile scheduler --cpus 2 --smp --out "$SLOG" | tee "$OUT/smp-run-qemu.txt" | tail -n 3
        rc=${PIPESTATUS[0]}
        if [ "$rc" -eq 0 ]; then
            cat "$SLOG/qemu-serial.log" "$SLOG/qemu-debug.log" > "$OUT/smp-scheduler-test.log"
            mapfile -t sl < <(judge_log "$OUT/smp-scheduler-test.log" 2 64)
            worst=PASS; why=""
            for line in "${sl[@]}"; do [ "${line%%|*}" = PASS ] || worst=FAIL; why="$why${line#*|}; "; done
            set_stage 6 "$worst" "$why"
        elif [ "$rc" -eq 2 ] || [ "$rc" -eq 3 ]; then set_stage 6 BLOCKED "$(tail -n 1 "$OUT/smp-run-qemu.txt" | sed 's/^RESULT [A-Z]* //')"
        else set_stage 6 FAIL "boot did not reach the tests: $(tail -n 1 "$OUT/smp-run-qemu.txt" | sed 's/^RESULT [A-Z]* //')"; fi
    fi
fi

# ------------------------------------------------------------------ report
RES="$OUT/scheduler-validation-results.tsv"; : > "$RES"
counts_pass=0; counts_fail=0; counts_blocked=0; counts_notrun=0
printf '\n%-3s %-40s %-9s %s\n' "#" "stage" "state" "detail"
for i in 0 1 2 3 4 5 6; do
    state="${STATE[$i]:-NOT RUN}"; why="${WHY[$i]:-not attempted}"
    level=host; [ "$i" -ge 3 ] && level=guest
    printf '%-3s %-40s %-9s %s\n' "$((i + 1))" "${NAME[$i]} [$level]" "$state" "$why"
    printf '%s\t%s\t%s\t%s\t%s\n' "$((i + 1))" "${NAME[$i]}" "$level" "$state" "$why" >> "$RES"
    case "$state" in PASS) counts_pass=$((counts_pass + 1)) ;; FAIL) counts_fail=$((counts_fail + 1)) ;; BLOCKED) counts_blocked=$((counts_blocked + 1)) ;; *) counts_notrun=$((counts_notrun + 1)) ;; esac
done
echo
echo "details: $RES"
summary="PASS=$counts_pass FAIL=$counts_fail BLOCKED=$counts_blocked NOT_RUN=$counts_notrun"
if [ "$host_rc" -eq 2 ]; then echo "RESULT BLOCKED step=host-test reason=no compiler ($summary)"; exit 2; fi
if [ "$counts_fail" -gt 0 ]; then echo "RESULT FAIL step=validate reason=$summary"; exit 4; fi
if [ "$counts_blocked" -gt 0 ]; then echo "RESULT BLOCKED step=validate reason=$summary"; exit 3; fi
if [ "$HOST_ONLY" = 1 ]; then echo "RESULT PASS step=host-only reason=$summary (guest stages NOT RUN)"; exit 0; fi
echo "RESULT PASS step=validate reason=$summary"; exit 0
