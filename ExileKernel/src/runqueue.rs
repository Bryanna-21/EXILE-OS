//! Fixed-capacity, allocation-free priority run queues.
//!
//! Entries are slot indices into a `ThreadTable`. Each slot has an intrusive
//! doubly-linked link, so insertion, removal and duplicate detection are O(1)
//! and never allocate (safe for use from interrupt context once a lock
//! protects the structure; this module itself performs no locking).
use crate::thread::PRIORITY_LEVELS;

const NONE: usize = usize::MAX;
const _: () = assert!(PRIORITY_LEVELS <= 8, "nonempty mask is a u8");

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QueueError {
    BadLevel,
    BadIndex,
    AlreadyQueued,
    NotQueued,
}

#[derive(Clone, Copy)]
struct Link {
    next: usize,
    prev: usize,
    level: u8,
    queued: bool,
}

pub struct RunQueues<const N: usize> {
    links: [Link; N],
    head: [usize; PRIORITY_LEVELS],
    tail: [usize; PRIORITY_LEVELS],
    len: [usize; PRIORITY_LEVELS],
    nonempty: u8,
}

impl<const N: usize> Default for RunQueues<N> {
    fn default() -> Self {
        Self::new()
    }
}

impl<const N: usize> RunQueues<N> {
    pub fn new() -> Self {
        Self {
            links: [Link {
                next: NONE,
                prev: NONE,
                level: 0,
                queued: false,
            }; N],
            head: [NONE; PRIORITY_LEVELS],
            tail: [NONE; PRIORITY_LEVELS],
            len: [0; PRIORITY_LEVELS],
            nonempty: 0,
        }
    }

    pub fn contains(&self, index: usize) -> bool {
        index < N && self.links[index].queued
    }

    pub fn level_of(&self, index: usize) -> Option<usize> {
        self.contains(index)
            .then(|| self.links[index].level as usize)
    }

    pub fn len_at(&self, level: usize) -> usize {
        self.len.get(level).copied().unwrap_or(0)
    }

    pub fn total(&self) -> usize {
        self.len.iter().sum()
    }

    /// Highest level that currently holds at least one entry.
    pub fn highest_level(&self) -> Option<usize> {
        if self.nonempty == 0 {
            None
        } else {
            Some(7 - self.nonempty.leading_zeros() as usize)
        }
    }

    pub fn push_back(&mut self, index: usize, level: usize) -> Result<(), QueueError> {
        if level >= PRIORITY_LEVELS {
            return Err(QueueError::BadLevel);
        }
        if index >= N {
            return Err(QueueError::BadIndex);
        }
        if self.links[index].queued {
            return Err(QueueError::AlreadyQueued);
        }
        let tail = self.tail[level];
        self.links[index] = Link {
            next: NONE,
            prev: tail,
            level: level as u8,
            queued: true,
        };
        if tail == NONE {
            self.head[level] = index;
        } else {
            self.links[tail].next = index;
        }
        self.tail[level] = index;
        self.len[level] += 1;
        self.nonempty |= 1 << level;
        Ok(())
    }

    /// Removes and returns the oldest entry of the highest non-empty level.
    pub fn pop_front_highest(&mut self) -> Option<(usize, usize)> {
        let level = self.highest_level()?;
        let index = self.head[level];
        self.remove(index).ok()?;
        Some((index, level))
    }

