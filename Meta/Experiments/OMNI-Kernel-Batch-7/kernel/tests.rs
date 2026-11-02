//! Hosted tests. Everything marked REAL executes the production assembly and scheduler on
//! an ordinary Linux x86-64 process (real stack switches, real register save/restore).
//! "User threads" are kernel-mode coroutines that call the same `Kernel::syscall` the
//! SYSCALL stub calls; nothing here enters ring 0 or runs the SYSCALL stub.

use crate::context::{self, CANARY, FRAME_BYTES, STACK_SIZE, Stack};
use crate::msr;
use crate::sched::*;
use crate::status;
use crate::syscall::*;
use crate::uaccess::*;
use core::arch::global_asm;
use core::mem::{offset_of, size_of};
use core::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering};
use std::boxed::Box;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};
use std::vec;
use std::vec::Vec;

// ---- helpers -------------------------------------------------------------------------

#[derive(Default)]
struct SimMem {
    regions: Mutex<Vec<(SpaceId, u64, Vec<u8>)>>,
}

impl SimMem {
    fn add(&self, space: SpaceId, base: u64, len: usize, fill: u8) {
        self.regions
            .lock()
            .unwrap()
            .push((space, base, vec![fill; len]));
    }
    fn peek(&self, space: SpaceId, addr: u64, len: usize) -> Vec<u8> {
        let mut out = vec![0u8; len];
        assert!(self.read(space, addr, &mut out));
        out
    }
}

impl UserBacking for SimMem {
    fn read(&self, space: SpaceId, addr: u64, out: &mut [u8]) -> bool {
        let g = self.regions.lock().unwrap();
        for (s, b, v) in g.iter() {
            if *s == space && addr >= *b && addr + out.len() as u64 <= *b + v.len() as u64 {
                let o = (addr - *b) as usize;
                out.copy_from_slice(&v[o..o + out.len()]);
                return true;
            }
        }
        false
    }
    fn write(&self, space: SpaceId, addr: u64, data: &[u8]) -> bool {
        let mut g = self.regions.lock().unwrap();
        for (s, b, v) in g.iter_mut() {
            if *s == space && addr >= *b && addr + data.len() as u64 <= *b + v.len() as u64 {
                let o = (addr - *b) as usize;
                v[o..o + data.len()].copy_from_slice(data);
                return true;
            }
        }
        false
    }
}

struct FailingBacking;
impl UserBacking for FailingBacking {
    fn read(&self, _: SpaceId, _: u64, _: &mut [u8]) -> bool {
        false
    }
    fn write(&self, _: SpaceId, _: u64, _: &[u8]) -> bool {
        false
    }
}

/// Build a kernel on a big-stack OS thread (it is a few hundred KiB) and leak it: stacks
/// hold its address, so it must never move.
fn kernel(debug: bool) -> (&'static Kernel, &'static SimMem) {
    let mem: &'static SimMem = Box::leak(Box::new(SimMem::default()));
    let addr = thread::Builder::new()
        .stack_size(16 << 20)
        .spawn(move || Box::into_raw(Box::new(Kernel::new(mem, debug))) as usize)
        .unwrap()
        .join()
        .unwrap();
    // SAFETY: just leaked, never freed.
    (unsafe { &*(addr as *const Kernel) }, mem)
}

fn sched() -> &'static Sched {
    &kernel(false).0.sched
}

extern "C" fn tramp(arg: usize) {
    // SAFETY: `arg` came from `spawn_fn`.
    let b = unsafe { Box::from_raw(arg as *mut Box<dyn FnOnce()>) };
    let f: Box<dyn FnOnce()> = *b;
    f();
}

fn spawn_fn(s: &Sched, space: SpaceId, f: impl FnOnce() + 'static) -> ThreadId {
    let b: Box<Box<dyn FnOnce()>> = Box::new(Box::new(f));
    s.spawn(space, tramp, Box::into_raw(b) as usize).unwrap()
}

type Log = Arc<Mutex<Vec<i64>>>;
fn log() -> Log {
    Arc::new(Mutex::new(Vec::new()))
}
fn push(l: &Log, v: i64) {
    l.lock().unwrap().push(v);
}
fn snap(l: &Log) -> Vec<i64> {
    l.lock().unwrap().clone()
}

fn run_all(s: &Sched) -> RunExit {
    s.run(|_| false)
}

fn ticking(s: &Sched) -> bool {
    s.tick();
    true
}

fn sc(k: &Kernel, nr: u64, a: [u64; 6]) -> i64 {
    let mut f = SyscallFrame {
        rax: nr,
        rdi: a[0],
        rsi: a[1],
        rdx: a[2],
        r10: a[3],
        r8: a[4],
        r9: a[5],
        rcx: 0x40_0000,
        r11: 0x202,
        user_rsp: 0x7FFF_0000,
        ..Default::default()
    };
    k.syscall(&mut f);
    f.rax as i64
}

fn mk_space(k: &Kernel, mem: &SimMem, id: SpaceId, base: u64, len: u64, perm: Perm, fill: u8) {
    let mut sp = k.spaces.lock();
    sp.create(id).unwrap();
    sp.get_mut(id).unwrap().map(base, len, perm).unwrap();
    mem.add(id, base, len as usize, fill);
}

// ---- assembly probes (test only) ------------------------------------------------------

global_asm!(
    r#"
    .text
    .globl omni_test_align_entry
omni_test_align_entry:
    movq %rsp, %rax
    andq $15, %rax
    movq %rax, (%rdi)
    ret

    .globl omni_test_probe
omni_test_probe:
    pushq %rbx
    pushq %rbp
    pushq %r12
    pushq %r13
    pushq %r14
    pushq %r15
    pushq %rdi
    movq %rdi, %rbx
    leaq 1(%rdi), %rbp
    leaq 2(%rdi), %r12
    leaq 3(%rdi), %r13
    leaq 4(%rdi), %r14
    leaq 5(%rdi), %r15
    pushfq
    popq %rax
    andq $-2097153, %rax
    movq %rdi, %rcx
    andq $1, %rcx
    shlq $21, %rcx
    orq %rcx, %rax
    pushq %rax
    popfq
    movq %rsi, %rdi
    callq omni_test_yield
    xorl %eax, %eax
    movq (%rsp), %rdx
    cmpq %rdx, %rbx
    je .Lp1
    orl $1, %eax
.Lp1:
    leaq 1(%rdx), %rcx
    cmpq %rcx, %rbp
    je .Lp2
    orl $2, %eax
.Lp2:
    leaq 2(%rdx), %rcx
    cmpq %rcx, %r12
    je .Lp3
    orl $4, %eax
.Lp3:
    leaq 3(%rdx), %rcx
    cmpq %rcx, %r13
    je .Lp4
    orl $8, %eax
.Lp4:
    leaq 4(%rdx), %rcx
    cmpq %rcx, %r14
    je .Lp5
    orl $16, %eax
.Lp5:
    leaq 5(%rdx), %rcx
    cmpq %rcx, %r15
    je .Lp6
    orl $32, %eax
.Lp6:
    pushfq
    popq %rcx
    andq $2097152, %rcx
    movq %rdx, %rsi
    andq $1, %rsi
    shlq $21, %rsi
    cmpq %rsi, %rcx
    je .Lp7
    orl $64, %eax
.Lp7:
    popq %rdx
    popq %r15
    popq %r14
    popq %r13
    popq %r12
    popq %rbp
    popq %rbx
    ret
"#,
    options(att_syntax)
);

