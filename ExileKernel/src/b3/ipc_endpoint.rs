//! Bounded IPC endpoint queue (Mission B3). SPDX-License-Identifier: Apache-2.0
//!
//! An [`Endpoint`] is a fixed-capacity FIFO of in-kernel [`Message`]s. It knows nothing about
//! threads or capabilities' meaning: blocking, hand-off and capability bookkeeping live in the
//! IPC domain, which uses the endpoint only as storage with strict bounds. A full queue never
//! drops or overwrites a message (`push` fails with [`QueueError::Full`]), which is what lets
//! the domain turn a full queue into sender back-pressure instead of data loss.
#![allow(dead_code)]

use super::ipc_message::Message;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QueueError {
    Full,
    Empty,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct EndpointStats {
    pub pushed: u64,
    pub popped: u64,
}

pub struct Endpoint<const Q: usize> {
    buf: [Option<Message>; Q],
    head: usize,
    len: usize,
    stats: EndpointStats,
}

impl<const Q: usize> Default for Endpoint<Q> {
    fn default() -> Self {
        Self::new()
    }
}

impl<const Q: usize> Endpoint<Q> {
    pub const fn new() -> Self {
        assert!(Q > 0, "an endpoint needs at least one queue slot");
        Self {
            buf: [None; Q],
            head: 0,
            len: 0,
            stats: EndpointStats {
                pushed: 0,
                popped: 0,
            },
        }
    }

    pub const fn capacity(&self) -> usize {
        Q
    }

    pub const fn len(&self) -> usize {
        self.len
    }

    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub const fn is_full(&self) -> bool {
        self.len == Q
    }

    pub const fn stats(&self) -> EndpointStats {
        self.stats
    }

    pub fn push(&mut self, m: Message) -> Result<(), QueueError> {
        if self.is_full() {
            return Err(QueueError::Full);
        }
        let tail = (self.head + self.len) % Q;
        self.buf[tail] = Some(m);
        self.len += 1;
        self.stats.pushed += 1;
        Ok(())
    }

    pub fn pop(&mut self) -> Result<Message, QueueError> {
        if self.len == 0 {
            return Err(QueueError::Empty);
        }
        let m = self.buf[self.head].take().ok_or(QueueError::Empty)?;
        self.head = (self.head + 1) % Q;
        self.len -= 1;
        self.stats.popped += 1;
        Ok(m)
    }

    pub fn peek(&self) -> Option<&Message> {
        if self.len == 0 {
            None
        } else {
            self.buf[self.head].as_ref()
        }
    }

    /// Removes and returns every queued message, oldest first, into `out`; returns the count.
    /// `out` must have room for `len()` messages (checked: nothing is lost on a short buffer).
    pub fn drain_into(&mut self, out: &mut [Option<Message>; Q]) -> usize {
        let mut n = 0;
        while let Ok(m) = self.pop() {
            out[n] = Some(m);
            n += 1;
        }
        n
    }

    /// Iterates the queued messages oldest first without removing them.
    pub fn iter(&self) -> impl Iterator<Item = &Message> + '_ {
        (0..self.len).filter_map(move |i| self.buf[(self.head + i) % Q].as_ref())
    }

    pub fn check_invariants(&self) -> Result<(), &'static str> {
        if self.len > Q || self.head >= Q {
            return Err("queue indices out of range");
        }
        for i in 0..Q {
            let inside = (i + Q - self.head) % Q < self.len;
            if inside != self.buf[i].is_some() {
                return Err("occupied slots do not match the head/len window");
            }
        }
        if self.stats.pushed - self.stats.popped != self.len as u64 {
            return Err("push/pop counters disagree with the length");
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    extern crate std;

    fn m(label: u64) -> Message {
        Message {
            label,
            len: 0,
            words: [0; 4],
            cap: None,
        }
    }

    #[test]
    fn fifo_order_and_bounds() {
        let mut e = Endpoint::<3>::new();
        assert!(e.is_empty() && !e.is_full());
        assert_eq!(e.pop(), Err(QueueError::Empty));
        assert!(e.peek().is_none());
        for i in 1..=3 {
            e.push(m(i)).unwrap();
        }
        assert!(e.is_full());
        assert_eq!(e.push(m(4)), Err(QueueError::Full), "no overwrite");
        assert_eq!(e.len(), 3);
        assert_eq!(e.peek().unwrap().label, 1);
        for i in 1..=3 {
            assert_eq!(e.pop().unwrap().label, i);
        }
        assert_eq!(e.pop(), Err(QueueError::Empty));
        e.check_invariants().unwrap();
    }

    #[test]
    fn wraps_around_many_times_without_losing_order() {
        let mut e = Endpoint::<4>::new();
        let mut next_in = 0u64;
        let mut next_out = 0u64;
        for round in 0..1000u64 {
            let burst = 1 + (round % 4) as usize;
            for _ in 0..burst {
                if e.push(m(next_in)).is_ok() {
                    next_in += 1;
                }
            }
            for _ in 0..(1 + (round * 7 % 4) as usize) {
                if let Ok(x) = e.pop() {
                    assert_eq!(x.label, next_out);
                    next_out += 1;
                }
            }
            e.check_invariants().unwrap();
        }
        while let Ok(x) = e.pop() {
            assert_eq!(x.label, next_out);
            next_out += 1;
        }
        assert_eq!(next_in, next_out);
        assert_eq!(e.stats().pushed, e.stats().popped);
    }

    #[test]
    fn single_slot_endpoint_works() {
        let mut e = Endpoint::<1>::new();
        e.push(m(1)).unwrap();
        assert_eq!(e.push(m(2)), Err(QueueError::Full));
        assert_eq!(e.pop().unwrap().label, 1);
        e.push(m(3)).unwrap();
        assert_eq!(e.pop().unwrap().label, 3);
    }

    #[test]
    fn drain_returns_everything_in_order_and_empties_the_queue() {
        let mut e = Endpoint::<4>::new();
        for i in 0..4 {
            e.push(m(i)).unwrap();
        }
        e.pop().unwrap();
        e.push(m(9)).unwrap(); // wrapped window
        assert_eq!(
            e.iter().map(|x| x.label).collect::<std::vec::Vec<_>>(),
            [1, 2, 3, 9]
        );
        let mut out = [None; 4];
        assert_eq!(e.drain_into(&mut out), 4);
        let got: std::vec::Vec<u64> = out.iter().flatten().map(|x| x.label).collect();
        assert_eq!(got, [1, 2, 3, 9]);
        assert!(e.is_empty());
        e.check_invariants().unwrap();
        assert_eq!(e.drain_into(&mut [None; 4]), 0);
    }

    #[test]
    fn invariant_checker_detects_corruption() {
        let mut e = Endpoint::<3>::new();
        e.push(m(1)).unwrap();
        e.check_invariants().unwrap();
        e.len = 2;
        assert!(e.check_invariants().is_err());
        e.len = 1;
        e.buf[2] = Some(m(5));
        assert!(e.check_invariants().is_err());
        e.buf[2] = None;
        e.stats.popped = 1;
        assert!(e.check_invariants().is_err());
    }
}
