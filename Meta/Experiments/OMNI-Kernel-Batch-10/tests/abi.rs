//! SEC-18 and SEC-20: syscall version query and register-convention adaptor. HOSTED.

#[path = "../../OMNI-Kernel-Batch-9/tests/common/mod.rs"]
mod common;
use common::*;
use omni_kernel7::status;
use omni_kernel7::syscall::*;
use omni_kernel7::uaccess::USER_TOP;
use omni10::abi::*;

fn frame(nr: u64) -> SyscallFrame {
    SyscallFrame {
        rax: nr,
        rcx: 0x4000_0000,
        r11: 0x202,
        user_rsp: 0x7FFF_0000,
        ..Default::default()
    }
}

#[test]
fn abi_01_query_returns_the_version_and_enforces_the_major() {
    let w = world(false);
    w.space(2, &[Range::rw(BASE, 1, 0)]);
    let (k, l) = (w.k, log());
    let l2 = l.clone();
    spawn_fn(&k.sched, 2, move || {
        let ak = AbiKernel::new(k);
        for (want, flags) in [
            (0u64, 0u64),
            (ABI_MAJOR as u64, 0),
            (ABI_MAJOR as u64 + 1, 0),
            (0, 1),
            (u64::MAX, 0),
        ] {
            let mut f = frame(SYS_ABI_QUERY);
            f.rdi = want;
            f.rsi = flags;
            ak.syscall(AbiMode::Omni, &mut f);
            push(&l2, f.rax as i64);
        }
    });
    run_all(&k.sched);
    let v = version_word() as i64;
    let bad = status::INVALID_ARGUMENT;
    assert_eq!(snap(&l), vec![v, v, bad, bad, bad]);
    assert_eq!(v, 0x1_0000);
}

#[test]
fn abi_02_exile_convention_maps_arguments_and_matches_omni() {
    let w = world(true); // SYS_DEBUG_ARGS is a pure function of its six arguments
    w.space(2, &[Range::rw(BASE, 1, 0)]);
    let (k, l) = (w.k, log());
    let l2 = l.clone();
    spawn_fn(&k.sched, 2, move || {
        let ak = AbiKernel::new(k);
        // Exile: rax number; rdx, rdi, rbx, rsi = arg1..arg4
        let mut f = frame(SYS_DEBUG_ARGS);
        f.rdx = 11;
        f.rdi = 22;
        f.rbx = 33;
        f.rsi = 44;
        ak.syscall(AbiMode::Exile, &mut f);
        push(&l2, f.rax as i64);
        // Omni: same four values in rdi, rsi, rdx, r10, then r8 = r9 = 0
        let mut g = frame(SYS_DEBUG_ARGS);
        g.rdi = 11;
        g.rsi = 22;
        g.rdx = 33;
        g.r10 = 44;
        ak.syscall(AbiMode::Omni, &mut g);
        push(&l2, g.rax as i64);
    });
    run_all(&k.sched);
    let s = snap(&l);
    assert_eq!(s[0], s[1]);
    assert_eq!(s[0], 11 + 22 * 2 + 33 * 3 + 44 * 4);
}

#[test]
fn abi_03_exile_mode_leaves_the_users_argument_registers_alone() {
    let w = world(true);
    w.space(2, &[Range::rw(BASE, 1, 0)]);
    let (k, l) = (w.k, log());
    let l2 = l.clone();
    spawn_fn(&k.sched, 2, move || {
        let ak = AbiKernel::new(k);
        let mut f = frame(SYS_DEBUG_ARGS);
        (f.rdx, f.rdi, f.rbx, f.rsi, f.r10, f.r8, f.r9, f.rbp) = (1, 2, 3, 4, 5, 6, 7, 8);
        let before = f;
        ak.syscall(AbiMode::Exile, &mut f);
        let same = (
            f.rdx, f.rdi, f.rbx, f.rsi, f.r10, f.r8, f.r9, f.rbp, f.rcx, f.user_rsp,
        ) == (
            before.rdx,
            before.rdi,
            before.rbx,
            before.rsi,
            before.r10,
            before.r8,
            before.r9,
            before.rbp,
            before.rcx,
            before.user_rsp,
        );
        push(&l2, same as i64);
    });
    run_all(&k.sched);
    assert_eq!(snap(&l), vec![1]);
}

#[test]
fn abi_04_query_outside_thread_context_is_invalid_state() {
    let w = world(false);
    let mut f = frame(SYS_ABI_QUERY);
    AbiKernel::new(w.k).syscall(AbiMode::Omni, &mut f);
    assert_eq!(f.rax as i64, status::INVALID_STATE);
}

#[test]
fn abi_05_query_with_a_frame_returning_into_the_kernel_half_kills_the_thread() {
    let w = world(false);
    w.space(2, &[Range::rw(BASE, 1, 0)]);
    let (k, l) = (w.k, log());
    let l2 = l.clone();
    let t = spawn_fn(&k.sched, 2, move || {
        let mut f = frame(SYS_ABI_QUERY);
        f.rcx = USER_TOP; // would sysret into non-user address
        AbiKernel::new(k).syscall(AbiMode::Omni, &mut f);
        push(&l2, 1); // must never run
    });
    run_all(&k.sched);
    assert!(snap(&l).is_empty(), "thread returned with a bad frame");
    assert_eq!(k.sched.exit_code_of(t), Some(EXIT_BAD_FRAME));
}

#[test]
fn abi_06_query_number_is_free_and_other_syscalls_pass_through_unchanged() {
    use omni_kernel7::syscall::{SYS_DEBUG_ARGS, SYS_SPACE_SELF, SYS_THREAD_SELF};
    // free of Batch 7 (0x1F0..=0x1F7) and Batch 5 IPC (0x100..=0x107)
    assert!(SYS_ABI_QUERY > SYS_DEBUG_ARGS && !(0x100..=0x107).contains(&SYS_ABI_QUERY));
    // the bare Kernel still refuses it: the layer is the only place that answers
    let w = world(false);
    w.space(2, &[Range::rw(BASE, 1, 0)]);
    let (k, l) = (w.k, log());
    let l2 = l.clone();
    spawn_fn(&k.sched, 2, move || {
        push(&l2, sc(k, SYS_ABI_QUERY, [0; 6]));
        let ak = AbiKernel::new(k);
        for nr in [SYS_THREAD_SELF, SYS_SPACE_SELF] {
            let mut f = frame(nr);
            ak.syscall(AbiMode::Omni, &mut f);
            push(&l2, f.rax as i64 - sc(k, nr, [0; 6]));
        }
    });
    run_all(&k.sched);
    assert_eq!(snap(&l), vec![status::NO_SYS, 0, 0]);
}

#[test]
fn abi_07_to_omni_is_pure_identity_for_omni_and_a_fixed_permutation_for_exile() {
    let mut f = frame(7);
    (f.rdi, f.rsi, f.rdx, f.r10, f.r8, f.r9, f.rbx) = (1, 2, 3, 4, 5, 6, 9);
    let g = to_omni(AbiMode::Omni, &f);
    assert_eq!(
        (g.rdi, g.rsi, g.rdx, g.r10, g.r8, g.r9, g.rbx),
        (1, 2, 3, 4, 5, 6, 9)
    );
    let mut e = frame(7);
    (e.rdx, e.rdi, e.rbx, e.rsi) = (10, 20, 30, 40);
    let h = to_omni(AbiMode::Exile, &e);
    assert_eq!(h.args(), [10, 20, 30, 40, 0, 0]);
    assert_eq!(h.rax, 7);
}