unsafe extern "C" {
    safe fn omni_test_align_entry(p: usize);
    fn omni_test_probe(seed: u64, sched: usize) -> u64;
}

#[unsafe(no_mangle)]
extern "C" fn omni_test_yield(s: usize) {
    // SAFETY: the probe passes the pinned scheduler.
    unsafe { (*(s as *const Sched)).yield_now().unwrap() };
}

// ---- CTX: context switch (REAL assembly, hosted) -----------------------------------------

extern "C" fn dummy_entry(_: usize) {}
extern "C" fn dummy_hook(_: usize) -> ! {
    unreachable!("never run: the frame is only inspected")
}

#[test]
fn ctx_01_initial_frame_layout() {
    let mut stack = Box::new(Stack::ZERO);
    let base = stack.0.as_mut_ptr();
    let rsp = unsafe { context::init_stack(base, STACK_SIZE, dummy_entry, 0xA1, dummy_hook, 0xB2) };
    assert_eq!(rsp % 16, 0, "saved rsp must be 16-byte aligned");
    assert!(context::validate_saved_rsp(base as usize, STACK_SIZE, rsp));
    let w = rsp as *const usize;
    let word = |i: usize| unsafe { w.add(i).read() };
    assert_eq!(word(0), 0xB2); // r15 = hook arg
    assert_eq!(word(1), dummy_hook as extern "C" fn(usize) -> ! as usize); // r14
    assert_eq!(word(2), 0xA1); // r13 = arg
    assert_eq!(word(3), dummy_entry as extern "C" fn(usize) as usize); // r12
    assert_eq!(word(4), 0); // rbx
    assert_eq!(word(5), 0); // rbp
    assert_eq!(word(6), 0x202); // rflags: IF, IOPL 0
    assert_eq!(word(7), context::trampoline_address());
    // after the final ret, rsp must be 16-aligned for the trampoline's calls
    assert_eq!((rsp + FRAME_BYTES) % 16, 0);
    assert_eq!(unsafe { (base as *const u64).read() }, CANARY);
    // scrubbed: no stale bytes outside the frame
    assert!(stack.0[8..STACK_SIZE - 128].iter().all(|&b| b == 0));
}

#[test]
fn ctx_02_validate_saved_rsp_bounds() {
    let (b, n) = (0x10000usize, STACK_SIZE);
    let top = b + n;
    assert!(context::validate_saved_rsp(b, n, top - 128));
    assert!(
        context::validate_saved_rsp(b, n, top - FRAME_BYTES),
        "frame ending exactly at top"
    );
    assert!(
        !context::validate_saved_rsp(b, n, top - FRAME_BYTES + 16),
        "frame past top"
    );
    assert!(!context::validate_saved_rsp(b, n, top - 120), "misaligned");
    assert!(!context::validate_saved_rsp(b, n, b), "inside canary");
    assert!(!context::validate_saved_rsp(b, n, b + 8), "misaligned low");
    assert!(context::validate_saved_rsp(b, n, b + 16));
    assert!(!context::validate_saved_rsp(b, n, 0));
    assert!(
        !context::validate_saved_rsp(b, n, usize::MAX - 15),
        "overflow"
    );
    assert!(!context::validate_saved_rsp(
        usize::MAX - 100,
        n,
        usize::MAX - 100
    ));
}

#[test]
fn ctx_03_callee_saved_registers_and_flags_survive_interleaving() {
    let s = sched();
    let l = log();
    for seed in [0x1111_0000u64, 0x2222_0001, 0x3333_0000] {
        let l = l.clone();
        spawn_fn(s, KERNEL_SPACE, move || {
            for _ in 0..3 {
                let m = unsafe { omni_test_probe(seed, s as *const Sched as usize) };
                push(&l, m as i64);
            }
        });
    }
    assert_eq!(run_all(s), RunExit::AllDone);
    assert_eq!(
        snap(&l),
        vec![0; 9],
        "a nonzero mask means a register or flag was clobbered"
    );
    assert!(s.stats().switches >= 12);
}

#[test]
fn ctx_04_new_thread_entry_sees_aligned_stack_and_its_argument() {
    let s = sched();
    let cell: &'static AtomicUsize = Box::leak(Box::new(AtomicUsize::new(99)));
    // omni_test_align_entry stores (rsp mod 16) at entry into *rdi
    s.spawn(
        KERNEL_SPACE,
        omni_test_align_entry,
        cell as *const AtomicUsize as usize,
    )
    .unwrap();
    assert_eq!(run_all(s), RunExit::AllDone);
    assert_eq!(
        cell.load(Ordering::SeqCst),
        8,
        "System V: rsp = 8 mod 16 at function entry"
    );
}

#[test]
fn ctx_05_returning_entry_runs_exit_hook() {
    let s = sched();
    let id = spawn_fn(s, KERNEL_SPACE, || {});
    assert_eq!(run_all(s), RunExit::AllDone);
    assert_eq!(s.exit_code_of(id), Some(0));
    assert_eq!(s.live(), 0);
}

#[test]
fn ctx_06_stack_canary_overflow_is_detected_and_thread_not_resumed() {
    let s = sched();
    let base = Arc::new(AtomicUsize::new(0));
    let after = Arc::new(AtomicBool::new(false));
    let (b2, a2) = (base.clone(), after.clone());
    let id = spawn_fn(s, KERNEL_SPACE, move || {
        unsafe { (b2.load(Ordering::SeqCst) as *mut u64).write(0) };
        s.yield_now().unwrap();
        a2.store(true, Ordering::SeqCst);
    });
    base.store(s.test_stack_base(id) as usize, Ordering::SeqCst);
    assert_eq!(run_all(s), RunExit::AllDone);
    assert!(
        !after.load(Ordering::SeqCst),
        "corrupted thread must never be resumed"
    );
    assert_eq!(s.stats().stack_corruptions, 1);
    assert_eq!(s.live(), 0);
    assert_eq!(s.exit_code_of(id), None);
}

