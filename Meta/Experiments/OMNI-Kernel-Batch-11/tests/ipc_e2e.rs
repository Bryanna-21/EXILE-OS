//! IPC syscalls 0x100..0x107 end to end: real context switches, user memory behind real (software) page tables. HOSTED.
#[path = "../../OMNI-Kernel-Batch-9/tests/common/mod.rs"]
mod common;
use common::*;
use omni_kernel7::sched::SpaceId;
use omni_kernel7::syscall::SyscallFrame;
use omni10::abi::AbiMode;
use omni11::caps::*;
use omni11::status;
use omni11::sys::*;

// user memory layout (two pages per space at BASE)
const CREATE: u64 = BASE;
const SARGS: u64 = BASE + 0x100;
const MSG: u64 = BASE + 0x200;
const DESCS: u64 = BASE + 0x700;
const RBUF: u64 = BASE + 0x1000;
const RRES: u64 = BASE + 0x1800;
const RARGS: u64 = BASE + 0x1900;
const STATS: u64 = BASE + 0x1A00;

fn ik(w: &World) -> &'static IpcKernel<'static> {
    Box::leak(Box::new(IpcKernel::new(w.k, w.b)))
}

fn call(ik: &IpcKernel, nr: u64, a: [u64; 6]) -> i64 {
    let mut f = SyscallFrame {
        rax: nr,
        rdi: a[0],
        rsi: a[1],
        rdx: a[2],
        r10: a[3],
        r8: a[4],
        r9: a[5],
        rcx: 0x4000_0000,
        r11: 0x202,
        user_rsp: 0x7FFF_0000,
        ..Default::default()
    };
    ik.syscall(AbiMode::Omni, &mut f);
    f.rax as i64
}

