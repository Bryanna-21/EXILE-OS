//! Generation-based TLB shootdown mailbox with acquire/release publication.

use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

pub const PAGE_SIZE: u64 = 4096;
pub const SHOOTDOWN_GLOBAL: u32 = 1;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TlbRequest {
    pub generation: u64,
    pub start: u64,
    pub end: u64,
    pub flags: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ShootdownError {
    InvalidRange,
    GenerationOverflow,
    Timeout,
    Busy,
}

pub struct TlbMailbox {
    generation: AtomicU64,
    acknowledged: AtomicU64,
    start: AtomicU64,
    end: AtomicU64,
    flags: AtomicU64,
    sequence: AtomicU64,
    publisher: AtomicBool,
}
impl TlbMailbox {
    pub const fn new() -> Self {
        Self {
            generation: AtomicU64::new(0),
            acknowledged: AtomicU64::new(0),
            start: AtomicU64::new(0),
            end: AtomicU64::new(0),
            flags: AtomicU64::new(0),
            sequence: AtomicU64::new(0),
            publisher: AtomicBool::new(false),
        }
    }
    pub fn publish(&self, start: u64, end: u64, flags: u32) -> Result<u64, ShootdownError> {
        if start >= end || start % PAGE_SIZE != 0 || end % PAGE_SIZE != 0 {
            return Err(ShootdownError::InvalidRange);
        }
        if self
            .publisher
            .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
        {
            return Err(ShootdownError::Busy);
        }

        let current = self.generation.load(Ordering::Relaxed);
        let Some(next) = current.checked_add(1) else {
            self.publisher.store(false, Ordering::Release);
            return Err(ShootdownError::GenerationOverflow);
        };

        // Odd sequence means a request is being published. Readers retry
        // unless they observe the same even sequence before and after copying.
        self.sequence.fetch_add(1, Ordering::AcqRel);
        self.start.store(start, Ordering::Relaxed);
        self.end.store(end, Ordering::Relaxed);
        self.flags.store(flags as u64, Ordering::Relaxed);
        self.generation.store(next, Ordering::Relaxed);
        self.sequence.fetch_add(1, Ordering::Release);

        self.publisher.store(false, Ordering::Release);
        Ok(next)
    }
    pub fn take_after(&self, seen_generation: u64) -> Option<TlbRequest> {
        loop {
            let before = self.sequence.load(Ordering::Acquire);
            if before & 1 != 0 {
                core::hint::spin_loop();
                continue;
            }

            let generation = self.generation.load(Ordering::Relaxed);
            if generation == 0 || generation == seen_generation {
                let after = self.sequence.load(Ordering::Acquire);
                if before == after {
                    return None;
                }
                continue;
            }

            let request = TlbRequest {
                generation,
                start: self.start.load(Ordering::Relaxed),
                end: self.end.load(Ordering::Relaxed),
                flags: self.flags.load(Ordering::Relaxed) as u32,
            };

            let after = self.sequence.load(Ordering::Acquire);
            if before == after {
                return Some(request);
            }
        }
    }
    pub fn acknowledge(&self, generation: u64) {
        // Acknowledge only generations that have actually been published.
        if generation <= self.generation.load(Ordering::Acquire) {
            self.acknowledged.fetch_max(generation, Ordering::Release);
        }
    }
    pub fn is_acknowledged(&self, generation: u64) -> bool {
        self.acknowledged.load(Ordering::Acquire) >= generation
    }
    pub fn wait_for_ack(&self, generation: u64, iterations: usize) -> Result<(), ShootdownError> {
        for _ in 0..iterations {
            if self.is_acknowledged(generation) {
                return Ok(());
            }
            core::hint::spin_loop();
        }
        Err(ShootdownError::Timeout)
    }
}
impl Default for TlbMailbox {
    fn default() -> Self {
        Self::new()
    }
}

pub trait TlbInvalidator {
    fn invalidate_page(&mut self, virtual_address: u64);
    fn flush_all(&mut self);
}

pub fn service_request(invalidator: &mut impl TlbInvalidator, request: TlbRequest) {
    if request.flags & SHOOTDOWN_GLOBAL != 0 {
        invalidator.flush_all();
        return;
    }
    let mut address = request.start;
    while address < request.end {
        invalidator.invalidate_page(address);
        address += PAGE_SIZE;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn publication_and_acknowledgement_are_monotonic() {
        let mailbox = TlbMailbox::new();
        let generation = mailbox.publish(0x1000, 0x3000, 0).unwrap();
        let request = mailbox.take_after(0).unwrap();
        assert_eq!(request.generation, generation);
        assert_eq!((request.start, request.end), (0x1000, 0x3000));
        mailbox.acknowledge(generation);
        assert!(mailbox.is_acknowledged(generation));
    }
    #[test]
    fn rejects_concurrent_publisher_without_overwriting_request() {
        let mailbox = TlbMailbox::new();
        let first = mailbox.publish(0x1000, 0x3000, 0).unwrap();

        mailbox.publisher.store(true, Ordering::Relaxed);
        assert_eq!(
            mailbox.publish(0x4000, 0x5000, 0),
            Err(ShootdownError::Busy)
        );
        mailbox.publisher.store(false, Ordering::Relaxed);

        let request = mailbox.take_after(0).unwrap();
        assert_eq!(request.generation, first);
        assert_eq!((request.start, request.end), (0x1000, 0x3000));
    }

    #[test]
    fn does_not_acknowledge_an_unpublished_generation() {
        let mailbox = TlbMailbox::new();
        mailbox.acknowledge(1);
        assert!(!mailbox.is_acknowledged(1));
    }

    #[test]
    fn rejects_unaligned_or_empty_ranges() {
        let mailbox = TlbMailbox::new();
        assert_eq!(
            mailbox.publish(1, 4096, 0),
            Err(ShootdownError::InvalidRange)
        );
        assert_eq!(
            mailbox.publish(4096, 4096, 0),
            Err(ShootdownError::InvalidRange)
        );
    }
}