#[test]
fn ctx_07_invalid_saved_rsp_is_refused() {
    let s = sched();
    let ran = Arc::new(AtomicU32::new(0));
    let mut ids = Vec::new();
    for _ in 0..3 {
        let r = ran.clone();
        ids.push(spawn_fn(s, KERNEL_SPACE, move || {
            r.fetch_add(1, Ordering::SeqCst);
        }));
    }
    let good = unsafe { s.test_peek_saved(ids[0]) };
    s.test_set_saved_rsp(ids[1], good + 1); // misaligned
    s.test_set_saved_rsp(ids[2], 0xdead_0000); // outside the stack
    assert_eq!(run_all(s), RunExit::AllDone);
    assert_eq!(ran.load(Ordering::SeqCst), 1, "only the intact thread runs");
    assert_eq!(s.stats().refused_switches, 2);
    assert_eq!(s.live(), 0);
}

#[test]
fn ctx_08_stale_ids_and_generation_reuse() {
    let s = sched();
    let a = spawn_fn(s, KERNEL_SPACE, || {});
    assert_eq!(run_all(s), RunExit::AllDone);
    assert_eq!(s.wake(a), Err(SchedError::Stale));
    assert_eq!(s.kill(a), Err(SchedError::Stale));
    assert_eq!(s.state_of(a), None);
    let b = spawn_fn(s, KERNEL_SPACE, || {});
    assert_eq!(a.index(), b.index(), "slot reused");
    assert_ne!(a, b, "but with a new generation");
    assert_eq!(s.wake(a), Err(SchedError::Stale));
    assert_eq!(s.wake(b), Ok(false));
    run_all(s);
}

// ---- SCH: scheduler ------------------------------------------------------------------------

#[test]
fn sch_01_fifo_round_robin() {
    let s = sched();
    let l = log();
    for n in 0..3i64 {
        let l = l.clone();
        spawn_fn(s, KERNEL_SPACE, move || {
            for _ in 0..3 {
                push(&l, n);
                s.yield_now().unwrap();
            }
        });
    }
    assert_eq!(run_all(s), RunExit::AllDone);
    assert_eq!(snap(&l), vec![0, 1, 2, 0, 1, 2, 0, 1, 2]);
}

#[test]
fn sch_02_blocking_calls_need_a_thread_context() {
    let s = sched();
    assert_eq!(s.current(), None);
    assert_eq!(s.yield_now(), Err(SchedError::InvalidState));
    assert_eq!(s.prepare_block().err(), Some(SchedError::InvalidState));
    assert_eq!(s.sleep_ticks(1).err(), Some(SchedError::InvalidState));
}

#[test]
fn sch_03_block_then_wake() {
    let s = sched();
    let l = log();
    let l1 = l.clone();
    let w = spawn_fn(s, KERNEL_SPACE, move || {
        let t = s.prepare_block().unwrap();
        push(&l1, 1);
        let r = s.block(t, None).unwrap();
        push(&l1, 10 + r as i64);
    });
    let l2 = l.clone();
    spawn_fn(s, KERNEL_SPACE, move || {
        push(
            &l2,
            if s.state_of(w) == Some(ThreadState::Blocked) {
                2
            } else {
                -2
            },
        );
        assert_eq!(s.wake(w), Ok(true));
        push(
            &l2,
            if s.state_of(w) == Some(ThreadState::Runnable) {
                3
            } else {
                -3
            },
        );
    });
    assert_eq!(run_all(s), RunExit::AllDone);
    assert_eq!(snap(&l), vec![1, 2, 3, 10 + Reason::Woken as i64]);
}

#[test]
fn sch_04_wake_between_prepare_and_block_is_not_lost() {
    let s = sched();
    let l = log();
    let l1 = l.clone();
    spawn_fn(s, KERNEL_SPACE, move || {
        let me = s.current().unwrap();
        let t = s.prepare_block().unwrap();
        assert_eq!(
            s.wake(me),
            Ok(false),
            "running thread: recorded, not queued"
        );
        push(&l1, s.block(t, None).unwrap() as i64);
    });
    assert_eq!(run_all(s), RunExit::AllDone);
    assert_eq!(snap(&l), vec![Reason::Woken as i64]);
    assert_eq!(s.stats().switches, 1, "returned without switching away");
}

static FIRED: AtomicBool = AtomicBool::new(false);
fn wake_once(s: &Sched, id: ThreadId) {
    if !FIRED.swap(true, Ordering::SeqCst) {
        s.wake(id).unwrap();
    }
}

#[test]
fn sch_05_wake_in_the_window_between_leaving_and_commit_is_not_lost() {
    let s = sched();
    let l = log();
    let l1 = l.clone();
    spawn_fn(s, KERNEL_SPACE, move || {
        let t = s.prepare_block().unwrap();
        push(&l1, s.block(t, None).unwrap() as i64);
    });
    // Fires right after the thread switched to the scheduler, before the block commits.
    s.test_set_pre_commit(Some(wake_once));
    assert_eq!(
        run_all(s),
        RunExit::AllDone,
        "a lost wake-up would leave it Blocked"
    );
    assert_eq!(snap(&l), vec![Reason::Woken as i64]);
}

#[test]
fn sch_06_reason_precedence() {
    assert!(Reason::None < Reason::Timeout);
    assert!(Reason::Timeout < Reason::Woken);
    assert!(Reason::Woken < Reason::Cancelled);
    assert!(Reason::Cancelled < Reason::Killed);
    let s = sched();
    let l = log();
    let l1 = l.clone();
    spawn_fn(s, KERNEL_SPACE, move || {
        let me = s.current().unwrap();
        let t = s.prepare_block().unwrap();
        s.wake(me).unwrap();
        s.cancel(me).unwrap();
        s.wake(me).unwrap();
        push(&l1, s.block(t, None).unwrap() as i64);
        // a stale reason must not leak into the next wait
        let t = s.prepare_block().unwrap();
        s.wake(me).unwrap();
        push(&l1, s.block(t, None).unwrap() as i64);
    });
    run_all(s);
    assert_eq!(
        snap(&l),
        vec![Reason::Cancelled as i64, Reason::Woken as i64]
    );
}

#[test]
fn sch_07_sleep_times_out_on_the_tick() {
    let s = sched();
    let l = log();
    let l1 = l.clone();
    let sl = spawn_fn(s, KERNEL_SPACE, move || {
        push(&l1, s.sleep_ticks(5).unwrap() as i64);
        push(&l1, s.ticks() as i64);
    });
    let l2 = l.clone();
    spawn_fn(s, KERNEL_SPACE, move || {
        push(
            &l2,
            if s.state_of(sl) == Some(ThreadState::Sleeping) {
                1
            } else {
                -1
            },
        );
    });
    assert_eq!(s.run(ticking), RunExit::AllDone);
    assert_eq!(snap(&l), vec![1, Reason::Timeout as i64, 5]);
}

