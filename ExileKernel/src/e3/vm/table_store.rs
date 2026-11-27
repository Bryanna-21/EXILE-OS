//! Fixed-capacity ownership ledger for page-table frames.
use super::backend::{BackendError, PageTableMemory};
use super::frame_source::{FrameLease, FrameSource, FrameSourceError};
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TableToken {
    slot: u16,
    generation: u32,
    physical: u64,
    level: u8,
}
impl TableToken {
    pub const fn physical(self) -> u64 {
        self.physical
    }
    pub const fn level(self) -> u8 {
        self.level
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StoreError {
    Full,
    Stale,
    Referenced,
    InvalidFrame,
    Frame(FrameSourceError),
    Backend(BackendError),
    BadLevel,
    GenerationExhausted,
    CleanupPending,
}
#[derive(Clone, Copy)]
struct PendingLease {
    frame: FrameLease,
    release_backend: bool,
}
#[derive(Clone, Copy)]
struct Record {
    frame: FrameLease,
    generation: u32,
    level: u8,
    references: u16,
}
pub struct TableStore<F, const N: usize> {
    frames: F,
    records: [Option<Record>; N],
    generations: [u32; N],
    retired: [bool; N],
    pending: [Option<PendingLease>; N],
}
impl<F: FrameSource, const N: usize> TableStore<F, N> {
    pub const fn new(frames: F) -> Self {
        Self {
            frames,
            records: [None; N],
            generations: [1; N],
            retired: [false; N],
            pending: [None; N],
        }
    }
    fn pending_slot(&self) -> Result<usize, StoreError> {
        self.pending
            .iter()
            .position(Option::is_none)
            .ok_or(StoreError::Full)
    }
    fn remember_pending(&mut self, slot: usize, frame: FrameLease, release_backend: bool) {
        self.pending[slot] = Some(PendingLease {
            frame,
            release_backend,
        });
    }
    pub fn pending_count(&self) -> usize {
        self.pending.iter().filter(|x| x.is_some()).count()
    }
    pub fn retry_pending<B: PageTableMemory>(&mut self, backend: &mut B) -> Result<(), StoreError> {
        for i in 0..N {
            if let Some(pending) = self.pending[i] {
                self.frames
                    .release(pending.frame)
                    .map_err(StoreError::Frame)?;
                if pending.release_backend {
                    backend.release_table(pending.frame.physical());
                }
                self.pending[i] = None;
            }
        }
        Ok(())
    }
    pub fn allocate<B: PageTableMemory>(
        &mut self,
        b: &mut B,
        level: u8,
    ) -> Result<TableToken, StoreError> {
        if !(1..=4).contains(&level) {
            return Err(StoreError::BadLevel);
        }
        if N > u16::MAX as usize + 1 {
            return Err(StoreError::Full);
        }
        let slot = self
            .records
            .iter()
            .enumerate()
            .position(|(i, r)| r.is_none() && !self.retired[i])
            .ok_or(StoreError::Full)?;
        let pending_slot = self.pending_slot()?;
        let frame = self.frames.allocate().map_err(StoreError::Frame)?;
        if frame.physical() == 0
            || frame.physical() & 4095 != 0
            || frame.physical() & !super::geometry::PHYS_ADDR_MASK != 0
        {
            if self.frames.release(frame).is_err() {
                self.remember_pending(pending_slot, frame, false);
                return Err(StoreError::CleanupPending);
            }
            return Err(StoreError::InvalidFrame);
        }
        if let Err(e) = b.zero_table(frame.physical()) {
            if self.frames.release(frame).is_err() {
                self.remember_pending(pending_slot, frame, true);
                return Err(StoreError::CleanupPending);
            }
            b.release_table(frame.physical());
            return Err(StoreError::Backend(e));
        }
        let g = self.generations[slot];
        self.records[slot] = Some(Record {
            frame,
            generation: g,
            level,
            references: 1,
        });
        Ok(TableToken {
            slot: slot as u16,
            generation: g,
            physical: frame.physical(),
            level,
        })
    }
    fn index(&self, t: TableToken) -> Result<usize, StoreError> {
        let i = t.slot as usize;
        let r = self
            .records
            .get(i)
            .and_then(|x| *x)
            .ok_or(StoreError::Stale)?;
        if r.generation != t.generation || r.frame.physical() != t.physical || r.level != t.level {
            return Err(StoreError::Stale);
        }
        Ok(i)
    }
    pub fn retain(&mut self, t: TableToken) -> Result<(), StoreError> {
        let i = self.index(t)?;
        let r = self.records[i].as_mut().ok_or(StoreError::Stale)?;
        r.references = r.references.checked_add(1).ok_or(StoreError::Referenced)?;
        Ok(())
    }
    pub fn release<B: PageTableMemory>(
        &mut self,
        b: &mut B,
        t: TableToken,
    ) -> Result<bool, StoreError> {
        let i = self.index(t)?;
        let r = self.records[i].ok_or(StoreError::Stale)?;
        if r.references > 1 {
            self.records[i]
                .as_mut()
                .ok_or(StoreError::Stale)?
                .references -= 1;
            return Ok(false);
        }
        self.frames.release(r.frame).map_err(StoreError::Frame)?;
        b.release_table(r.frame.physical());
        self.records[i] = None;
        if let Some(g) = self.generations[i].checked_add(1) {
            self.generations[i] = g
        } else {
            self.retired[i] = true
        }
        Ok(true)
    }
    pub fn token_for_physical(&self, p: u64) -> Option<TableToken> {
        self.records.iter().enumerate().find_map(|(i, r)| {
            r.filter(|x| x.frame.physical() == p).map(|x| TableToken {
                slot: i as u16,
                generation: x.generation,
                physical: p,
                level: x.level,
            })
        })
    }
    pub fn contains(&self, p: u64) -> bool {
        self.token_for_physical(p).is_some()
    }
    pub fn level_for_physical(&self, p: u64) -> Option<u8> {
        self.token_for_physical(p).map(|t| t.level())
    }
    pub fn live(&self) -> usize {
        self.records.iter().filter(|x| x.is_some()).count()
    }
    pub fn release_all<B: PageTableMemory>(&mut self, b: &mut B) -> Result<(), StoreError> {
        self.retry_pending(b)?;
        if self.records.iter().flatten().any(|r| r.references != 1) {
            return Err(StoreError::Referenced);
        }
        for i in (0..N).rev() {
            if let Some(r) = self.records[i] {
                self.frames.release(r.frame).map_err(StoreError::Frame)?;
                b.release_table(r.frame.physical());
                self.records[i] = None;
                if let Some(g) = self.generations[i].checked_add(1) {
                    self.generations[i] = g
                } else {
                    self.retired[i] = true
                }
            }
        }
        Ok(())
    }
    pub fn allocate_data_frame(&mut self) -> Result<FrameLease, StoreError> {
        let pending_slot = self.pending_slot()?;
        let l = self.frames.allocate().map_err(StoreError::Frame)?;
        if l.physical() == 0
            || l.physical() & 4095 != 0
            || l.physical() & !super::geometry::PHYS_ADDR_MASK != 0
        {
            if self.frames.release(l).is_err() {
                self.remember_pending(pending_slot, l, false);
                return Err(StoreError::CleanupPending);
            }
            return Err(StoreError::InvalidFrame);
        }
        Ok(l)
    }
    pub fn discard_data_frame(&mut self, lease: FrameLease) -> Result<(), StoreError> {
        let pending_slot = self.pending_slot()?;
        match self.frames.release(lease) {
            Ok(()) => Ok(()),
            Err(error) => {
                self.remember_pending(pending_slot, lease, false);
                Err(StoreError::Frame(error))
            }
        }
    }
    pub fn validate_data_frame(&self, l: FrameLease) -> bool {
        self.frames.validate(l)
    }
    pub fn release_data_frame(&mut self, l: FrameLease) -> Result<(), StoreError> {
        self.frames.release(l).map_err(StoreError::Frame)
    }
    pub fn check_invariants(&self) -> bool {
        self.records.iter().flatten().all(|r| {
            r.references > 0
                && r.frame.physical() != 0
                && r.frame.physical() & 4095 == 0
                && r.level >= 1
                && r.level <= 4
                && self.frames.validate(r.frame)
        }) && self
            .pending
            .iter()
            .flatten()
            .all(|p| p.frame.physical() & 4095 == 0 && self.frames.validate(p.frame))
            && self.records.iter().enumerate().all(|(i, a)| {
                a.is_none_or(|x| {
                    self.records[i + 1..]
                        .iter()
                        .flatten()
                        .all(|y| x.frame.physical() != y.frame.physical())
                        && self
                            .pending
                            .iter()
                            .flatten()
                            .all(|y| x.frame.physical() != y.frame.physical())
                })
            })
            && self.pending.iter().enumerate().all(|(i, a)| {
                a.is_none_or(|x| {
                    self.pending[i + 1..]
                        .iter()
                        .flatten()
                        .all(|y| x.frame.physical() != y.frame.physical())
                })
            })
    }
}
#[cfg(test)]
mod tests {
    use super::super::{backend::MockBackend, frame_source::MockFrames};
    use super::*;
    #[test]
    fn zero_failure_releases_frame_and_generation_rejects_stale_table() {
        let mut store = TableStore::<_, 1>::new(MockFrames::<2>::new(0x1000));
        let mut no_backend = MockBackend::<0>::new();
        assert_eq!(
            store.allocate(&mut no_backend, 1),
            Err(StoreError::Backend(BackendError::Capacity))
        );
        assert_eq!(store.live(), 0);
        let mut backend = MockBackend::<2>::new();
        let t = store.allocate(&mut backend, 1).unwrap();
        store.release(&mut backend, t).unwrap();
        assert_eq!(store.release(&mut backend, t), Err(StoreError::Stale));
        let next = store.allocate(&mut backend, 1).unwrap();
        assert_ne!(t.generation, next.generation);
    }
}
#[cfg(test)]
mod reference_tests {
    use super::super::{backend::MockBackend, frame_source::MockFrames};
    use super::*;
    #[test]
    fn retained_tables_cannot_be_torn_down_early() {
        let mut store = TableStore::<_, 1>::new(MockFrames::<2>::new(0x8000));
        let mut backend = MockBackend::<1>::new();
        let t = store.allocate(&mut backend, 4).unwrap();
        store.retain(t).unwrap();
        assert!(store.check_invariants());
        assert_eq!(store.release_all(&mut backend), Err(StoreError::Referenced));
        assert!(store.check_invariants());
        assert_eq!(store.release(&mut backend, t), Ok(false));
        assert!(store.check_invariants());
        assert_eq!(store.release(&mut backend, t), Ok(true));
        assert_eq!(store.live(), 0);
    }
}

#[cfg(test)]
mod pending_cleanup_tests {
    use super::super::{backend::MockBackend, frame_source::MockFrames};
    use super::*;

    #[test]
    fn failed_table_cleanup_remains_owned_and_can_be_retried() {
        let mut frames = MockFrames::<2>::new(0x1000);
        frames.fail_next_releases(1);
        let mut store = TableStore::<_, 1>::new(frames);
        let mut backend = MockBackend::<0>::new();

        assert_eq!(
            store.allocate(&mut backend, 1),
            Err(StoreError::CleanupPending)
        );
        assert_eq!(store.pending_count(), 1);
        assert!(store.check_invariants());
        assert_eq!(store.release_all(&mut backend), Ok(()));
        assert_eq!(store.pending_count(), 0);
        assert!(store.check_invariants());
    }

    #[test]
    fn failed_invalid_data_frame_cleanup_remains_owned_and_can_be_retried() {
        let mut frames = MockFrames::<2>::new(0);
        frames.fail_next_releases(1);
        let mut store = TableStore::<_, 1>::new(frames);

        assert_eq!(store.allocate_data_frame(), Err(StoreError::CleanupPending));
        assert_eq!(store.pending_count(), 1);
        assert!(store.check_invariants());
        store.retry_pending(&mut MockBackend::<1>::new()).unwrap();
        assert_eq!(store.pending_count(), 0);
        assert!(store.check_invariants());
    }
}
