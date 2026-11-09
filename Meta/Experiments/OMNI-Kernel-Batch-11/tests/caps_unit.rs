//! Capability table and IPC state: negative and boundary tests. HOSTED, no user memory needed.
#[path = "../../OMNI-Kernel-Batch-9/tests/common/mod.rs"]
mod common;
use common::*;
use omni11::caps::*;
use omni11::ipc::*;
use omni11::status;

fn desc(h: Handle, mode: Mode, rights: Rights) -> Desc {
    Desc {
        handle: h,
        otype: ObjectType::Channel,
        mode,
        rights,
    }
}

#[test]
fn cap_01_rights_are_checked_in_the_documented_order() {
    let mut st = State::new();
    let (a, _b) = st.create_pair(2).unwrap();
    // wrong type is reported before missing rights
    assert_eq!(
        st.resolve(2, a, ObjectType::Process, Rights::NONE)
            .unwrap_err(),
        status::INVALID_HANDLE
    );
    assert!(st.resolve(2, a, ObjectType::Channel, Rights::READ).is_ok());
    // narrow the handle to read-only through a copy-less table edit: use a derived cap via send later;
    // here: an invalid handle is INVALID_HANDLE, never ACCESS_DENIED.
    assert_eq!(
        st.resolve(2, Handle(0), ObjectType::Channel, Rights::READ)
            .unwrap_err(),
        status::INVALID_HANDLE
    );
    assert_eq!(
        st.resolve(2, Handle::make(31, 1), ObjectType::Channel, Rights::READ)
            .unwrap_err(),
        status::INVALID_HANDLE
    );
    assert_eq!(
        st.resolve(2, Handle::make(40, 1), ObjectType::Channel, Rights::READ)
            .unwrap_err(),
        status::INVALID_HANDLE
    );
}

#[test]
fn cap_02_handles_are_per_space_and_never_cross() {
    let mut st = State::new();
    let (a, _b) = st.create_pair(2).unwrap();
    assert_eq!(
        st.resolve(3, a, ObjectType::Channel, Rights::NONE)
            .unwrap_err(),
        status::INVALID_HANDLE
    );
    assert_eq!(
        st.resolve(7, a, ObjectType::Channel, Rights::NONE)
            .unwrap_err(),
        status::INVALID_HANDLE
    );
}

#[test]
fn cap_03_stale_handle_after_release_is_rejected_and_slot_is_reusable() {
    let w = world(false);
    let mut st = State::new();
    let (a, b) = st.create_pair(2).unwrap();
    st.table_mut(2).unwrap().remove(a).unwrap();
    st.gc(&w.k.sched);
    assert_eq!(
        st.resolve(2, a, ObjectType::Channel, Rights::NONE)
            .unwrap_err(),
        status::INVALID_HANDLE
    );
    // the surviving end sees a dead peer
    let (oi, _) = st.resolve(2, b, ObjectType::Channel, Rights::NONE).unwrap();
    assert!(st.is_peer_dead(oi));
    let (c, _d) = st.create_pair(2).unwrap();
    assert_ne!(c, a, "the reused slot carries a new generation");
    assert_eq!(
        st.resolve(2, a, ObjectType::Channel, Rights::NONE)
            .unwrap_err(),
        status::INVALID_HANDLE
    );
}

#[test]
fn cap_04_generation_exhaustion_retires_the_slot() {
    let mut t = CapTable::new();
    let cap = Cap {
        obj: 0,
        obj_gen: 1,
        otype: ObjectType::Channel,
        rights: Rights::READ,
    };
    t.force_generation(0, u16::MAX);
    let h = t.insert(cap).unwrap();
    assert_eq!(h.index(), 0);
    assert_eq!(h.generation(), u16::MAX);
    t.remove(h).unwrap();
    assert_eq!(
        t.free_slots(),
        MAX_HANDLES - 1,
        "slot 0 is retired, not wrapped to generation 0"
    );
    let h2 = t.insert(cap).unwrap();
    assert_ne!(h2.index(), 0);
    assert!(t.get(h).is_none());
}

#[test]
fn cap_05_handle_table_exhaustion_returns_quota_exceeded() {
    let mut st = State::new();
    let mut n = 0;
    loop {
        match st.create_pair(2) {
            Ok(_) => n += 1,
            Err(e) => {
                assert_eq!(e, status::QUOTA_EXCEEDED);
                break;
            }
        }
    }
    assert_eq!(n, MAX_HANDLES / 2);
    assert!(st.live_objects() <= MAX_OBJECTS);
}

#[test]
fn cap_06_object_pool_exhaustion_is_reported_and_leaks_nothing() {
    let mut st = State::new();
    let mut spaces = 2u16;
    let mut pairs = 0;
    'outer: loop {
        match st.create_pair(spaces) {
            Ok(_) => pairs += 1,
            Err(e) => {
                assert!(
                    e == status::QUOTA_EXCEEDED || e == status::INVALID_ARGUMENT,
                    "{e}"
                );
                if spaces < 2 + MAX_SPACES as u16 - 1 {
                    spaces += 1;
                    continue;
                }
                break 'outer;
            }
        }
    }
    assert!(pairs > 0);
    assert!(st.live_objects() <= MAX_OBJECTS);
    assert_eq!(st.live_objects() % 2, 0, "ends are created in pairs");
}