#[test]
fn sch_08_sleep_woken_early() {
    let s = sched();
    let l = log();
    let l1 = l.clone();
    let sl = spawn_fn(s, KERNEL_SPACE, move || {
        push(&l1, s.sleep_ticks(50).unwrap() as i64);
        push(&l1, s.ticks() as i64);
    });
    spawn_fn(s, KERNEL_SPACE, move || {
        s.wake(sl).unwrap();
    });
    assert_eq!(s.run(ticking), RunExit::AllDone);
    assert_eq!(snap(&l), vec![Reason::Woken as i64, 0]);
}

#[test]
fn sch_09_zero_sleep_returns_at_once() {
    let s = sched();
    let l = log();
    let l1 = l.clone();
    spawn_fn(s, KERNEL_SPACE, move || {
        push(&l1, s.sleep_ticks(0).unwrap() as i64)
    });
    assert_eq!(run_all(s), RunExit::AllDone);
    assert_eq!(snap(&l), vec![Reason::Timeout as i64]);
}

#[test]
fn sch_10_kill_wakes_blocked_thread_and_is_sticky() {
    let s = sched();
    let l = log();
    let l1 = l.clone();
    let v = spawn_fn(s, KERNEL_SPACE, move || {
        let t = s.prepare_block().unwrap();
        push(&l1, s.block(t, None).unwrap() as i64);
        let t = s.prepare_block().unwrap();
        push(&l1, s.block(t, None).unwrap() as i64); // returns at once: sticky
    });
    spawn_fn(s, KERNEL_SPACE, move || {
        s.kill(v).unwrap();
        assert!(s.is_killed(v));
    });
    assert_eq!(run_all(s), RunExit::AllDone);
    assert_eq!(snap(&l), vec![Reason::Killed as i64, Reason::Killed as i64]);
}

#[test]
fn sch_11_kill_before_first_run_reaps_without_running() {
    let s = sched();
    let ran = Arc::new(AtomicBool::new(false));
    let r = ran.clone();
    let id = spawn_fn(s, KERNEL_SPACE, move || r.store(true, Ordering::SeqCst));
    s.kill(id).unwrap();
    assert_eq!(run_all(s), RunExit::AllDone);
    assert!(!ran.load(Ordering::SeqCst));
    assert_eq!(s.stats().killed_before_run, 1);
    assert_eq!(s.live(), 0);
}

#[test]
fn sch_12_terminate_space_kills_only_that_space() {
    let s = sched();
    let l = log();
    for _ in 0..3 {
        let l = l.clone();
        spawn_fn(s, 2, move || {
            loop {
                if s.sleep_ticks(100).unwrap() == Reason::Killed {
                    break;
                }
            }
            push(&l, 2);
        });
    }
    let l3 = l.clone();
    spawn_fn(s, 3, move || {
        assert_eq!(s.sleep_ticks(3).unwrap(), Reason::Timeout);
        push(&l3, 3);
    });
    let l0 = l.clone();
    spawn_fn(s, KERNEL_SPACE, move || {
        assert_eq!(
            s.terminate_space(KERNEL_SPACE),
            Err(SchedError::InvalidState)
        );
        push(&l0, s.terminate_space(2).unwrap() as i64 + 100);
        assert_eq!(s.terminate_space(2).unwrap(), 0, "already marked");
    });
    assert_eq!(s.run(ticking), RunExit::AllDone);
    let mut got = snap(&l);
    assert_eq!(got.remove(0), 103);
    got.sort();
    assert_eq!(got, vec![2, 2, 2, 3]);
    assert_eq!(s.stats().exited, 5);
    assert_eq!(s.live(), 0);
}

#[test]
fn sch_13_table_full_and_slot_reuse() {
    let s = sched();
    for _ in 0..MAX_THREADS {
        spawn_fn(s, KERNEL_SPACE, || {});
    }
    let b: Box<Box<dyn FnOnce()>> = Box::new(Box::new(|| {}));
    assert_eq!(
        s.spawn(KERNEL_SPACE, tramp, Box::into_raw(b) as usize),
        Err(SchedError::Full)
    );
    assert_eq!(run_all(s), RunExit::AllDone);
    assert_eq!(s.live(), 0);
    spawn_fn(s, KERNEL_SPACE, || {});
    assert_eq!(s.live(), 1);
    run_all(s);
    assert_eq!(s.stats().spawned, MAX_THREADS as u64 + 1);
    assert_eq!(s.stats().exited, MAX_THREADS as u64 + 1);
}

#[test]
fn sch_14_run_stops_with_blocked_threads_and_resumes_later() {
    let s = sched();
    let l = log();
    let l1 = l.clone();
    let id = spawn_fn(s, KERNEL_SPACE, move || {
        let t = s.prepare_block().unwrap();
        push(&l1, s.block(t, None).unwrap() as i64);
    });
    assert_eq!(run_all(s), RunExit::Stopped);
    assert_eq!(s.live(), 1);
    assert_eq!(s.state_of(id), Some(ThreadState::Blocked));
    assert_eq!(s.wake(id), Ok(true));
    assert_eq!(run_all(s), RunExit::AllDone);
    assert_eq!(snap(&l), vec![Reason::Woken as i64]);
}

#[test]
fn sch_15_idle_runs_only_when_nothing_is_runnable() {
    let s = sched();
    spawn_fn(s, KERNEL_SPACE, move || {
        s.sleep_ticks(3).unwrap();
    });
    let mut idles = 0;
    let exit = s.run(|s| {
        idles += 1;
        s.tick();
        true
    });
    assert_eq!(exit, RunExit::AllDone);
    assert_eq!(idles, 3);
}

static HOOKED: Mutex<Vec<u16>> = Mutex::new(Vec::new());
fn record_space(sp: SpaceId) {
    HOOKED.lock().unwrap().push(sp);
}

#[test]
fn sch_16_address_space_is_loaded_before_each_switch() {
    let s = sched();
    s.set_space_hook(Some(record_space));
    let l = log();
    for sp in [2u16, 3, 2] {
        let l = l.clone();
        spawn_fn(s, sp, move || {
            for _ in 0..2 {
                push(&l, (s.loaded_space() == sp) as i64);
                s.yield_now().unwrap();
            }
        });
    }
    assert_eq!(run_all(s), RunExit::AllDone);
    assert_eq!(
        snap(&l),
        vec![1; 6],
        "every slice ran with its own space loaded"
    );
    assert_eq!(*HOOKED.lock().unwrap(), vec![2, 3, 2, 3, 2, 3, 2]);
    assert_eq!(
        s.stats().space_loads,
        7,
        "reload skipped when the space did not change"
    );
}