    pub fn remove(&mut self, index: usize) -> Result<(), QueueError> {
        if index >= N {
            return Err(QueueError::BadIndex);
        }
        let link = self.links[index];
        if !link.queued {
            return Err(QueueError::NotQueued);
        }
        let level = link.level as usize;
        if link.prev == NONE {
            self.head[level] = link.next;
        } else {
            self.links[link.prev].next = link.next;
        }
        if link.next == NONE {
            self.tail[level] = link.prev;
        } else {
            self.links[link.next].prev = link.prev;
        }
        self.links[index] = Link {
            next: NONE,
            prev: NONE,
            level: 0,
            queued: false,
        };
        self.len[level] -= 1;
        if self.len[level] == 0 {
            self.nonempty &= !(1 << level);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;

    #[test]
    fn fifo_within_level() {
        let mut q = RunQueues::<8>::new();
        for i in [3, 1, 2] {
            q.push_back(i, 4).unwrap();
        }
        assert_eq!(q.pop_front_highest(), Some((3, 4)));
        assert_eq!(q.pop_front_highest(), Some((1, 4)));
        assert_eq!(q.pop_front_highest(), Some((2, 4)));
        assert_eq!(q.pop_front_highest(), None);
    }

    #[test]
    fn highest_level_first() {
        let mut q = RunQueues::<8>::new();
        q.push_back(0, 1).unwrap();
        q.push_back(1, 7).unwrap();
        q.push_back(2, 3).unwrap();
        assert_eq!(q.highest_level(), Some(7));
        assert_eq!(q.pop_front_highest(), Some((1, 7)));
        assert_eq!(q.pop_front_highest(), Some((2, 3)));
        assert_eq!(q.pop_front_highest(), Some((0, 1)));
        assert_eq!(q.highest_level(), None);
    }

    #[test]
    fn duplicate_and_bounds_rejected() {
        let mut q = RunQueues::<4>::new();
        q.push_back(0, 2).unwrap();
        assert_eq!(q.push_back(0, 2), Err(QueueError::AlreadyQueued));
        assert_eq!(q.push_back(0, 5), Err(QueueError::AlreadyQueued));
        assert_eq!(q.push_back(4, 2), Err(QueueError::BadIndex));
        assert_eq!(q.push_back(1, PRIORITY_LEVELS), Err(QueueError::BadLevel));
        assert_eq!(q.remove(1), Err(QueueError::NotQueued));
        assert_eq!(q.remove(9), Err(QueueError::BadIndex));
        assert_eq!(q.total(), 1);
    }

    #[test]
    fn remove_head_middle_tail() {
        let mut q = RunQueues::<8>::new();
        for i in 0..5 {
            q.push_back(i, 2).unwrap();
        }
        q.remove(2).unwrap();
        q.remove(0).unwrap();
        q.remove(4).unwrap();
        assert_eq!(q.len_at(2), 2);
        assert_eq!(q.pop_front_highest(), Some((1, 2)));
        assert_eq!(q.pop_front_highest(), Some((3, 2)));
        assert_eq!(q.nonempty, 0);
    }

    #[test]
    fn reinsert_after_remove() {
        let mut q = RunQueues::<2>::new();
        q.push_back(0, 1).unwrap();
        q.remove(0).unwrap();
        q.push_back(0, 6).unwrap();
        assert_eq!(q.level_of(0), Some(6));
    }

    #[test]
    fn matches_reference_model_under_random_ops() {
        const N: usize = 12;
        let mut q = RunQueues::<N>::new();
        let mut model: Vec<VecDeque<usize>> = vec![VecDeque::new(); PRIORITY_LEVELS];
        let mut x = 0x9e37_79b9_7f4a_7c15u64;
        for _ in 0..50_000 {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            let idx = (x % N as u64) as usize;
            let lvl = ((x >> 8) % PRIORITY_LEVELS as u64) as usize;
            match (x >> 20) % 3 {
                0 => {
                    let queued = model.iter().any(|d| d.contains(&idx));
                    assert_eq!(q.push_back(idx, lvl).is_ok(), !queued);
                    if !queued {
                        model[lvl].push_back(idx);
                    }
                }
                1 => {
                    let expect = model.iter().rposition(|d| !d.is_empty()).map(|l| {
                        let i = model[l].pop_front().unwrap();
                        (i, l)
                    });
                    assert_eq!(q.pop_front_highest(), expect);
                }
                _ => {
                    let pos = model
                        .iter()
                        .enumerate()
                        .find_map(|(l, d)| d.iter().position(|&e| e == idx).map(|p| (l, p)));
                    assert_eq!(q.remove(idx).is_ok(), pos.is_some());
                    if let Some((l, p)) = pos {
                        model[l].remove(p);
                    }
                }
            }
            assert_eq!(q.total(), model.iter().map(|d| d.len()).sum::<usize>());
        }
    }
}