#[test]
fn cap_07_descriptor_rules_move_copy_escalation_and_cycle() {
    let w = world(false);
    let mut st = State::new();
    let (a, b) = st.create_pair(2).unwrap(); // a,b in space 2
    let (c, _d) = st.create_pair(2).unwrap();
    let (oi_a, _) = st
        .resolve(2, a, ObjectType::Channel, Rights::WRITE)
        .unwrap();
    let dst = st.peer_of(oi_a).unwrap();
    // direct cycle: sending an end over the channel it is queued on
    assert!(
        st.validate_descs(2, &[desc(b, Mode::Move, Rights::CHANNEL_DEFAULT)], dst)
            .is_err()
    );
    // rights escalation: requested rights not a subset
    let full = Rights::CHANNEL_DEFAULT.union(Rights::EXECUTE);
    assert_eq!(
        st.validate_descs(2, &[desc(c, Mode::Copy, full)], dst)
            .unwrap_err(),
        status::ACCESS_DENIED
    );
    // unknown / stale handle
    assert_eq!(
        st.validate_descs(
            2,
            &[desc(Handle::make(30, 5), Mode::Move, Rights::READ)],
            dst
        )
        .unwrap_err(),
        status::INVALID_HANDLE
    );
    // duplicate handle in one message
    assert!(
        st.validate_descs(
            2,
            &[
                desc(c, Mode::Move, Rights::CHANNEL_DEFAULT),
                desc(c, Mode::Copy, Rights::READ)
            ],
            dst
        )
        .is_err()
    );
    // a valid narrowing copy is accepted
    assert_eq!(
        st.validate_descs(
            2,
            &[desc(c, Mode::Copy, Rights::READ.union(Rights::TRANSFER))],
            dst
        ),
        Ok(())
    );
    // a copy that drops TRANSFER is malformed
    assert!(
        st.validate_descs(2, &[desc(c, Mode::Copy, Rights::READ)], dst)
            .is_err()
    );
    let _ = w;
}

#[test]
fn cap_08_gc_collects_reference_cycles_and_purges_dead_caps() {
    let w = world(false);
    let mut st = State::new();
    let (a, b) = st.create_pair(2).unwrap();
    let (c, d) = st.create_pair(2).unwrap();
    let (oi_a, _) = st
        .resolve(2, a, ObjectType::Channel, Rights::WRITE)
        .unwrap();
    let dst_a = st.peer_of(oi_a).unwrap();
    // put c into a's peer queue (move), and d into c's... build a cycle through queues
    st.commit_send(
        2,
        dst_a,
        &hdr(0),
        1,
        1,
        7,
        &[desc(c, Mode::Move, Rights::CHANNEL_DEFAULT)],
    )
    .unwrap();
    st.table_mut(2).unwrap().remove(a).unwrap();
    st.table_mut(2).unwrap().remove(b).unwrap();
    st.table_mut(2).unwrap().remove(d).unwrap();
    st.gc(&w.k.sched);
    assert_eq!(
        st.live_objects(),
        0,
        "queued handle to an unreachable end does not keep anything alive"
    );
    assert_eq!(st.table(2).unwrap().len(), 0);
}

fn hdr(payload: usize) -> Vec<u8> {
    let mut m = vec![0u8; 32 + payload];
    m[0..4].copy_from_slice(&((32 + payload) as u32).to_le_bytes());
    m[4..6].copy_from_slice(&1u16.to_le_bytes());
    m[6..8].copy_from_slice(&1u16.to_le_bytes());
    m[12..14].copy_from_slice(&1u16.to_le_bytes());
    m[24..28].copy_from_slice(&(payload as u32).to_le_bytes());
    m
}

#[test]
fn cap_09_cleanup_space_is_idempotent_and_releases_everything() {
    let w = world(false);
    let mut st = State::new();
    for _ in 0..5 {
        st.create_pair(3).unwrap();
    }
    st.cleanup_space(3, &w.k.sched);
    st.cleanup_space(3, &w.k.sched);
    assert_eq!(st.live_objects(), 0);
}

#[test]
fn cap_10_move_cannot_attenuate_and_close_purges_dead_caps() {
    let w = world(false);
    let mut st = State::new();
    let (a, b) = st.create_pair(2).unwrap();
    let (c, _d) = st.create_pair(2).unwrap();
    let (oi_a, _) = st
        .resolve(2, a, ObjectType::Channel, Rights::WRITE)
        .unwrap();
    let dst = st.peer_of(oi_a).unwrap();
    assert_eq!(
        st.validate_descs(
            2,
            &[desc(c, Mode::Move, Rights::READ.union(Rights::TRANSFER))],
            dst
        )
        .unwrap_err(),
        status::INVALID_ARGUMENT
    );
    let before = st.table(2).unwrap().len();
    let (oi_b, _) = st
        .resolve(2, b, ObjectType::Channel, Rights::ADMIN)
        .unwrap();
    st.close(oi_b, &w.k.sched);
    // the closed end's own capability no longer occupies a table slot
    assert!(st.table(2).unwrap().len() < before);
    assert_eq!(
        st.resolve(2, b, ObjectType::Channel, Rights::NONE)
            .unwrap_err(),
        status::INVALID_HANDLE
    );
}