#[test]
fn sch_17_async_wakes_from_another_cpu_never_lose_a_wakeup() {
    const N: u32 = 20_000;
    let s = sched();
    let cond = Arc::new(AtomicBool::new(false));
    let done = Arc::new(AtomicU32::new(0));
    let (c, d) = (cond.clone(), done.clone());
    let id = spawn_fn(s, KERNEL_SPACE, move || {
        while d.load(Ordering::SeqCst) < N {
            let t = s.prepare_block().unwrap();
            if c.swap(false, Ordering::SeqCst) {
                d.fetch_add(1, Ordering::SeqCst);
                continue;
            }
            s.block(t, None).unwrap();
        }
    });
    let (c2, d2) = (cond.clone(), done.clone());
    let waker = thread::spawn(move || {
        let limit = Instant::now() + Duration::from_secs(20);
        for i in 0..N {
            c2.store(true, Ordering::SeqCst);
            let _ = s.wake(id);
            while d2.load(Ordering::SeqCst) <= i {
                if Instant::now() > limit {
                    return;
                }
                core::hint::spin_loop();
            }
        }
    });
    let limit = Instant::now() + Duration::from_secs(25);
    let exit = s.run(|_| Instant::now() < limit);
    waker.join().unwrap();
    assert_eq!(exit, RunExit::AllDone, "Stopped means a wake-up was lost");
    assert_eq!(done.load(Ordering::SeqCst), N);
}

// ---- SYS: syscall dispatch -------------------------------------------------------------------

#[test]
fn sys_01_unknown_and_aliased_numbers_are_no_sys() {
    let (k, mem) = kernel(true);
    mk_space(k, mem, 2, 0x10000, 0x1000, Perm::RW, 0);
    let l = log();
    let l1 = l.clone();
    spawn_fn(&k.sched, 2, move || {
        for nr in [
            0u64,
            1,
            0xFF,
            0x100,
            0x107,
            0x1EF,
            0x1F8,
            0xFFFF_FFFF,
            (1 << 32) | SYS_YIELD,
            (1 << 63) | SYS_THREAD_SELF,
            u64::MAX,
        ] {
            push(&l1, sc(k, nr, [0; 6]));
        }
    });
    run_all(&k.sched);
    assert_eq!(snap(&l), vec![status::NO_SYS; 11]);
}

#[test]
fn sys_02_caller_identity_comes_from_the_scheduler_not_registers() {
    let (k, _) = kernel(false);
    let l = log();
    let ids = Arc::new(Mutex::new(Vec::new()));
    let mut made = Vec::new();
    for sp in [2u16, 3] {
        let (l, ids) = (l.clone(), ids.clone());
        made.push(spawn_fn(&k.sched, sp, move || {
            // fill every argument register with the *other* thread's identity
            let other = ids.lock().unwrap().first().copied().unwrap_or(0xDEAD);
            let me = sc(k, SYS_THREAD_SELF, [other; 6]);
            push(&l, me);
            push(&l, sc(k, SYS_SPACE_SELF, [other; 6]));
        }));
    }
    ids.lock().unwrap().push(made[0].raw() as u64);
    run_all(&k.sched);
    assert_eq!(
        snap(&l),
        vec![made[0].raw() as i64, 2, made[1].raw() as i64, 3]
    );
}

#[test]
fn sys_03_syscall_outside_thread_context_is_refused() {
    let (k, _) = kernel(false);
    assert_eq!(sc(k, SYS_THREAD_SELF, [0; 6]), status::INVALID_STATE);
}

#[test]
fn sys_04_exit_terminates_and_never_returns() {
    let (k, _) = kernel(false);
    let after = Arc::new(AtomicBool::new(false));
    let a = after.clone();
    let id = spawn_fn(&k.sched, 2, move || {
        sc(k, SYS_EXIT, [42, 0, 0, 0, 0, 0]);
        a.store(true, Ordering::SeqCst);
    });
    let a = after.clone();
    let id2 = spawn_fn(&k.sched, 2, move || {
        sc(k, SYS_EXIT, [(-5i64) as u64, 0, 0, 0, 0, 0]);
        a.store(true, Ordering::SeqCst);
    });
    run_all(&k.sched);
    assert!(!after.load(Ordering::SeqCst));
    assert_eq!(k.sched.exit_code_of(id), Some(42));
    assert_eq!(k.sched.exit_code_of(id2), Some(-5));
    assert_eq!(k.sched.live(), 0);
}

#[test]
fn sys_05_yield_syscall_interleaves_threads() {
    let (k, _) = kernel(false);
    let l = log();
    for n in 0..2i64 {
        let l = l.clone();
        spawn_fn(&k.sched, 2, move || {
            for _ in 0..2 {
                push(&l, n);
                assert_eq!(sc(k, SYS_YIELD, [0; 6]), 0);
            }
        });
    }
    run_all(&k.sched);
    assert_eq!(snap(&l), vec![0, 1, 0, 1]);
}

#[test]
fn sys_06_sleep_bounds_and_results() {
    let (k, _) = kernel(false);
    let l = log();
    let l1 = l.clone();
    let sl = spawn_fn(&k.sched, 2, move || {
        push(&l1, sc(k, SYS_SLEEP, [MAX_SLEEP_TICKS + 1, 0, 0, 0, 0, 0]));
        push(&l1, sc(k, SYS_SLEEP, [2, 0, 0, 0, 0, 0]));
        push(&l1, sc(k, SYS_SLEEP, [1000, 0, 0, 0, 0, 0]));
    });
    let mut stage = 0;
    k.sched.run(|s| {
        stage += 1;
        if stage == 3 {
            s.wake(sl).unwrap(); // cut the long sleep short
        } else {
            s.tick();
        }
        true
    });
    assert_eq!(snap(&l), vec![status::INVALID_ARGUMENT, status::OK, 1]);
}

#[test]
fn sys_07_bad_user_frame_terminates_the_thread() {
    let (k, _) = kernel(false);
    let after = Arc::new(AtomicU32::new(0));
    let mut bad = Vec::new();
    for (rip, rsp) in [
        (USER_TOP, 0x7000u64),
        (0xFFFF_8000_0000_0000, 0x7000),
        (0x40_0000, u64::MAX),
        (0x40_0000, USER_TOP),
    ] {
        let a = after.clone();
        bad.push(spawn_fn(&k.sched, 2, move || {
            let mut f = SyscallFrame {
                rax: SYS_YIELD,
                rcx: rip,
                user_rsp: rsp,
                r11: 0x202,
                ..Default::default()
            };
            k.syscall(&mut f);
            a.fetch_add(1, Ordering::SeqCst);
        }));
    }
    let a = after.clone();
    let ok = spawn_fn(&k.sched, 2, move || {
        let mut f = SyscallFrame {
            rax: SYS_YIELD,
            rcx: USER_TOP - 1,
            user_rsp: USER_TOP - 8,
            r11: 0x202,
            ..Default::default()
        };
        k.syscall(&mut f);
        a.fetch_add(100, Ordering::SeqCst);
    });
    run_all(&k.sched);
    assert_eq!(
        after.load(Ordering::SeqCst),
        100,
        "only the valid frame returned"
    );
    for id in bad {
        assert_eq!(k.sched.exit_code_of(id), Some(EXIT_BAD_FRAME));
    }
    assert_eq!(k.sched.exit_code_of(ok), Some(0));
}

