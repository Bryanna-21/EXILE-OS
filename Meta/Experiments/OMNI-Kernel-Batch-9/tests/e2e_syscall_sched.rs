//! End-to-end: syscall dispatch, thread lifecycle, scheduling and cleanup (Batch 7) running
//! over the Batch 8 memory layer. HOSTED; cooperative, uniprocessor.

mod common;
use common::*;
use omni_kernel7::sched::{MAX_THREADS, RunExit, SchedError, ThreadState};
use omni_kernel7::status;
use omni_kernel7::syscall::*;
use omni_kernel7::uaccess::USER_TOP;

#[test]
fn sys_01_unwired_and_unknown_numbers_are_no_sys_including_batch5_ipc() {
    let w = world(true);
    w.space(2, &[Range::rw(BASE, 1, 0)]);
    let (k, l) = (w.k, log());
    let l2 = l.clone();
    spawn_fn(&k.sched, 2, move || {
        // 0x100..0x107 are the Batch 5 IPC numbers: the Zig IPC layer is NOT wired into this
        // kernel, so they must be refused rather than silently succeed.
        for nr in [
            0u64,
            1,
            0x100,
            0x101,
            0x102,
            0x103,
            0x104,
            0x105,
            0x106,
            0x107,
            0x1EF,
            0x1F8,
            0xFFFF_FFFF,
            1 << 32,
            u64::MAX,
        ] {
            push(&l2, sc(k, nr, [1, 2, 3, 4, 5, 6]));
        }
    });
    run_all(&k.sched);
    assert_eq!(snap(&l), vec![status::NO_SYS; 15]);
}

#[test]
fn sys_02_debug_syscalls_are_off_by_default() {
    let w = world(false);
    w.space(2, &[Range::rw(BASE, 1, 9)]);
    let (k, l) = (w.k, log());
    let l2 = l.clone();
    spawn_fn(&k.sched, 2, move || {
        for nr in [SYS_DEBUG_CHECKSUM, SYS_DEBUG_FILL, SYS_DEBUG_ARGS] {
            push(&l2, sc(k, nr, [BASE, 4, 1, 0, 0, 0]));
        }
        push(
            &l2,
            sc(k, SYS_THREAD_SELF, [0; 6]) - sc(k, SYS_THREAD_SELF, [0; 6]),
        );
    });
    run_all(&k.sched);
    assert_eq!(
        snap(&l),
        vec![status::NO_SYS, status::NO_SYS, status::NO_SYS, 0]
    );
}

#[test]
fn sys_03_syscall_outside_thread_context_is_refused() {
    let w = world(true);
    assert_eq!(sc(w.k, SYS_THREAD_SELF, [0; 6]), status::INVALID_STATE);
    assert_eq!(
        sc(w.k, SYS_DEBUG_CHECKSUM, [BASE, 4, 0, 0, 0, 0]),
        status::INVALID_STATE
    );
}

#[test]
fn sys_04_pipeline_threads_in_three_spaces_run_to_completion_and_clean_up() {
    // The strongest hosted path: memory + syscalls + scheduling + context switching + cleanup.
    let w = world(true);
    let roots: Vec<u64> = (2u16..=4)
        .map(|s| w.space(s, &[Range::rw(BASE, 2, 0)]))
        .collect();
    let (k, l) = (w.k, log());
    let mut ids = Vec::new();
    for sp in 2u16..=4 {
        let l2 = l.clone();
        ids.push(spawn_fn(&k.sched, sp, move || {
            assert_eq!(sc(k, SYS_DEBUG_FILL, [BASE, 64, sp as u64, 0, 0, 0]), 0);
            assert_eq!(sc(k, SYS_YIELD, [0; 6]), 0);
            let sum = sc(k, SYS_DEBUG_CHECKSUM, [BASE, 64, 0, 0, 0, 0]);
            assert_eq!(sc(k, SYS_SPACE_SELF, [0; 6]), sp as i64);
            push(&l2, sum);
            sc(k, SYS_EXIT, [sum as u64, 0, 0, 0, 0, 0]);
            unreachable!("exit never returns");
        }));
    }
    assert_eq!(run_all(&k.sched), RunExit::AllDone);
    let mut got = snap(&l);
    got.sort();
    assert_eq!(got, vec![64 * 2, 64 * 3, 64 * 4]);
    for (id, sp) in ids.iter().zip(2i64..) {
        assert_eq!(k.sched.exit_code_of(*id), Some(64 * sp));
        assert_eq!(
            k.sched.state_of(*id),
            None,
            "an exited thread's id is stale"
        );
    }
    assert_eq!(k.sched.live(), 0);
    assert!(
        k.sched.stats().switches >= 6,
        "threads interleaved through the real context switch"
    );
    assert_eq!(k.sched.stats().stack_corruptions, 0);
    assert_eq!(k.sched.stats().bad_switches, 0);
    for (r, _) in roots.iter().zip(2..) {
        assert_eq!(
            w.byte_at(*r, BASE),
            Some(2 + (roots.iter().position(|x| x == r).unwrap() as u8))
        );
    }
}

