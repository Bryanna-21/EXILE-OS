use omni8::preempt::{Preempt, PreemptError};

#[test]
fn preempt_01_zero_quantum_is_refused() {
    assert!(matches!(Preempt::new(0), Err(PreemptError::ZeroQuantum)));
}

#[test]
fn preempt_02_request_is_raised_exactly_every_quantum_ticks() {
    let p = Preempt::new(3).unwrap();
    let raised: Vec<bool> = (0..9).map(|_| p.on_tick()).collect();
    assert_eq!(
        raised,
        [false, false, true, false, false, true, false, false, true]
    );
    assert_eq!(p.requests(), 3);
}

#[test]
fn preempt_03_quantum_of_one_requests_on_every_tick() {
    let p = Preempt::new(1).unwrap();
    assert!(p.on_tick() && p.on_tick() && p.on_tick());
    assert_eq!(p.requests(), 3);
}

#[test]
fn preempt_04_a_request_is_consumed_once() {
    let p = Preempt::new(2).unwrap();
    assert!(!p.take_request());
    p.on_tick();
    p.on_tick();
    assert!(p.take_request());
    assert!(!p.take_request());
    assert_eq!(p.honoured(), 1);
}

#[test]
fn preempt_05_disabled_regions_defer_but_do_not_lose_a_request() {
    let p = Preempt::new(1).unwrap();
    p.disable();
    p.on_tick();
    assert!(!p.take_request());
    assert!(p.is_disabled());
    p.enable().unwrap();
    assert!(p.take_request());
}

#[test]
fn preempt_06_disable_nests() {
    let p = Preempt::new(1).unwrap();
    p.disable();
    p.disable();
    p.on_tick();
    p.enable().unwrap();
    assert!(!p.take_request());
    p.enable().unwrap();
    assert!(p.take_request());
}

#[test]
fn preempt_07_unbalanced_enable_is_an_error_not_an_underflow() {
    let p = Preempt::new(1).unwrap();
    assert_eq!(p.enable(), Err(PreemptError::Unbalanced));
    assert!(!p.is_disabled());
}

#[test]
fn preempt_08_requests_raised_while_one_is_pending_coalesce_into_one_yield() {
    let p = Preempt::new(1).unwrap();
    p.on_tick();
    p.on_tick();
    assert_eq!(p.requests(), 2);
    assert!(p.take_request());
    assert!(!p.take_request());
    assert_eq!(p.honoured(), 1);
}