#[test]
fn sys_08_user_rflags_are_sanitised() {
    let (k, _) = kernel(false);
    let l = log();
    let l1 = l.clone();
    spawn_fn(&k.sched, 2, move || {
        for r11 in [
            u64::MAX,
            0,
            0x3000 | 0x200 | 0x100 | 0x4000 | 0x1_0000 | 0x2_0000,
        ] {
            let mut f = SyscallFrame {
                rax: SYS_THREAD_SELF,
                rcx: 0x40_0000,
                r11,
                ..Default::default()
            };
            k.syscall(&mut f);
            push(&l1, f.r11 as i64);
        }
    });
    run_all(&k.sched);
    assert_eq!(snap(&l), vec![0xED7, 0x202, 0x202]);
}

#[test]
fn sys_09_killed_thread_exits_at_the_syscall_boundary() {
    let (k, _) = kernel(false);
    let l = log();
    let l1 = l.clone();
    let victim = spawn_fn(&k.sched, 2, move || {
        loop {
            sc(k, SYS_YIELD, [0; 6]);
            push(&l1, 1);
        }
    });
    let l2 = l.clone();
    spawn_fn(&k.sched, 2, move || {
        sc(k, SYS_YIELD, [0; 6]);
        sc(k, SYS_YIELD, [0; 6]);
        k.sched.kill(victim).unwrap();
        push(&l2, 9);
    });
    run_all(&k.sched);
    assert_eq!(snap(&l), vec![1, 1, 9], "no progress after the kill");
    assert_eq!(k.sched.exit_code_of(victim), Some(EXIT_KILLED));
}

#[test]
fn sys_10_extern_c_shim_reaches_the_kernel() {
    let (k, _) = kernel(false);
    let l = log();
    let l1 = l.clone();
    let id = spawn_fn(&k.sched, 2, move || {
        let mut f = SyscallFrame {
            rax: SYS_THREAD_SELF,
            rcx: 0x40_0000,
            r11: 0x202,
            ..Default::default()
        };
        unsafe { omni_syscall_common(&mut f) }; // no kernel installed yet
        push(&l1, f.rax as i64);
        install_kernel(k);
        let mut f = SyscallFrame {
            rax: SYS_THREAD_SELF,
            rcx: 0x40_0000,
            r11: 0x202,
            ..Default::default()
        };
        unsafe { omni_syscall_common(&mut f) };
        push(&l1, f.rax as i64);
        unsafe { omni_syscall_common(core::ptr::null_mut()) };
    });
    run_all(&k.sched);
    assert_eq!(snap(&l), vec![status::NO_SYS, id.raw() as i64]);
}

#[test]
fn sys_11_frame_layout_matches_the_entry_stub() {
    assert_eq!(size_of::<SyscallFrame>(), 128);
    let offs = [
        offset_of!(SyscallFrame, r15),
        offset_of!(SyscallFrame, r14),
        offset_of!(SyscallFrame, r13),
        offset_of!(SyscallFrame, r12),
        offset_of!(SyscallFrame, rbp),
        offset_of!(SyscallFrame, rbx),
        offset_of!(SyscallFrame, r11),
        offset_of!(SyscallFrame, r10),
        offset_of!(SyscallFrame, r9),
        offset_of!(SyscallFrame, r8),
        offset_of!(SyscallFrame, rcx),
        offset_of!(SyscallFrame, rdx),
        offset_of!(SyscallFrame, rsi),
        offset_of!(SyscallFrame, rdi),
        offset_of!(SyscallFrame, rax),
        offset_of!(SyscallFrame, user_rsp),
    ];
    for (i, o) in offs.iter().enumerate() {
        assert_eq!(*o, i * 8);
    }
    assert_eq!(offset_of!(PerCpu, kernel_rsp), 0);
    assert_eq!(offset_of!(PerCpu, user_rsp), 8);
}

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

#[test]
fn sys_12_entry_stub_bytes() {
    let b = entry_stub_bytes();
    assert!(b.len() > 64 && b.len() < 256, "len {}", b.len());
    assert_eq!(&b[0..3], &[0x0F, 0x01, 0xF8], "swapgs first");
    assert_eq!(
        find(b, &[0x65, 0x48, 0x89, 0x24, 0x25, 8, 0, 0, 0]),
        Some(3),
        "mov %rsp,%gs:8"
    );
    assert_eq!(
        find(b, &[0x65, 0x48, 0x8B, 0x24, 0x25, 0, 0, 0, 0]),
        Some(12),
        "mov %gs:0,%rsp"
    );
    assert!(find(b, &[0xFC]).is_some(), "cld before calling Rust");
    assert_eq!(
        &b[b.len() - 6..],
        &[0x0F, 0x01, 0xF8, 0x48, 0x0F, 0x07],
        "ends swapgs; sysretq"
    );
}

#[test]
fn sys_13_argument_registers_map_in_abi_order() {
    let (k, _) = kernel(true);
    let l = log();
    let l1 = l.clone();
    spawn_fn(&k.sched, 2, move || {
        push(
            &l1,
            sc(k, SYS_DEBUG_ARGS, [1, 10, 100, 1000, 10000, 100000]),
        );
    });
    run_all(&k.sched);
    assert_eq!(
        snap(&l),
        vec![654_321],
        "rdi,rsi,rdx,r10,r8,r9 -> positions 1..6"
    );
}

#[test]
fn sys_14_debug_syscalls_are_off_by_default() {
    let (k, _) = kernel(false);
    let l = log();
    let l1 = l.clone();
    spawn_fn(&k.sched, 2, move || {
        for nr in [SYS_DEBUG_CHECKSUM, SYS_DEBUG_FILL, SYS_DEBUG_ARGS] {
            push(&l1, sc(k, nr, [0x10000, 4, 0, 0, 0, 0]));
        }
    });
    run_all(&k.sched);
    assert_eq!(snap(&l), vec![status::NO_SYS; 3]);
}

// ---- UA: user memory -----------------------------------------------------------------------

