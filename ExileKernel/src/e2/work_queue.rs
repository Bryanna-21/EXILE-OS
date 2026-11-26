//! Bounded FIFO work queue. Queue ownership is external; no worker is created here.
use super::work_item::{WorkError, WorkId, WorkItem, WorkState};
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QueueError {
    Full,
    Duplicate,
    NotFound,
    Corrupt,
    CounterOverflow,
    Item(WorkError),
}
impl From<WorkError> for QueueError {
    fn from(e: WorkError) -> Self {
        Self::Item(e)
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct QueueStats {
    pub enqueued: u64,
    pub dequeued: u64,
    pub cancelled: u64,
    pub high_water: usize,
}
pub struct WorkQueue<const N: usize> {
    items: [Option<WorkItem>; N],
    head: usize,
    len: usize,
    stats: QueueStats,
}
impl<const N: usize> WorkQueue<N> {
    pub const fn new() -> Self {
        Self {
            items: [None; N],
            head: 0,
            len: 0,
            stats: QueueStats {
                enqueued: 0,
                dequeued: 0,
                cancelled: 0,
                high_water: 0,
            },
        }
    }
    pub const fn len(&self) -> usize {
        self.len
    }
    pub const fn stats(&self) -> QueueStats {
        self.stats
    }
    pub fn enqueue(&mut self, item: WorkItem) -> Result<(), QueueError> {
        if item.state() != WorkState::Pending {
            return Err(QueueError::Item(WorkError::InvalidTransition));
        }
        if self.len == N {
            return Err(QueueError::Full);
        }
        if self.items.iter().flatten().any(|x| x.id == item.id) {
            return Err(QueueError::Duplicate);
        }
        let next_count = self
            .stats
            .enqueued
            .checked_add(1)
            .ok_or(QueueError::CounterOverflow)?;
        let i = (self.head + self.len) % N.max(1);
        self.items[i] = Some(item);
        self.len += 1;
        self.stats.enqueued = next_count;
        self.stats.high_water = self.stats.high_water.max(self.len);
        Ok(())
    }
    pub fn dequeue(&mut self) -> Result<Option<WorkItem>, QueueError> {
        if self.len == 0 {
            return Ok(None);
        }
        let next_count = self
            .stats
            .dequeued
            .checked_add(1)
            .ok_or(QueueError::CounterOverflow)?;
        let mut item = self.items[self.head].ok_or(QueueError::Corrupt)?;
        item.start()?;
        self.items[self.head] = None;
        self.head = (self.head + 1) % N.max(1);
        self.len -= 1;
        self.stats.dequeued = next_count;
        Ok(Some(item))
    }
    pub fn cancel(&mut self, id: WorkId) -> Result<(), QueueError> {
        let offset = (0..self.len)
            .find(|o| self.items[(self.head + *o) % N.max(1)].is_some_and(|w| w.id == id))
            .ok_or(QueueError::NotFound)?;
        let next_count = self
            .stats
            .cancelled
            .checked_add(1)
            .ok_or(QueueError::CounterOverflow)?;
        let pos = (self.head + offset) % N.max(1);
        let mut item = self.items[pos].ok_or(QueueError::Corrupt)?;
        item.cancel()?;
        for o in offset..self.len - 1 {
            let dst = (self.head + o) % N.max(1);
            let src = (self.head + o + 1) % N.max(1);
            self.items[dst] = self.items[src].take();
        }
        let last = (self.head + self.len - 1) % N.max(1);
        self.items[last] = None;
        self.len -= 1;
        self.stats.cancelled = next_count;
        Ok(())
    }
    pub fn check_invariants(&self) -> bool {
        self.len <= N
            && self.items.iter().filter(|x| x.is_some()).count() == self.len
            && (0..self.len).all(|o| {
                self.items[(self.head + o) % N.max(1)]
                    .is_some_and(|w| w.state() == WorkState::Pending)
            })
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn item(n: u64) -> WorkItem {
        WorkItem::new(WorkId::new(n).unwrap(), 0)
    }
    #[test]
    fn fifo_duplicate_cancel() {
        let mut q = WorkQueue::<3>::new();
        q.enqueue(item(1)).unwrap();
        q.enqueue(item(2)).unwrap();
        assert_eq!(q.enqueue(item(1)), Err(QueueError::Duplicate));
        q.cancel(WorkId::new(1).unwrap()).unwrap();
        assert_eq!(q.dequeue().unwrap().unwrap().id, WorkId::new(2).unwrap());
        assert!(q.check_invariants());
    }
    #[test]
    fn full_and_bounded() {
        let mut q = WorkQueue::<1>::new();
        q.enqueue(item(1)).unwrap();
        assert_eq!(q.enqueue(item(2)), Err(QueueError::Full));
        assert_eq!(
            q.dequeue().unwrap().unwrap().state(),
            super::super::work_item::WorkState::Running
        );
    }
    #[test]
    fn cancellation_preserves_fifo_across_ring_wrap() {
        let mut q = WorkQueue::<3>::new();
        q.enqueue(item(1)).unwrap();
        q.enqueue(item(2)).unwrap();
        q.dequeue().unwrap();
        q.enqueue(item(3)).unwrap();
        q.enqueue(item(4)).unwrap();
        q.cancel(WorkId::new(3).unwrap()).unwrap();
        assert_eq!(q.dequeue().unwrap().unwrap().id, WorkId::new(2).unwrap());
        assert_eq!(q.dequeue().unwrap().unwrap().id, WorkId::new(4).unwrap());
        assert!(q.check_invariants());
    }
}