#[test]
fn sys_05_exit_codes_kill_and_space_termination() {
    let w = world(true);
    w.space(5, &[Range::rw(BASE, 1, 0)]);
    let k = w.k;
    let a = spawn_fn(&k.sched, 5, move || {
        sc(k, SYS_SLEEP, [1000, 0, 0, 0, 0, 0]);
    });
    let b = spawn_fn(&k.sched, 5, move || {
        sc(k, SYS_SLEEP, [1000, 0, 0, 0, 0, 0]);
    });
    let c = spawn_fn(&k.sched, 6, move || {
        sc(k, SYS_EXIT, [(-7i64) as u64, 0, 0, 0, 0, 0]);
    });
    // run until everyone that can run has blocked; the idle callback returning false stops the run
    assert_eq!(k.sched.run(|_| false), RunExit::Stopped);
    assert_eq!(k.sched.state_of(a), Some(ThreadState::Sleeping));
    assert_eq!(k.sched.exit_code_of(c), Some(-7));
    assert_eq!(k.sched.terminate_space(5), Ok(2));
    assert_eq!(k.sched.terminate_space(0), Err(SchedError::InvalidState));
    assert_eq!(run_all(&k.sched), RunExit::AllDone);
    assert_eq!(k.sched.exit_code_of(a), Some(EXIT_KILLED));
    assert_eq!(k.sched.exit_code_of(b), Some(EXIT_KILLED));
    assert_eq!(k.sched.live(), 0);
}

#[test]
fn sys_06_sleep_wakes_by_tick_and_rejects_absurd_durations() {
    let w = world(true);
    w.space(2, &[Range::rw(BASE, 1, 0)]);
    let (k, l) = (w.k, log());
    let l2 = l.clone();
    spawn_fn(&k.sched, 2, move || {
        push(&l2, sc(k, SYS_SLEEP, [u64::MAX, 0, 0, 0, 0, 0]));
        push(&l2, sc(k, SYS_SLEEP, [3, 0, 0, 0, 0, 0]));
    });
    let mut ticks = 0;
    assert_eq!(
        k.sched.run(|s| {
            s.tick();
            ticks += 1;
            ticks < 100
        }),
        RunExit::AllDone
    );
    assert_eq!(snap(&l), vec![status::INVALID_ARGUMENT, status::OK]);
    assert!(
        (3..10).contains(&ticks),
        "slept about three ticks, not {ticks}"
    );
}

#[test]
fn sys_07_a_frame_returning_to_the_kernel_half_kills_the_thread() {
    let w = world(true);
    w.space(2, &[Range::rw(BASE, 1, 0)]);
    let k = w.k;
    let after = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let a = after.clone();
    let t = spawn_fn(&k.sched, 2, move || {
        let mut f = SyscallFrame {
            rax: SYS_YIELD,
            rcx: USER_TOP,
            r11: 0x202,
            user_rsp: 0x7FFF_0000,
            ..Default::default()
        };
        k.syscall(&mut f);
        a.store(true, std::sync::atomic::Ordering::SeqCst);
    });
    let a = after.clone();
    let t2 = spawn_fn(&k.sched, 2, move || {
        let mut f = SyscallFrame {
            rax: SYS_YIELD,
            rcx: 0x4000_0000,
            r11: 0x202,
            user_rsp: USER_TOP,
            ..Default::default()
        };
        k.syscall(&mut f);
        a.store(true, std::sync::atomic::Ordering::SeqCst);
    });
    run_all(&k.sched);
    assert!(
        !after.load(std::sync::atomic::Ordering::SeqCst),
        "an invalid frame must never reach the return path"
    );
    assert_eq!(k.sched.exit_code_of(t), Some(EXIT_BAD_FRAME));
    assert_eq!(k.sched.exit_code_of(t2), Some(EXIT_BAD_FRAME));
}