fn poke(ik: &IpcKernel, sp: SpaceId, addr: u64, d: &[u8]) {
    let k = ik.abi.kernel;
    k.spaces
        .lock()
        .copy_to_user(ik_backing(ik), sp, k.sched.loaded_space(), addr, d)
        .unwrap();
}
fn peek(ik: &IpcKernel, sp: SpaceId, addr: u64, n: usize) -> Vec<u8> {
    let k = ik.abi.kernel;
    let mut v = vec![0u8; n];
    k.spaces
        .lock()
        .copy_from_user(ik_backing(ik), sp, k.sched.loaded_space(), addr, &mut v)
        .unwrap();
    v
}
fn ik_backing(_ik: &IpcKernel) -> &'static dyn omni_kernel7::uaccess::UserBacking {
    BACKING.with(|b| b.get()).expect("backing set")
}
thread_local! { static BACKING: std::cell::Cell<Option<&'static dyn omni_kernel7::uaccess::UserBacking>> = const { std::cell::Cell::new(None) }; }

fn header(op: u16, kind: u16, rid: u64, payload: &[u8], nh: u32) -> Vec<u8> {
    let mut m = vec![0u8; 32 + payload.len()];
    m[0..4].copy_from_slice(&((32 + payload.len()) as u32).to_le_bytes());
    m[4..6].copy_from_slice(&1u16.to_le_bytes());
    m[6..8].copy_from_slice(&op.to_le_bytes());
    m[12..14].copy_from_slice(&kind.to_le_bytes());
    m[16..24].copy_from_slice(&rid.to_le_bytes());
    m[24..28].copy_from_slice(&(payload.len() as u32).to_le_bytes());
    m[28..32].copy_from_slice(&nh.to_le_bytes());
    m[32..].copy_from_slice(payload);
    m
}

fn dsc(h: Handle, otype: u8, mode: u8, rights: Rights) -> [u8; 16] {
    let mut d = [0u8; 16];
    d[0..4].copy_from_slice(&h.0.to_le_bytes());
    d[4] = otype;
    d[5] = mode;
    d[8..12].copy_from_slice(&rights.0.to_le_bytes());
    d
}

fn send_args(msg_len: usize, nd: u32, timeout: u64) -> [u8; 48] {
    let mut a = [0u8; 48];
    a[0..8].copy_from_slice(&MSG.to_le_bytes());
    a[8..12].copy_from_slice(&(msg_len as u32).to_le_bytes());
    a[12..16].copy_from_slice(&nd.to_le_bytes());
    a[16..24].copy_from_slice(&DESCS.to_le_bytes());
    a[24..32].copy_from_slice(&timeout.to_le_bytes());
    a
}
fn recv_args(timeout: u64) -> [u8; 48] {
    let mut a = [0u8; 48];
    a[0..8].copy_from_slice(&RBUF.to_le_bytes());
    a[8..12].copy_from_slice(&1024u32.to_le_bytes());
    a[16..24].copy_from_slice(&RRES.to_le_bytes());
    a[24..32].copy_from_slice(&timeout.to_le_bytes());
    a
}

fn new_pair(ik: &IpcKernel, sp: SpaceId) -> (u32, u32) {
    assert_eq!(call(ik, SYS_ENDPOINT_CREATE, [CREATE, 0, 0, 0, 0, 0]), 0);
    let o = peek(ik, sp, CREATE, 8);
    (
        u32::from_le_bytes(o[0..4].try_into().unwrap()),
        u32::from_le_bytes(o[4..8].try_into().unwrap()),
    )
}

fn send(
    ik: &IpcKernel,
    sp: SpaceId,
    h: u32,
    op: u16,
    rid: u64,
    payload: &[u8],
    descs: &[[u8; 16]],
) -> i64 {
    let m = header(op, 1, rid, payload, descs.len() as u32);
    poke(ik, sp, MSG, &m);
    for (i, d) in descs.iter().enumerate() {
        poke(ik, sp, DESCS + 16 * i as u64, d);
    }
    poke(ik, sp, SARGS, &send_args(m.len(), descs.len() as u32, 0));
    call(ik, SYS_SEND, [h as u64, SARGS, 0, 0, 0, 0])
}

fn recv(ik: &IpcKernel, sp: SpaceId, h: u32, timeout: u64) -> (i64, Vec<u8>, Vec<u8>) {
    poke(ik, sp, RARGS, &recv_args(timeout));
    let r = call(ik, SYS_RECV, [h as u64, RARGS, 0, 0, 0, 0]);
    if r != 0 {
        return (r, vec![], vec![]);
    }
    let res = peek(ik, sp, RRES, 88);
    let n = u32::from_le_bytes(res[0..4].try_into().unwrap()) as usize;
    (0, peek(ik, sp, RBUF, n), res)
}

fn setup(spaces: &[SpaceId]) -> (World, &'static IpcKernel<'static>) {
    let w = world(false);
    for &s in spaces {
        w.space(s, &[Range::rw(BASE, 2, 0)]);
    }
    let i = ik(&w);
    (w, i)
}

macro_rules! thread {
    ($w:expr, $sp:expr, $b:expr) => {{
        let bk: &'static dyn omni_kernel7::uaccess::UserBacking = $w.b;
        spawn_fn(&$w.k.sched, $sp, move || {
            BACKING.with(|c| c.set(Some(bk)));
            $b
        })
    }};
}

#[test]
fn ipc_01_message_round_trip_between_two_spaces_with_a_transferred_endpoint() {
    let (w, i) = setup(&[2, 3]);
    let l = log();
    let (la, lb) = (l.clone(), l.clone());
    // space 2 makes a pair, hands one end... both threads need the same channel: create in 2, send an end over a
    // bootstrap channel created by 2 and "inherited" by 3 through the kernel state (test-only seeding).
    let (boot_a, boot_b) = {
        let mut st = i.state.lock();
        let (a, b) = st.create_pair(2).unwrap();
        let (_, ob) = st.resolve(2, b, ObjectType::Channel, Rights::NONE).unwrap();
        // move end b into space 3's table
        let cap = st.table_mut(2).unwrap().remove(b).unwrap();
        let _ = ob;
        let h3 = st.table_mut(3).unwrap().insert(cap).unwrap();
        (a.0, h3.0)
    };
    thread!(w, 2, {
        let (x, y) = new_pair(i, 2);
        // send x (move) and a copy of y with reduced rights across the bootstrap channel
        let rc = send(
            i,
            2,
            boot_a,
            7,
            99,
            b"hello",
            &[
                dsc(Handle(x), 1, 1, Rights::CHANNEL_DEFAULT),
                dsc(Handle(y), 1, 2, Rights::READ.union(Rights::TRANSFER)),
            ],
        );
        push(&la, rc);
        // moved handle is gone from our table
        push(&la, send(i, 2, x, 1, 1, b"", &[]));
        // reply arrives on y
        let (r, p, _) = recv(i, 2, y, 50);
        push(&la, r);
        push(&la, p.len() as i64);
    });
    thread!(w, 3, {
        let (r, p, res) = recv(i, 3, boot_b, 50);
        push(&lb, r);
        push(&lb, 1000 + p.len() as i64);
        push(
            &lb,
            2000 + u16::from_le_bytes(res[4..6].try_into().unwrap()) as i64,
        );
        push(
            &lb,
            3000 + u64::from_le_bytes(res[8..16].try_into().unwrap()) as i64,
        );
        push(
            &lb,
            4000 + u32::from_le_bytes(res[20..24].try_into().unwrap()) as i64,
        );
        let hx = u32::from_le_bytes(res[28..32].try_into().unwrap());
        let hy = u32::from_le_bytes(res[36..40].try_into().unwrap());
        // hy was copied READ|TRANSFER only: writing must be denied
        push(&lb, send(i, 3, hy, 1, 1, b"", &[]));
        // hx is the full end: its peer (x's partner is y in space 2)... x and y are a pair; send via hx reaches y
        push(&lb, send(i, 3, hx, 5, 2, b"reply", &[]));
    });
    w.k.sched.run(|s| {
        s.tick();
        false
    });
    let got = snap(&l);
    // order of pushes is deterministic under the cooperative scheduler; assert by content
    assert!(got.contains(&1005), "{got:?}");
    assert!(got.contains(&2007), "{got:?}");
    assert!(got.contains(&3099), "{got:?}");
    assert!(got.contains(&4002), "{got:?}");
    assert!(got.contains(&status::INVALID_HANDLE), "{got:?}"); // moved handle gone
    assert!(got.contains(&status::ACCESS_DENIED), "{got:?}"); // reduced-rights copy cannot write
    assert!(got.contains(&5), "reply payload length delivered: {got:?}");
}

#[test]
fn ipc_02_blocking_recv_times_out_and_wakes() {
    let (w, i) = setup(&[2, 3]);
    let (a, b) = {
        let mut st = i.state.lock();
        let (a, b) = st.create_pair(2).unwrap();
        let cap = st.table_mut(2).unwrap().remove(b).unwrap();
        (a.0, st.table_mut(3).unwrap().insert(cap).unwrap().0)
    };
    let l = log();
    let (l1, l2) = (l.clone(), l.clone());
    thread!(w, 2, {
        let (r, _, _) = recv(i, 2, a, 3); // nothing ever sent: times out
        push(&l1, r);
        let (r, p, _) = recv(i, 2, a, 100); // woken by a send
        push(&l1, r);
        push(&l1, p.len() as i64);
    });
    thread!(w, 3, {
        call(i, 0x1F2, [8, 0, 0, 0, 0, 0]); // SLEEP 8 ticks
        push(&l2, send(i, 3, b, 1, 1, b"abc", &[]));
    });
    let mut n = 0;
    w.k.sched.run(|s| {
        s.tick();
        n += 1;
        n < 200
    });
    let got = snap(&l);
    assert!(got.contains(&status::TIMED_OUT), "{got:?}");
    assert!(got.contains(&3), "{got:?}");
}

fn seed_pair(i: &IpcKernel, a: SpaceId, b: SpaceId) -> (u32, u32) {
    let mut st = i.state.lock();
    let (x, y) = st.create_pair(a).unwrap();
    let cap = st.table_mut(a).unwrap().remove(y).unwrap();
    (x.0, st.table_mut(b).unwrap().insert(cap).unwrap().0)
}

#[test]
fn ipc_03_malformed_arguments_get_defined_errors() {
    let (w, i) = setup(&[2]);
    let l = log();
    let l1 = l.clone();
    thread!(w, 2, {
        let (a, _b) = new_pair(i, 2);
        let a64 = a as u64;
        // unused argument registers must be zero
        push(&l1, call(i, SYS_HANDLE_RELEASE, [a64, 1, 0, 0, 0, 0]));
        // handles wider than 32 bits are rejected, not truncated
        push(
            &l1,
            call(i, SYS_ENDPOINT_CLOSE, [(1u64 << 32) | a64, 0, 0, 0, 0, 0]),
        );
        // cancellation objects are not implemented
        push(&l1, call(i, SYS_CANCEL_CREATE, [0; 6]));
        push(&l1, call(i, SYS_CANCEL_SIGNAL, [0; 6]));
        // bad user pointers
        push(&l1, call(i, SYS_ENDPOINT_CREATE, [0, 0, 0, 0, 0, 0]));
        push(
            &l1,
            call(
                i,
                SYS_ENDPOINT_CREATE,
                [0xFFFF_FFFF_0000_0000, 0, 0, 0, 0, 0],
            ),
        );
        push(&l1, call(i, SYS_SEND, [a64, 0x1000, 0, 0, 0, 0]));
        // wrong header version
        let mut m = header(1, 1, 1, b"", 0);
        m[4] = 9;
        poke(i, 2, MSG, &m);
        poke(i, 2, SARGS, &send_args(32, 0, 0));
        push(&l1, call(i, SYS_SEND, [a64, SARGS, 0, 0, 0, 0]));
        // oversized length / count
        poke(i, 2, SARGS, &send_args(4096, 0, 0));
        push(&l1, call(i, SYS_SEND, [a64, SARGS, 0, 0, 0, 0]));
        poke(i, 2, SARGS, &send_args(32, 9, 0));
        push(&l1, call(i, SYS_SEND, [a64, SARGS, 0, 0, 0, 0]));
        // descriptor count disagreeing with the header
        let m = header(1, 1, 1, b"", 0);
        poke(i, 2, MSG, &m);
        poke(i, 2, SARGS, &send_args(32, 1, 0));
        push(&l1, call(i, SYS_SEND, [a64, SARGS, 0, 0, 0, 0]));
        // blocking send is not supported
        poke(i, 2, SARGS, &send_args(32, 0, 5));
        push(&l1, call(i, SYS_SEND, [a64, SARGS, 0, 0, 0, 0]));
        // recv: forever and over-long waits, short buffer
        push(&l1, recv(i, 2, a, u64::MAX).0);
        push(&l1, recv(i, 2, a, 1 << 30).0);
        // unknown handle and a number just outside the range
        push(
            &l1,
            call(i, SYS_ENDPOINT_STATS, [0x7777, STATS, 0, 0, 0, 0]),
        );
        push(&l1, call(i, 0x108, [0; 6]));
    });
    w.k.sched.run(|_| false);
    use status::*;
    assert_eq!(
        snap(&l),
        vec![
            INVALID_ARGUMENT,
            INVALID_ARGUMENT,
            NOT_SUPPORTED,
            NOT_SUPPORTED,
            FAULT,
            FAULT,
            FAULT,
            BAD_VERSION,
            INVALID_ARGUMENT,
            INVALID_ARGUMENT,
            INVALID_ARGUMENT,
            NOT_SUPPORTED,
            INVALID_ARGUMENT,
            INVALID_ARGUMENT,
            INVALID_HANDLE,
            NO_SYS
        ]
    );
}

#[test]
fn ipc_04_queue_full_peer_death_and_stats() {
    let (w, i) = setup(&[2, 3]);
    let (a, b) = seed_pair(i, 2, 3);
    let l = log();
    let (l1, l2) = (l.clone(), l.clone());
    thread!(w, 2, {
        for n in 0..5 {
            push(&l1, send(i, 2, a, 1, n, b"x", &[]));
        }
        let r = call(i, SYS_ENDPOINT_STATS, [a as u64, STATS, 0, 0, 0, 0]);
        push(&l1, r);
        // stats of the *peer's* queue are reported on the peer's handle; ours is empty
        let s = peek(i, 2, STATS, 32);
        push(&l1, u32::from_le_bytes(s[0..4].try_into().unwrap()) as i64);
        call(i, 0x1F0, [0; 6]); // yield: let space 3 drain and close
        push(&l1, send(i, 2, a, 1, 9, b"x", &[]));
    });
    thread!(w, 3, {
        let s = call(i, SYS_ENDPOINT_STATS, [b as u64, STATS, 0, 0, 0, 0]);
        push(&l2, s);
        let st = peek(i, 3, STATS, 32);
        push(
            &l2,
            100 + u32::from_le_bytes(st[0..4].try_into().unwrap()) as i64,
        );
        for _ in 0..4 {
            push(&l2, recv(i, 3, b, 0).0);
        }
        push(&l2, recv(i, 3, b, 0).0); // empty, peer alive
        push(&l2, call(i, SYS_HANDLE_RELEASE, [b as u64, 0, 0, 0, 0, 0]));
        push(&l2, call(i, SYS_HANDLE_RELEASE, [b as u64, 0, 0, 0, 0, 0])); // stale
    });
    w.k.sched.run(|_| false);
    let got = snap(&l);
    assert_eq!(&got[..5], &[0, 0, 0, 0, status::WOULD_BLOCK], "{got:?}");
    assert!(
        got.contains(&104),
        "receiver saw four queued messages: {got:?}"
    );
    assert!(got.contains(&status::WOULD_BLOCK));
    assert!(
        got.contains(&status::PEER_DEAD),
        "send after the peer released its end: {got:?}"
    );
    assert!(
        got.contains(&status::INVALID_HANDLE),
        "double release: {got:?}"
    );
}

#[test]
fn ipc_05_terminate_space_wakes_waiters_and_frees_every_capability() {
    let (w, i) = setup(&[2, 3]);
    let (a, b) = seed_pair(i, 2, 3);
    let l = log();
    let (l1, l2) = (l.clone(), l.clone());
    thread!(w, 3, {
        // blocks forever (long timeout) until its space is terminated
        let (r, _, _) = recv(i, 3, b, 1000);
        push(&l2, r); // never reached: the thread is killed
    });
    thread!(w, 2, {
        call(i, 0x1F2, [2, 0, 0, 0, 0, 0]); // sleep: let the receiver block
        push(&l1, i.state.lock().live_objects() as i64);
        let r = i.terminate_space(3).map(|n| n as i64).unwrap_or(-99);
        push(&l1, r);
        push(&l1, i.state.lock().live_objects() as i64);
        push(&l1, recv(i, 2, a, 0).0);
    });
    let mut n = 0;
    w.k.sched.run(|s| {
        s.tick();
        n += 1;
        n < 100
    });
    let got = snap(&l);
    assert_eq!(got[0], 2, "{got:?}");
    assert_eq!(got[1], 1, "one thread terminated: {got:?}");
    assert_eq!(
        got[2], 1,
        "the terminated space's end is destroyed; space 2 keeps its own: {got:?}"
    );
    assert_eq!(got[3], status::PEER_DEAD, "{got:?}");
    assert!(snap(&l).len() == 4);
    i.cleanup_space(2);
    assert_eq!(
        i.state.lock().live_objects(),
        0,
        "everything is reclaimed when the last holder goes"
    );
}

#[test]
fn ipc_06_unreadable_output_buffers_do_not_consume_messages_or_leak_handles() {
    let (w, i) = setup(&[2]);
    let l = log();
    let l1 = l.clone();
    thread!(w, 2, {
        let (a, b) = new_pair(i, 2);
        push(&l1, send(i, 2, a, 1, 1, b"hi", &[]));
        // result pointer outside any mapping: probed before the message is taken
        let mut ra = recv_args(0);
        ra[16..24].copy_from_slice(&0x1000u64.to_le_bytes());
        poke(i, 2, RARGS, &ra);
        push(&l1, call(i, SYS_RECV, [b as u64, RARGS, 0, 0, 0, 0]));
        let (r, p, _) = recv(i, 2, b, 0);
        push(&l1, r);
        push(&l1, p.len() as i64);
        // an endpoint_create whose output cannot be written leaves no handles behind
        let before = i.state.lock().table(2).map(|t| t.len()).unwrap();
        push(&l1, call(i, SYS_ENDPOINT_CREATE, [0x1000, 0, 0, 0, 0, 0]));
        push(
            &l1,
            (i.state.lock().table(2).map(|t| t.len()).unwrap() == before) as i64,
        );
    });
    w.k.sched.run(|_| false);
    assert_eq!(snap(&l), vec![0, status::FAULT, 0, 2, status::FAULT, 1]);
}

#[test]
fn ipc_07_deterministic_fuzz_never_panics_and_stays_inside_the_status_set() {
    let (w, i) = setup(&[2, 3]);
    let (a, b) = seed_pair(i, 2, 3);
    let bad = log();
    let bl = bad.clone();
    thread!(w, 2, {
        let mut x: u64 = 0x9E37_79B9_7F4A_7C15;
        let mut rnd = move || {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            x
        };
        for round in 0..400u32 {
            let len = 32 + (rnd() % 96) as usize;
            let mut m: Vec<u8> = (0..len).map(|_| rnd() as u8).collect();
            if round % 2 == 0 {
                // plausible header, random corruption of a few bytes
                m = header(
                    1 + (rnd() % 3) as u16,
                    1 + (rnd() % 3) as u16,
                    rnd(),
                    &m[32..],
                    0,
                );
                for _ in 0..(rnd() % 3) {
                    let k = (rnd() as usize) % m.len();
                    m[k] ^= 1 << (rnd() % 8);
                }
            }
            poke(i, 2, MSG, &m);
            let nd = (rnd() % 3) as u32;
            for d in 0..nd as u64 {
                let mut raw = [0u8; 16];
                raw.iter_mut().for_each(|b| *b = rnd() as u8);
                poke(i, 2, DESCS + 16 * d, &raw);
            }
            poke(i, 2, SARGS, &send_args(m.len(), nd, 0));
            let r = call(
                i,
                SYS_SEND,
                [
                    if round % 7 == 0 {
                        rnd() & 0xFFFF
                    } else {
                        a as u64
                    },
                    SARGS,
                    0,
                    0,
                    0,
                    0,
                ],
            );
            push(&bl, r);
        }
    });
    w.k.sched.run(|_| false);
    let allowed = [
        0,
        status::INVALID_ARGUMENT,
        status::INVALID_HANDLE,
        status::ACCESS_DENIED,
        status::WOULD_BLOCK,
        status::BAD_VERSION,
        status::PEER_DEAD,
        status::NOT_SUPPORTED,
        status::QUOTA_EXCEEDED,
        status::FAULT,
    ];
    let got = snap(&bad);
    assert_eq!(got.len(), 400);
    assert!(
        got.iter().all(|r| allowed.contains(r)),
        "{:?}",
        got.iter()
            .filter(|r| !allowed.contains(r))
            .collect::<Vec<_>>()
    );
    // queue invariant: no more than the bound is ever held
    let st = i.state.lock();
    let _ = b;
    assert!(st.live_objects() <= 32);
}

#[test]
fn ipc_08_ipc_numbers_pass_through_to_the_base_syscalls() {
    let (w, i) = setup(&[2]);
    let l = log();
    let l1 = l.clone();
    thread!(w, 2, {
        push(&l1, call(i, 0x1F4, [0; 6])); // SPACE_SELF
        push(&l1, call(i, 0x1F8, [0, 0, 0, 0, 0, 0])); // ABI_QUERY
        push(&l1, call(i, 0x2FF, [0; 6]));
    });
    w.k.sched.run(|_| false);
    let g = snap(&l);
    assert_eq!(g[0], 2);
    assert_eq!(g[1], 0x1_0000);
    assert_eq!(g[2], status::NO_SYS);
}
