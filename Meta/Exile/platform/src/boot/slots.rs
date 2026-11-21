//! A/B update slots with boot counting, rollback, and power-loss-safe persistence.
//!
//! Persistence model: two 32-byte record copies (A at index 0, B at index 1). A write goes to the
//! copy that is NOT the current newest, with generation+1, CRC last. On load the valid copy with
//! the highest generation wins, so a torn write leaves the previous record intact.
//! Verified only by simulated tearing at byte granularity; real flash/ESP semantics untested.

use crate::boot::info::crc32;

/// Slot identifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Slot {
    /// Slot A.
    A,
    /// Slot B.
    B,
}
impl Slot {
    /// The other slot.
    pub fn other(self) -> Slot {
        match self {
            Slot::A => Slot::B,
            Slot::B => Slot::A,
        }
    }
    fn idx(self) -> usize {
        self as usize
    }
}

/// Per-slot health.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SlotState {
    /// Never written / unusable.
    Empty = 0,
    /// Image written, not yet booted (pending trial).
    Trial = 1,
    /// Booted and confirmed good.
    Good = 2,
    /// Exhausted its boot attempts without confirmation.
    Bad = 3,
}

/// Maximum boot attempts for a trial slot.
pub const MAX_TRIES: u8 = 3;

/// The persistent update record.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Record {
    /// Monotonic generation.
    pub generation: u32,
    /// Slot the bootloader should try first.
    pub active: Slot,
    /// State of A and B.
    pub state: [SlotState; 2],
    /// Remaining tries for each slot.
    pub tries: [u8; 2],
}

/// Errors.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SlotError {
    /// Neither stored copy is valid.
    NoValidRecord,
    /// Update target is the active slot, or otherwise illegal in this state.
    IllegalTransition,
}

const LEN: usize = 32;
const MAGIC: u32 = 0x4C53_5845; // "EXSL"

impl Record {
    /// Fresh factory record: slot A good, B empty.
    pub fn factory() -> Self {
        Record {
            generation: 1,
            active: Slot::A,
            state: [SlotState::Good, SlotState::Empty],
            tries: [0, 0],
        }
    }
    /// Serialise to a 32-byte record with trailing CRC.
    pub fn encode(&self) -> [u8; LEN] {
        let mut b = [0u8; LEN];
        b[0..4].copy_from_slice(&MAGIC.to_le_bytes());
        b[4..8].copy_from_slice(&self.generation.to_le_bytes());
        b[8] = self.active as u8;
        b[9] = self.state[0] as u8;
        b[10] = self.state[1] as u8;
        b[11] = self.tries[0];
        b[12] = self.tries[1];
        let c = crc32(&b[..LEN - 4]);
        b[LEN - 4..].copy_from_slice(&c.to_le_bytes());
        b
    }
    /// Decode and validate; any inconsistency yields None.
    pub fn decode(b: &[u8; LEN]) -> Option<Record> {
        if u32::from_le_bytes([b[0], b[1], b[2], b[3]]) != MAGIC {
            return None;
        }
        if u32::from_le_bytes([b[28], b[29], b[30], b[31]]) != crc32(&b[..LEN - 4]) {
            return None;
        }
        let st = |v| match v {
            0 => Some(SlotState::Empty),
            1 => Some(SlotState::Trial),
            2 => Some(SlotState::Good),
            3 => Some(SlotState::Bad),
            _ => None,
        };
        let active = match b[8] {
            0 => Slot::A,
            1 => Slot::B,
            _ => return None,
        };
        Some(Record {
            generation: u32::from_le_bytes([b[4], b[5], b[6], b[7]]),
            active,
            state: [st(b[9])?, st(b[10])?],
            tries: [b[11], b[12]],
        })
    }

    /// Bootloader decision: which slot to boot now. Consumes one try from a Trial slot; marks it
    /// Bad when tries hit zero and falls back to the other slot if that is bootable.
    pub fn select_boot(&mut self) -> Option<Slot> {
        for _ in 0..2 {
            let s = self.active;
            match self.state[s.idx()] {
                SlotState::Good => return Some(s),
                SlotState::Trial if self.tries[s.idx()] > 0 => {
                    self.tries[s.idx()] -= 1;
                    return Some(s);
                }
                SlotState::Trial => {
                    self.state[s.idx()] = SlotState::Bad;
                    self.active = s.other();
                }
                _ => self.active = s.other(),
            }
        }
        None
    }
    /// The running OS confirms its slot is healthy.
    pub fn mark_good(&mut self, running: Slot) {
        self.state[running.idx()] = SlotState::Good;
        self.tries[running.idx()] = 0;
    }
    /// Begin an update: write the image to the inactive slot, then make it the trial slot.
    pub fn begin_update(&mut self) -> Result<Slot, SlotError> {
        let target = self.active.other();
        if self.state[self.active.idx()] != SlotState::Good {
            return Err(SlotError::IllegalTransition); // never update from an unconfirmed slot
        }
        self.state[target.idx()] = SlotState::Trial;
        self.tries[target.idx()] = MAX_TRIES;
        self.active = target;
        Ok(target)
    }
    /// Operator/automatic rollback to the other slot if it is Good.
    pub fn rollback(&mut self) -> Result<Slot, SlotError> {
        let o = self.active.other();
        if self.state[o.idx()] != SlotState::Good {
            return Err(SlotError::IllegalTransition);
        }
        self.state[self.active.idx()] = SlotState::Bad;
        self.active = o;
        Ok(o)
    }
}

