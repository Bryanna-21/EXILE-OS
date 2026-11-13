//! Tests for the shared self-test logic, including running the scheduler scenario that the
//! booted kernel runs, here on the host with the same Batch 7 scheduler.

use omni_kernel7::sched::Sched;
use omni8::selftest::*;
use std::boxed::Box;

fn obs(vector: u64, error: u64, cr2: u64) -> Option<Observation> {
    Some(Observation {
        vector,
        error,
        cr2,
        from_user: false,
    })
}

#[test]
fn st_01_expectation_checking() {
    let e = Expectation {
        vector: 14,
        error: Some(2),
        cr2: Some(0x1000),
    };
    assert_eq!(check(obs(14, 2, 0x1000), e), Ok(()));
    assert_eq!(check(None, e), Err(Mismatch::NoTrap));
    assert_eq!(check(obs(13, 2, 0x1000), e), Err(Mismatch::Vector));
    assert_eq!(check(obs(14, 3, 0x1000), e), Err(Mismatch::ErrorCode));
    assert_eq!(check(obs(14, 2, 0x2000), e), Err(Mismatch::FaultAddress));
    let user = Some(Observation {
        vector: 14,
        error: 2,
        cr2: 0x1000,
        from_user: true,
    });
    assert_eq!(check(user, e), Err(Mismatch::WrongRing));
    let any = Expectation {
        vector: 3,
        error: None,
        cr2: None,
    };
    assert_eq!(check(obs(3, 0xFFFF, 0x1234), any), Ok(()));
}

fn big_stack<R: Send + 'static>(f: impl FnOnce() -> R + Send + 'static) -> R {
    std::thread::Builder::new()
        .stack_size(16 << 20)
        .spawn(f)
        .unwrap()
        .join()
        .unwrap()
}

#[test]
fn st_02_scheduler_scenario_runs_on_the_batch7_scheduler() {
    // The booted kernel runs exactly this function. Here it runs on the host, where Batch 7's
    // real context-switch assembly switches real stacks.
    let addr = big_stack(|| Box::into_raw(Box::new(Sched::new())) as usize);
    let sched: &'static Sched = unsafe { &*(addr as *const Sched) };
    let log = Box::leak(Box::new(OrderLog::new()));
    assert_eq!(run_order_scenario(sched, log), Ok(()));
    assert_eq!(log.snapshot(), EXPECTED_ORDER);
    assert_eq!(log.len(), 9);
    assert_eq!(sched.stats().switches, EXPECTED_SWITCHES);
    assert_eq!(EXPECTED_SWITCHES, 12);
    // the scenario can be repeated on the same scheduler (slots are reclaimed)
    let log2 = Box::leak(Box::new(OrderLog::new()));
    assert_eq!(run_order_scenario(sched, log2), Ok(()));
    assert_eq!(sched.stats().switches, 2 * EXPECTED_SWITCHES);
}

#[test]
fn st_03_scenario_detects_a_wrong_order() {
    // a log that already holds entries cannot match the expected order
    let addr = big_stack(|| Box::into_raw(Box::new(Sched::new())) as usize);
    let sched: &'static Sched = unsafe { &*(addr as *const Sched) };
    let log = Box::leak(Box::new(OrderLog::new()));
    // pre-fill by running once, then run again against the same (full) log
    assert_eq!(run_order_scenario(sched, log), Ok(()));
    assert_eq!(run_order_scenario(sched, log), Err(ScenarioError::Order));
    assert!(OrderLog::new().is_empty());
}

fn new_sched() -> &'static Sched {
    let addr = big_stack(|| Box::into_raw(Box::new(Sched::new())) as usize);
    unsafe { &*(addr as *const Sched) }
}

#[test]
fn st_10_tick_driven_preemption_interleaves_two_threads_on_the_real_scheduler() {
    use omni8::preempt::Preempt;
    let sched = new_sched();
    let preempt = Box::leak(Box::new(Preempt::new(PREEMPT_QUANTUM).unwrap()));
    let log = Box::leak(Box::new(PreemptLog::new()));
    assert_eq!(run_preempt_scenario(sched, preempt, log), Ok(()));
    assert_eq!(log.snapshot(), PREEMPT_EXPECTED_ORDER);
    assert_eq!(preempt.requests(), 6);
    assert_eq!(preempt.honoured(), 6);
    assert_eq!(sched.stats().switches, PREEMPT_EXPECTED_SWITCHES);
    assert_eq!(PREEMPT_EXPECTED_SWITCHES, 8);
}

#[test]
fn st_11_a_different_quantum_changes_the_interleaving() {
    use omni8::preempt::Preempt;
    let sched = new_sched();
    let preempt = Box::leak(Box::new(Preempt::new(4).unwrap()));
    let log = Box::leak(Box::new(PreemptLog::new()));
    assert_eq!(
        run_preempt_scenario(sched, preempt, log),
        Err(PreemptScenarioError::Order)
    );
}

#[test]
fn st_12_disabled_preemption_means_no_thread_is_ever_switched_away_from() {
    use omni8::preempt::Preempt;
    let sched = new_sched();
    let preempt = Box::leak(Box::new(Preempt::new(PREEMPT_QUANTUM).unwrap()));
    let log = Box::leak(Box::new(PreemptLog::new()));
    preempt.disable();
    assert_eq!(
        run_preempt_scenario(sched, preempt, log),
        Err(PreemptScenarioError::Order)
    );
    // thread 0 ran all of its work first, then thread 1: nothing was preempted
    let snap = log.snapshot();
    assert!(snap[..PREEMPT_ITERATIONS].iter().all(|&v| v == 0));
    assert!(snap[PREEMPT_ITERATIONS..].iter().all(|&v| v == 1));
    assert_eq!(preempt.honoured(), 0);
    assert!(preempt.requests() > 0);
}
