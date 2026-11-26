//! Notification objects: a signal word for event delivery (Mission B3). SPDX-License-Identifier: Apache-2.0
//!
//! A [`Notify`] holds a 64-bit word. `signal(bits)` ORs bits in and never blocks, so it is the
//! operation interrupt handlers and other non-blocking contexts use; repeated signals before a
//! wait coalesce (the word is a set of pending events, not a counter). `take()` returns the
//! whole word and clears it atomically with respect to the single-CPU kernel lock, so no signal
//! is ever lost or seen twice. Sleeping/waking is the IPC domain's job.
#![allow(dead_code)]

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotifyError {
    /// `signal(0)` carries no event and is rejected rather than silently ignored.
    NoBits,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct NotifyStats {
    pub signals: u64,
    pub takes: u64,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Notify {
    word: u64,
    stats: NotifyStats,
}

impl Notify {
    pub const fn new() -> Self {
        Self {
            word: 0,
            stats: NotifyStats {
                signals: 0,
                takes: 0,
            },
        }
    }

    pub const fn pending(&self) -> u64 {
        self.word
    }

    pub const fn stats(&self) -> NotifyStats {
        self.stats
    }

    pub fn signal(&mut self, bits: u64) -> Result<(), NotifyError> {
        if bits == 0 {
            return Err(NotifyError::NoBits);
        }
        self.word |= bits;
        self.stats.signals += 1;
        Ok(())
    }

    /// Returns and clears the pending word; `None` when nothing is pending.
    pub fn take(&mut self) -> Option<u64> {
        if self.word == 0 {
            return None;
        }
        self.stats.takes += 1;
        Some(core::mem::take(&mut self.word))
    }

    pub fn reset(&mut self) {
        self.word = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signals_coalesce_and_take_clears() {
        let mut n = Notify::new();
        assert_eq!(n.take(), None);
        n.signal(0b0001).unwrap();
        n.signal(0b0100).unwrap();
        n.signal(0b0001).unwrap();
        assert_eq!(n.pending(), 0b0101);
        assert_eq!(n.take(), Some(0b0101));
        assert_eq!(n.pending(), 0);
        assert_eq!(n.take(), None, "no event is seen twice");
        assert_eq!(
            n.stats(),
            NotifyStats {
                signals: 3,
                takes: 1
            }
        );
    }

    #[test]
    fn zero_signal_is_rejected_and_changes_nothing() {
        let mut n = Notify::new();
        n.signal(8).unwrap();
        assert_eq!(n.signal(0), Err(NotifyError::NoBits));
        assert_eq!(n.pending(), 8);
        assert_eq!(n.stats().signals, 1);
    }

    #[test]
    fn all_sixty_four_bits_are_usable() {
        let mut n = Notify::new();
        for b in 0..64 {
            n.signal(1u64 << b).unwrap();
        }
        assert_eq!(n.take(), Some(u64::MAX));
        n.signal(u64::MAX).unwrap();
        n.signal(1 << 63).unwrap();
        assert_eq!(n.take(), Some(u64::MAX));
    }

    #[test]
    fn reset_discards_pending_events() {
        let mut n = Notify::new();
        n.signal(3).unwrap();
        n.reset();
        assert_eq!(n.take(), None);
    }
}