#[test]
fn ua_01_map_validation() {
    let mut sp = AddressSpace::new(2);
    assert_eq!(sp.map(0x10000, 0, Perm::R), Err(UserError::BadRange));
    assert_eq!(sp.map(0x10001, 0x1000, Perm::R), Err(UserError::BadRange));
    assert_eq!(sp.map(0x10000, 0x1001, Perm::R), Err(UserError::BadRange));
    assert_eq!(
        sp.map(0, 0x1000, Perm::R),
        Err(UserError::BadRange),
        "null page"
    );
    assert_eq!(sp.map(USER_TOP, 0x1000, Perm::R), Err(UserError::BadRange));
    assert_eq!(
        sp.map(USER_TOP - 0x1000, 0x2000, Perm::R),
        Err(UserError::BadRange)
    );
    assert_eq!(
        sp.map(0xFFFF_FFFF_FFFF_F000, 0x2000, Perm::R),
        Err(UserError::BadRange),
        "overflow"
    );
    assert_eq!(sp.map(0x10000, 0x2000, Perm::RW), Ok(()));
    assert_eq!(
        sp.map(0x11000, 0x1000, Perm::R),
        Err(UserError::BadRange),
        "overlap"
    );
    assert_eq!(
        sp.map(0xF000, 0x2000, Perm::R),
        Err(UserError::BadRange),
        "overlap below"
    );
    assert_eq!(
        sp.map(USER_TOP - 0x1000, 0x1000, Perm::R),
        Ok(()),
        "last page is fine"
    );
    for i in 0..MAX_REGIONS - 2 {
        assert_eq!(
            sp.map(0x100000 + i as u64 * 0x2000, 0x1000, Perm::R),
            Ok(())
        );
    }
    assert_eq!(sp.map(0x900000, 0x1000, Perm::R), Err(UserError::Full));
}

#[test]
fn ua_02_check_range() {
    let mut sp = AddressSpace::new(2);
    sp.map(0x10000, 0x1000, Perm::RW).unwrap();
    sp.map(0x11000, 0x1000, Perm::R).unwrap();
    sp.map(0x20000, 0x1000, Perm::RW).unwrap();
    let r = Access::Read;
    let w = Access::Write;
    assert_eq!(sp.check_range(0x10000, 0x1000, r), Ok(()));
    assert_eq!(sp.check_range(0x10FFF, 1, r), Ok(()));
    assert_eq!(
        sp.check_range(0x10FFF, 2, r),
        Ok(()),
        "spans adjacent regions"
    );
    assert_eq!(
        sp.check_range(0x10FFF, 2, w),
        Err(UserError::Fault),
        "second region is read-only"
    );
    assert_eq!(sp.check_range(0x11000, 1, w), Err(UserError::Fault));
    assert_eq!(
        sp.check_range(0x12000, 1, r),
        Err(UserError::Fault),
        "one past the end"
    );
    assert_eq!(
        sp.check_range(0x11FFF, 2, r),
        Err(UserError::Fault),
        "gap after"
    );
    assert_eq!(
        sp.check_range(0x1FFFF, 2, r),
        Err(UserError::Fault),
        "starts in a gap"
    );
    assert_eq!(sp.check_range(0, 1, r), Err(UserError::Fault));
    assert_eq!(
        sp.check_range(u64::MAX - 3, 8, r),
        Err(UserError::Fault),
        "wraps"
    );
    assert_eq!(sp.check_range(0x10000, u64::MAX, r), Err(UserError::Fault));
    assert_eq!(sp.check_range(USER_TOP - 1, 2, r), Err(UserError::Fault));
    assert_eq!(
        sp.check_range(0xFFFF_8000_0000_0000, 1, r),
        Err(UserError::Fault),
        "kernel half"
    );
    assert_eq!(sp.check_range(0x30000, 0, r), Ok(()));
    assert_eq!(sp.check_range(USER_TOP + 1, 0, r), Err(UserError::Fault));
}

#[test]
fn ua_03_copy_roundtrip_through_syscalls() {
    let (k, mem) = kernel(true);
    mk_space(k, mem, 2, 0x10000, 0x2000, Perm::RW, 0);
    let l = log();
    let l1 = l.clone();
    spawn_fn(&k.sched, 2, move || {
        push(&l1, sc(k, SYS_DEBUG_FILL, [0x10010, 16, 0xAB, 0, 0, 0]));
        push(&l1, sc(k, SYS_DEBUG_CHECKSUM, [0x10010, 16, 0, 0, 0, 0]));
        push(&l1, sc(k, SYS_DEBUG_CHECKSUM, [0x10000, 16, 0, 0, 0, 0]));
        push(&l1, sc(k, SYS_DEBUG_FILL, [0x10000, 1, 256, 0, 0, 0]));
    });
    run_all(&k.sched);
    assert_eq!(snap(&l), vec![0, 16 * 0xAB, 0, status::INVALID_ARGUMENT]);
    assert_eq!(mem.peek(2, 0x10010, 16), vec![0xAB; 16]);
    assert_eq!(mem.peek(2, 0x1000F, 1), vec![0], "byte before untouched");
}

#[test]
fn ua_04_two_spaces_same_address_different_memory() {
    let (k, mem) = kernel(true);
    mk_space(k, mem, 2, 0x10000, 0x1000, Perm::RW, 1);
    mk_space(k, mem, 3, 0x10000, 0x1000, Perm::RW, 2);
    k.spaces
        .lock()
        .get_mut(2)
        .unwrap()
        .map(0x30000, 0x1000, Perm::RW)
        .unwrap();
    mem.add(2, 0x30000, 0x1000, 7);
    let l = log();
    for sp in [2u16, 3] {
        let l = l.clone();
        spawn_fn(&k.sched, sp, move || {
            push(&l, sc(k, SYS_DEBUG_CHECKSUM, [0x10000, 4, 0, 0, 0, 0]));
            push(&l, sc(k, SYS_DEBUG_CHECKSUM, [0x30000, 4, 0, 0, 0, 0]));
        });
    }
    run_all(&k.sched);
    assert_eq!(
        snap(&l),
        vec![4, 28, 8, status::FAULT],
        "space 3 cannot see space 2's mapping"
    );
}

#[test]
fn ua_05_caller_must_be_the_loaded_space() {
    let mem: &'static SimMem = Box::leak(Box::new(SimMem::default()));
    let mut sps = Spaces::new();
    sps.create(2).unwrap();
    sps.create(3).unwrap();
    sps.get_mut(2)
        .unwrap()
        .map(0x10000, 0x1000, Perm::RW)
        .unwrap();
    mem.add(2, 0x10000, 0x1000, 1);
    let mut b = [0u8; 4];
    assert_eq!(sps.copy_from_user(mem, 2, 2, 0x10000, &mut b), Ok(()));
    assert_eq!(
        sps.copy_from_user(mem, 2, 3, 0x10000, &mut b),
        Err(UserError::SpaceMismatch)
    );
    assert_eq!(
        sps.copy_to_user(mem, 2, 3, 0x10000, &b),
        Err(UserError::SpaceMismatch)
    );
    assert_eq!(
        sps.copy_from_user(mem, 0, 0, 0x10000, &mut b),
        Err(UserError::NoSpace),
        "kernel space"
    );
    assert_eq!(
        sps.copy_from_user(mem, 9, 9, 0x10000, &mut b),
        Err(UserError::NoSpace)
    );
    assert_eq!(sps.create(2), Err(UserError::Exists));
    assert_eq!(sps.create(0), Err(UserError::BadRange));
}

