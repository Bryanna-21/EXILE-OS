//! Bounded application-processor startup sequencing and state transitions.

use core::sync::atomic::{AtomicU8, Ordering};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum ApState {
    Offline = 0,
    Prepared = 1,
    InitSent = 2,
    SipiSent = 3,
    Online = 4,
    Failed = 5,
}

impl ApState {
    fn from_raw(value: u8) -> Self {
        match value {
            1 => Self::Prepared,
            2 => Self::InitSent,
            3 => Self::SipiSent,
            4 => Self::Online,
            5 => Self::Failed,
            _ => Self::Offline,
        }
    }
}

pub struct ApBootRecord {
    apic_id: u32,
    state: AtomicU8,
}
impl ApBootRecord {
    pub const fn new(apic_id: u32) -> Self {
        Self {
            apic_id,
            state: AtomicU8::new(ApState::Offline as u8),
        }
    }
    pub const fn apic_id(&self) -> u32 {
        self.apic_id
    }
    pub fn state(&self) -> ApState {
        ApState::from_raw(self.state.load(Ordering::Acquire))
    }
    fn transition(&self, from: ApState, to: ApState) -> Result<(), StartupError> {
        self.state
            .compare_exchange(from as u8, to as u8, Ordering::AcqRel, Ordering::Acquire)
            .map(|_| ())
            .map_err(|_| StartupError::InvalidState)
    }
    pub fn prepare(&self) -> Result<(), StartupError> {
        self.transition(ApState::Offline, ApState::Prepared)
    }
    pub fn mark_init_sent(&self) -> Result<(), StartupError> {
        self.transition(ApState::Prepared, ApState::InitSent)
    }
    pub fn mark_sipi_sent(&self) -> Result<(), StartupError> {
        self.transition(ApState::InitSent, ApState::SipiSent)
    }
    pub fn mark_online(&self) -> Result<(), StartupError> {
        self.transition(ApState::SipiSent, ApState::Online)
    }
    /// Marks startup failed unless the AP has already acknowledged being online.
    /// Returns true only when this call changed the state to Failed.
    pub fn mark_failed(&self) -> bool {
        loop {
            let current = self.state.load(Ordering::Acquire);
            if current == ApState::Online as u8 || current == ApState::Failed as u8 {
                return false;
            }
            if self
                .state
                .compare_exchange_weak(
                    current,
                    ApState::Failed as u8,
                    Ordering::AcqRel,
                    Ordering::Acquire,
                )
                .is_ok()
            {
                return true;
            }
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StartupError {
    InvalidVector,
    InvalidState,
    Transport,
    OnlineTimeout,
}

pub trait StartupTransport {
    fn send_init(&mut self, destination_apic_id: u32) -> Result<(), ()>;
    fn send_sipi(&mut self, destination_apic_id: u32, startup_vector: u8) -> Result<(), ()>;
    fn delay_us(&mut self, microseconds: u32);
}

/// Sends INIT, then two SIPIs, and waits a bounded number of iterations for AP acknowledgement.
/// `startup_vector` is the physical 4-KiB page number containing the real-mode trampoline.
pub fn start_ap<T: StartupTransport>(
    record: &ApBootRecord,
    transport: &mut T,
    startup_vector: u8,
    online_wait_iterations: usize,
) -> Result<(), StartupError> {
    if startup_vector == 0 {
        return Err(StartupError::InvalidVector);
    }
    record.prepare()?;
    if transport.send_init(record.apic_id()).is_err() {
        record.mark_failed();
        return if record.state() == ApState::Online {
            Ok(())
        } else {
            Err(StartupError::Transport)
        };
    }
    record.mark_init_sent()?;
    transport.delay_us(10_000);
    if transport
        .send_sipi(record.apic_id(), startup_vector)
        .is_err()
    {
        record.mark_failed();
        return if record.state() == ApState::Online {
            Ok(())
        } else {
            Err(StartupError::Transport)
        };
    }
    record.mark_sipi_sent()?;
    transport.delay_us(200);
    if transport
        .send_sipi(record.apic_id(), startup_vector)
        .is_err()
    {
        record.mark_failed();
        return if record.state() == ApState::Online {
            Ok(())
        } else {
            Err(StartupError::Transport)
        };
    }
    for _ in 0..online_wait_iterations {
        if record.state() == ApState::Online {
            return Ok(());
        }
        core::hint::spin_loop();
    }
    record.mark_failed();
    if record.state() == ApState::Online {
        Ok(())
    } else {
        Err(StartupError::OnlineTimeout)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Fake {
        calls: usize,
        fail: bool,
    }
    impl StartupTransport for Fake {
        fn send_init(&mut self, _: u32) -> Result<(), ()> {
            self.calls += 1;
            if self.fail { Err(()) } else { Ok(()) }
        }
        fn send_sipi(&mut self, _: u32, _: u8) -> Result<(), ()> {
            self.calls += 1;
            if self.fail { Err(()) } else { Ok(()) }
        }
        fn delay_us(&mut self, _: u32) {}
    }
    #[test]
    fn state_machine_rejects_skipped_transition() {
        let record = ApBootRecord::new(1);
        assert_eq!(record.mark_online(), Err(StartupError::InvalidState));
    }
    #[test]
    fn failure_transition_never_overwrites_online_state() {
        let record = ApBootRecord::new(1);
        record.prepare().unwrap();
        record.mark_init_sent().unwrap();
        record.mark_sipi_sent().unwrap();
        record.mark_online().unwrap();

        assert!(!record.mark_failed());
        assert_eq!(record.state(), ApState::Online);
    }

    #[test]
    fn failed_transport_marks_cpu_failed() {
        let record = ApBootRecord::new(1);
        let mut fake = Fake {
            calls: 0,
            fail: true,
        };
        assert_eq!(
            start_ap(&record, &mut fake, 8, 2),
            Err(StartupError::Transport)
        );
        assert_eq!(record.state(), ApState::Failed);
        assert_eq!(fake.calls, 1);
    }
}