/// Two-copy store over caller-provided storage.
pub struct Store<'a> {
    /// Backing bytes: two consecutive 32-byte copies.
    pub bytes: &'a mut [u8; 2 * LEN],
}

impl Store<'_> {
    fn copy(&self, i: usize) -> Option<Record> {
        let mut a = [0u8; LEN];
        a.copy_from_slice(&self.bytes[i * LEN..(i + 1) * LEN]);
        Record::decode(&a)
    }
    /// Load the valid copy with the highest generation.
    pub fn load(&self) -> Result<Record, SlotError> {
        match (self.copy(0), self.copy(1)) {
            (Some(a), Some(b)) => Ok(if b.generation > a.generation { b } else { a }),
            (Some(a), None) => Ok(a),
            (None, Some(b)) => Ok(b),
            _ => Err(SlotError::NoValidRecord),
        }
    }
    /// Save `rec` (bumping generation) into the copy that does not hold the newest valid record.
    /// `limit` simulates a power cut after that many bytes (None = complete write).
    pub fn save_torn(&mut self, rec: &Record, limit: Option<usize>) {
        let newest_is_0 = match (self.copy(0), self.copy(1)) {
            (Some(a), Some(b)) => a.generation >= b.generation,
            (Some(_), None) => true,
            _ => false,
        };
        let target = if newest_is_0 { 1 } else { 0 };
        let mut r = *rec;
        r.generation = self
            .load()
            .map(|c| c.generation)
            .unwrap_or(0)
            .wrapping_add(1);
        let enc = r.encode();
        let n = limit.unwrap_or(LEN).min(LEN);
        self.bytes[target * LEN..target * LEN + n].copy_from_slice(&enc[..n]);
    }
    /// Complete save.
    pub fn save(&mut self, rec: &Record) {
        self.save_torn(rec, None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fresh() -> [u8; 64] {
        let mut b = [0u8; 64];
        Store { bytes: &mut b }.save(&Record::factory());
        b
    }
    #[test]
    fn encode_decode_round_trip_and_corruption() {
        let r = Record::factory();
        assert_eq!(Record::decode(&r.encode()), Some(r));
        for i in 0..LEN {
            let mut e = r.encode();
            e[i] ^= 0x40;
            assert_eq!(Record::decode(&e), None, "flip at {i}");
        }
    }
    #[test]
    fn factory_boots_a() {
        let mut r = Record::factory();
        assert_eq!(r.select_boot(), Some(Slot::A));
    }
    #[test]
    fn update_trial_then_confirm() {
        let mut r = Record::factory();
        assert_eq!(r.begin_update(), Ok(Slot::B));
        assert_eq!(r.select_boot(), Some(Slot::B));
        r.mark_good(Slot::B);
        assert_eq!(r.select_boot(), Some(Slot::B));
        assert_eq!(r.state, [SlotState::Good, SlotState::Good]);
    }
    #[test]
    fn unconfirmed_trial_falls_back_after_max_tries() {
        let mut r = Record::factory();
        r.begin_update().unwrap();
        for _ in 0..MAX_TRIES {
            assert_eq!(r.select_boot(), Some(Slot::B));
        }
        assert_eq!(
            r.select_boot(),
            Some(Slot::A),
            "must roll back to the good slot"
        );
        assert_eq!(r.state[1], SlotState::Bad);
    }
    #[test]
    fn cannot_update_from_unconfirmed_slot_or_rollback_to_bad() {
        let mut r = Record::factory();
        r.begin_update().unwrap(); // B is Trial, active
        assert_eq!(r.begin_update(), Err(SlotError::IllegalTransition));
        assert_eq!(r.rollback(), Ok(Slot::A));
        assert_eq!(r.rollback(), Err(SlotError::IllegalTransition)); // B is now Bad
    }
    #[test]
    fn nothing_bootable_returns_none() {
        let mut r = Record {
            generation: 1,
            active: Slot::A,
            state: [SlotState::Bad, SlotState::Empty],
            tries: [0, 0],
        };
        assert_eq!(r.select_boot(), None);
    }
    #[test]
    fn power_loss_at_every_byte_never_loses_a_valid_record() {
        for cut in 0..=LEN {
            let mut b = fresh();
            let before = Store { bytes: &mut b }.load().unwrap();
            let mut next = before;
            next.begin_update().unwrap();
            Store { bytes: &mut b }.save_torn(&next, Some(cut));
            let got = Store { bytes: &mut b }.load().unwrap();
            if cut == LEN {
                assert_eq!(
                    (got.active, got.generation),
                    (Slot::B, before.generation + 1)
                );
            } else {
                assert_eq!(got, before, "torn write at {cut} corrupted state");
            }
        }
    }
    #[test]
    fn generations_alternate_copies_and_newest_wins() {
        let mut b = fresh();
        let mut r = Store { bytes: &mut b }.load().unwrap();
        for _ in 0..5 {
            Store { bytes: &mut b }.save(&r);
            r = Store { bytes: &mut b }.load().unwrap();
        }
        assert_eq!(r.generation, 6);
    }
    #[test]
    fn both_copies_destroyed_is_reported_not_guessed() {
        let mut b = [0xFFu8; 64];
        assert_eq!(
            Store { bytes: &mut b }.load(),
            Err(SlotError::NoValidRecord)
        );
    }
}