#[test]
fn sys_08_thread_table_exhaustion_fails_cleanly_and_slots_come_back() {
    let w = world(true);
    let k = w.k;
    let mut ids = Vec::new();
    for _ in 0..MAX_THREADS {
        ids.push(spawn_fn(&k.sched, 2, move || {
            sc(k, SYS_YIELD, [0; 6]);
        }));
    }
    extern "C" fn nop(_: usize) {}
    assert_eq!(k.sched.spawn(2, nop, 0), Err(SchedError::Full));
    assert_eq!(k.sched.live(), MAX_THREADS);
    run_all(&k.sched);
    assert_eq!(k.sched.live(), 0);
    // stale ids are rejected, even though their slots are being reused
    let fresh = k.sched.spawn(2, nop, 0).unwrap();
    assert_ne!(fresh, ids[0]);
    assert_eq!(k.sched.kill(ids[0]), Err(SchedError::Stale));
    assert_eq!(k.sched.wake(ids[0]), Err(SchedError::Stale));
    // The exit log keeps the last MAX_THREADS exit codes, keyed by generation-carrying id.
    assert_eq!(k.sched.exit_code_of(ids[0]), Some(0));
    run_all(&k.sched);
    for _ in 0..MAX_THREADS {
        k.sched.spawn(2, nop, 0).unwrap();
    }
    run_all(&k.sched);
    assert_eq!(
        k.sched.exit_code_of(ids[0]),
        None,
        "exit codes are retained for the most recent {MAX_THREADS} exits only"
    );
    assert_eq!(k.sched.stats().stack_corruptions, 0);
}

#[test]
fn sys_09_caller_identity_cannot_be_forged_through_registers() {
    let w = world(true);
    w.space(2, &[Range::rw(BASE, 1, 1)]);
    w.space(3, &[Range::rw(BASE, 1, 2)]);
    let (k, l) = (w.k, log());
    // thread in space 3 passes space 2's identity in every register; it must still act as 3
    let l2 = l.clone();
    spawn_fn(&k.sched, 3, move || {
        push(&l2, sc(k, SYS_SPACE_SELF, [2; 6]));
        push(&l2, sc(k, SYS_DEBUG_CHECKSUM, [BASE, 1, 2, 2, 2, 2]));
    });
    run_all(&k.sched);
    assert_eq!(snap(&l), vec![3, 2]);
}

#[test]
fn sys_10_every_status_a_syscall_returns_is_in_the_batch2_status_set() {
    let w = world(true);
    w.space(2, &[Range::rw(BASE, 1, 1)]);
    let (k, l) = (w.k, log());
    let l2 = l.clone();
    spawn_fn(&k.sched, 2, move || {
        for nr in [
            SYS_YIELD,
            SYS_SLEEP,
            SYS_THREAD_SELF,
            SYS_SPACE_SELF,
            SYS_DEBUG_CHECKSUM,
            SYS_DEBUG_FILL,
            0x100,
            u64::MAX,
        ] {
            for args in [
                [0u64; 6],
                [BASE, 4, 1, 0, 0, 0],
                [u64::MAX; 6],
                [BASE, DEBUG_COPY_MAX + 1, 300, 0, 0, 0],
            ] {
                if nr == SYS_SLEEP && args[0] != 0 && args[0] < u64::MAX / 2 {
                    continue;
                }
                let s = sc(k, nr, args);
                if s < 0 {
                    push(&l2, s);
                }
            }
        }
    });
    run_all(&k.sched);
    // Batch 2 wire statuses are -1..-17; Batch 5/7 add -32 (fault) and -33 (no_sys).
    let allowed: Vec<i64> = (1..=17).map(|n| -n).chain([-32, -33]).collect();
    let got = snap(&l);
    assert!(!got.is_empty());
    assert!(
        got.iter().all(|s| allowed.contains(s)),
        "unexpected status in {got:?}"
    );
}
