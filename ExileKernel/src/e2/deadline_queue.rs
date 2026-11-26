//! Stable, fixed-capacity deadline min-queue (no clock or scheduler policy).
use super::deadline::{Deadline, Instant};
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QueueError {
    Full,
    Duplicate,
    SequenceExhausted,
    NotFound,
    Corrupt,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DeadlineEntry<T: Copy> {
    pub id: u64,
    pub deadline: Deadline,
    pub value: T,
}
#[derive(Clone, Copy)]
struct Slot<T: Copy> {
    entry: DeadlineEntry<T>,
    sequence: u64,
}
pub struct DeadlineQueue<T: Copy, const N: usize> {
    slots: [Option<Slot<T>>; N],
    len: usize,
    next_sequence: u64,
}
impl<T: Copy, const N: usize> Default for DeadlineQueue<T, N> {
    fn default() -> Self {
        Self::new()
    }
}
impl<T: Copy, const N: usize> DeadlineQueue<T, N> {
    pub const fn new() -> Self {
        Self {
            slots: [None; N],
            len: 0,
            next_sequence: 0,
        }
    }
    pub const fn len(&self) -> usize {
        self.len
    }
    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }
    pub fn insert(&mut self, entry: DeadlineEntry<T>) -> Result<(), QueueError> {
        if self.len == N {
            return Err(QueueError::Full);
        }
        if self.slots.iter().flatten().any(|s| s.entry.id == entry.id) {
            return Err(QueueError::Duplicate);
        }
        let seq = self.next_sequence;
        self.next_sequence = self
            .next_sequence
            .checked_add(1)
            .ok_or(QueueError::SequenceExhausted)?;
        let pos = self
            .slots
            .iter()
            .position(Option::is_none)
            .ok_or(QueueError::Corrupt)?;
        self.slots[pos] = Some(Slot {
            entry,
            sequence: seq,
        });
        self.len += 1;
        Ok(())
    }
    fn earliest_index(&self) -> Option<usize> {
        self.slots
            .iter()
            .enumerate()
            .filter_map(|(i, s)| s.map(|v| (i, v)))
            .min_by(|a, b| {
                a.1.entry
                    .deadline
                    .instant()
                    .cmp(&b.1.entry.deadline.instant())
                    .then(a.1.sequence.cmp(&b.1.sequence))
            })
            .map(|x| x.0)
    }
    pub fn earliest(&self) -> Option<DeadlineEntry<T>> {
        self.earliest_index()
            .and_then(|i| self.slots[i].map(|s| s.entry))
    }
    pub fn remove(&mut self, id: u64) -> Result<DeadlineEntry<T>, QueueError> {
        let i = self
            .slots
            .iter()
            .position(|s| s.is_some_and(|v| v.entry.id == id))
            .ok_or(QueueError::NotFound)?;
        let s = self.slots[i].take().ok_or(QueueError::Corrupt)?;
        self.len -= 1;
        Ok(s.entry)
    }
    pub fn pop_expired(&mut self, now: Instant) -> Option<DeadlineEntry<T>> {
        let i = self.earliest_index()?;
        let s = self.slots[i]?;
        if !s.entry.deadline.is_expired(now) {
            return None;
        }
        self.slots[i] = None;
        self.len -= 1;
        Some(s.entry)
    }
    pub fn check_invariants(&self) -> bool {
        self.len == self.slots.iter().filter(|s| s.is_some()).count() && self.len <= N
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::e2::deadline::Duration;
    #[test]
    fn stable_equal_deadlines_and_expiration() {
        let mut q = DeadlineQueue::<u8, 3>::new();
        let d = Deadline::after(Instant::ZERO, Duration::from_nanos(5)).unwrap();
        for (id, v) in [(1, 10), (2, 20)] {
            q.insert(DeadlineEntry {
                id,
                deadline: d,
                value: v,
            })
            .unwrap();
        }
        assert_eq!(q.pop_expired(Instant::from_nanos(4)), None);
        assert_eq!(q.pop_expired(Instant::from_nanos(5)).unwrap().id, 1);
        assert_eq!(q.pop_expired(Instant::from_nanos(5)).unwrap().id, 2);
    }
    #[test]
    fn capacity_and_remove() {
        let mut q = DeadlineQueue::<(), 1>::new();
        let d = Deadline::at(Instant::ZERO);
        q.insert(DeadlineEntry {
            id: 1,
            deadline: d,
            value: (),
        })
        .unwrap();
        assert_eq!(
            q.insert(DeadlineEntry {
                id: 2,
                deadline: d,
                value: ()
            }),
            Err(QueueError::Full)
        );
        q.remove(1).unwrap();
        assert!(q.check_invariants());
    }
}