#[test]
fn ua_06_length_cap() {
    let mem: &'static SimMem = Box::leak(Box::new(SimMem::default()));
    let mut sps = Spaces::new();
    sps.create(2).unwrap();
    sps.get_mut(2)
        .unwrap()
        .map(0x10000, 0x20_0000, Perm::RW)
        .unwrap();
    let mut big = vec![0u8; MAX_COPY + 1];
    assert_eq!(
        sps.copy_from_user(mem, 2, 2, 0x10000, &mut big),
        Err(UserError::TooLong)
    );
    assert_eq!(UserError::TooLong.status(), status::INVALID_ARGUMENT);
}

#[test]
fn ua_07_faulting_copy_writes_nothing() {
    let (k, mem) = kernel(true);
    mk_space(k, mem, 2, 0x10000, 0x1000, Perm::RW, 5);
    let l = log();
    let l1 = l.clone();
    spawn_fn(&k.sched, 2, move || {
        push(&l1, sc(k, SYS_DEBUG_FILL, [0x10FF8, 16, 0xEE, 0, 0, 0])); // straddles the end
    });
    run_all(&k.sched);
    assert_eq!(snap(&l), vec![status::FAULT]);
    assert_eq!(mem.peek(2, 0x10FF8, 8), vec![5; 8], "no partial write");
}

#[test]
fn ua_08_read_only_region_cannot_be_written() {
    let (k, mem) = kernel(true);
    mk_space(k, mem, 2, 0x10000, 0x1000, Perm::R, 3);
    let l = log();
    let l1 = l.clone();
    spawn_fn(&k.sched, 2, move || {
        push(&l1, sc(k, SYS_DEBUG_FILL, [0x10000, 4, 9, 0, 0, 0]));
        push(&l1, sc(k, SYS_DEBUG_CHECKSUM, [0x10000, 4, 0, 0, 0, 0]));
    });
    run_all(&k.sched);
    assert_eq!(snap(&l), vec![status::FAULT, 12]);
}

#[test]
fn ua_09_process_cleanup_kills_threads_and_destroys_the_space() {
    let (k, mem) = kernel(true);
    mk_space(k, mem, 2, 0x10000, 0x1000, Perm::RW, 1);
    let mut victims = Vec::new();
    for _ in 0..3 {
        victims.push(spawn_fn(&k.sched, 2, move || {
            loop {
                sc(k, SYS_SLEEP, [100, 0, 0, 0, 0, 0]);
            }
        }));
    }
    spawn_fn(&k.sched, KERNEL_SPACE, move || {
        assert_eq!(k.sched.terminate_space(2), Ok(3));
        assert!(k.spaces.lock().destroy(2));
    });
    assert_eq!(k.sched.run(ticking), RunExit::AllDone);
    for v in victims {
        assert_eq!(k.sched.exit_code_of(v), Some(EXIT_KILLED));
    }
    assert!(k.spaces.lock().get(2).is_none());
    assert_eq!(k.sched.live(), 0);
}

#[test]
fn ua_10_kernel_threads_get_no_user_access() {
    let (k, mem) = kernel(true);
    mk_space(k, mem, 2, 0x10000, 0x1000, Perm::RW, 1);
    let l = log();
    let l1 = l.clone();
    spawn_fn(&k.sched, KERNEL_SPACE, move || {
        push(&l1, sc(k, SYS_DEBUG_CHECKSUM, [0x10000, 4, 0, 0, 0, 0]));
    });
    run_all(&k.sched);
    assert_eq!(snap(&l), vec![status::ACCESS_DENIED]);
}

#[test]
fn ua_11_backing_failure_is_a_fault_and_zeroes_the_buffer() {
    let mut sps = Spaces::new();
    sps.create(2).unwrap();
    sps.get_mut(2)
        .unwrap()
        .map(0x10000, 0x1000, Perm::RW)
        .unwrap();
    let mut b = [0xFFu8; 8];
    assert_eq!(
        sps.copy_from_user(&FailingBacking, 2, 2, 0x10000, &mut b),
        Err(UserError::Fault)
    );
    assert_eq!(b, [0; 8]);
    assert_eq!(
        sps.copy_to_user(&FailingBacking, 2, 2, 0x10000, &b),
        Err(UserError::Fault)
    );
}

// ---- MSR: encodings (the init routine itself is never executed) ------------------------------

#[test]
fn msr_01_star_and_sysret_selectors() {
    assert_eq!(
        msr::star_value(msr::KERNEL_CS, msr::SYSRET_BASE),
        0x0010_0008_0000_0000
    );
    assert_eq!(
        msr::sysret_selectors(msr::SYSRET_BASE),
        (msr::USER_CS, msr::USER_SS)
    );
    assert_eq!((msr::USER_CS, msr::USER_SS), (0x23, 0x1B));
    assert_eq!(
        msr::KERNEL_SS,
        msr::KERNEL_CS + 8,
        "SYSCALL loads SS = CS + 8"
    );
}

#[test]
fn msr_02_gdt_descriptors() {
    assert_eq!(msr::GDT_KERNEL_CODE64, 0x00AF_9A00_0000_FFFF);
    assert_eq!(msr::GDT_KERNEL_DATA, 0x00CF_9200_0000_FFFF);
    assert_eq!(msr::GDT_USER_DATA, 0x00CF_F200_0000_FFFF);
    assert_eq!(msr::GDT_USER_CODE64, 0x00AF_FA00_0000_FFFF);
}

#[test]
fn msr_03_fmask_clears_dangerous_flags() {
    for bit in [0x100u64, 0x200, 0x400, 0x4000, 0x4_0000] {
        assert_ne!(msr::FMASK & bit, 0, "bit {bit:#x} must be cleared on entry");
    }
    assert_eq!(
        msr::FMASK & USER_RFLAGS_KEEP & !0x400,
        0,
        "only DF overlaps the user-visible set"
    );
}

#[test]
fn msr_04_cpu_reports_syscall() {
    assert!(msr::supports_syscall());
}

#[test]
fn msr_05_sysret_target_rule() {
    assert!(msr::sysret_target_ok(0x40_0000));
    assert!(msr::sysret_target_ok(USER_TOP - 1));
    assert!(!msr::sysret_target_ok(USER_TOP));
    assert!(!msr::sysret_target_ok(0xFFFF_FFFF_8000_0000));
    assert_eq!(
        USER_RFLAGS_KEEP,
        0x1 | 0x4 | 0x10 | 0x40 | 0x80 | 0x400 | 0x800
    );
}

#[test]
fn msr_06_msr_numbers() {
    assert_eq!(msr::IA32_EFER, 0xC000_0080);
    assert_eq!(msr::IA32_STAR, 0xC000_0081);
    assert_eq!(msr::IA32_LSTAR, 0xC000_0082);
    assert_eq!(msr::IA32_FMASK, 0xC000_0084);
    assert_eq!(msr::IA32_KERNEL_GS_BASE, 0xC000_0102);
}
