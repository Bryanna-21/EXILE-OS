//! IPC message format and wire encoding (Mission B3). SPDX-License-Identifier: Apache-2.0
//!
//! A message is a label, up to [`MAX_WORDS`] data words and at most one capability. It is a
//! small fixed-size value (no allocation, `Copy`), so queues and hand-off need no buffers.
//!
//! # Wire format (little endian, user memory)
//! Send buffer, [`WIRE_IN`] = 56 bytes: `label, len, words[4], cap`. Receive buffer,
//! [`WIRE_OUT`] = 64 bytes: the same fields followed by `flags`. `len` is the number of used
//! words (0..=4); unused words **must** be zero on send (so the format has exactly one encoding
//! per message and padding cannot carry hidden data) and are zero on receive. `cap` is a
//! capability handle in the sender's space (0 = none); on receive it is a handle in the
//! receiver's space. `flags` bit 0 ([`FLAG_CAP_LOST`]) says a capability was sent but could
//! not be delivered (revoked in flight, or the receiver's table was full).
#![allow(dead_code)]

use super::cap_space::CapRef;

pub const MAX_WORDS: usize = 4;
pub const WIRE_IN: usize = 8 * (2 + MAX_WORDS + 1);
pub const WIRE_OUT: usize = WIRE_IN + 8;
pub const FLAG_CAP_LOST: u64 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MsgError {
    /// `len` greater than [`MAX_WORDS`].
    TooLong,
    /// A word beyond `len` is non-zero.
    DirtyPadding,
}

/// A message as supplied by a sender, before any capability has been resolved.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct UserMessage {
    pub label: u64,
    pub len: u8,
    pub words: [u64; MAX_WORDS],
    /// Capability handle in the sender's space.
    pub cap: Option<u64>,
}

impl UserMessage {
    pub fn new(label: u64, data: &[u64], cap: Option<u64>) -> Result<Self, MsgError> {
        if data.len() > MAX_WORDS {
            return Err(MsgError::TooLong);
        }
        let mut words = [0; MAX_WORDS];
        words[..data.len()].copy_from_slice(data);
        Ok(Self {
            label,
            len: data.len() as u8,
            words,
            cap,
        })
    }

    pub fn data(&self) -> &[u64] {
        &self.words[..self.len as usize]
    }

    pub fn validate(&self) -> Result<(), MsgError> {
        if self.len as usize > MAX_WORDS {
            return Err(MsgError::TooLong);
        }
        if self.words[self.len as usize..].iter().any(|&w| w != 0) {
            return Err(MsgError::DirtyPadding);
        }
        Ok(())
    }

    pub fn decode(b: &[u8; WIRE_IN]) -> Result<Self, MsgError> {
        let w = |i: usize| u64::from_le_bytes(b[i * 8..i * 8 + 8].try_into().unwrap());
        let len = w(1);
        if len > MAX_WORDS as u64 {
            return Err(MsgError::TooLong);
        }
        let mut words = [0; MAX_WORDS];
        for (i, slot) in words.iter_mut().enumerate() {
            *slot = w(2 + i);
        }
        let cap = w(2 + MAX_WORDS);
        let m = Self {
            label: w(0),
            len: len as u8,
            words,
            cap: if cap == 0 { None } else { Some(cap) },
        };
        m.validate()?;
        Ok(m)
    }

    pub fn encode(&self) -> [u8; WIRE_IN] {
        let mut b = [0u8; WIRE_IN];
        let mut put = |i: usize, v: u64| b[i * 8..i * 8 + 8].copy_from_slice(&v.to_le_bytes());
        put(0, self.label);
        put(1, self.len as u64);
        for (i, w) in self.words.iter().enumerate() {
            put(2 + i, *w);
        }
        put(2 + MAX_WORDS, self.cap.unwrap_or(0));
        b
    }
}

/// A message inside the kernel: the capability, if any, lives in limbo as a [`CapRef`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Message {
    pub label: u64,
    pub len: u8,
    pub words: [u64; MAX_WORDS],
    pub cap: Option<CapRef>,
}

/// A message as handed to a receiver.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Delivery {
    pub label: u64,
    pub len: u8,
    pub words: [u64; MAX_WORDS],
    /// Handle in the receiver's space.
    pub cap: Option<u64>,
    pub cap_lost: bool,
}

impl Delivery {
    pub fn data(&self) -> &[u64] {
        &self.words[..self.len as usize]
    }

    pub fn encode(&self) -> [u8; WIRE_OUT] {
        let mut b = [0u8; WIRE_OUT];
        let m = UserMessage {
            label: self.label,
            len: self.len,
            words: self.words,
            cap: self.cap,
        };
        b[..WIRE_IN].copy_from_slice(&m.encode());
        let flags = if self.cap_lost { FLAG_CAP_LOST } else { 0 };
        b[WIRE_IN..].copy_from_slice(&flags.to_le_bytes());
        b
    }

    pub fn decode(b: &[u8; WIRE_OUT]) -> Result<Self, MsgError> {
        let m = UserMessage::decode(b[..WIRE_IN].try_into().unwrap())?;
        let flags = u64::from_le_bytes(b[WIRE_IN..].try_into().unwrap());
        Ok(Self {
            label: m.label,
            len: m.len,
            words: m.words,
            cap: m.cap,
            cap_lost: flags & FLAG_CAP_LOST != 0,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_match_the_documented_layout() {
        assert_eq!(WIRE_IN, 56);
        assert_eq!(WIRE_OUT, 64);
    }

    #[test]
    fn new_pads_with_zero_and_rejects_oversize() {
        let m = UserMessage::new(7, &[1, 2], Some(9)).unwrap();
        assert_eq!(m.data(), &[1, 2]);
        assert_eq!(m.words, [1, 2, 0, 0]);
        assert_eq!(m.validate(), Ok(()));
        assert_eq!(UserMessage::new(0, &[0; 5], None), Err(MsgError::TooLong));
        assert_eq!(UserMessage::new(0, &[1; 4], None).unwrap().data().len(), 4);
        assert_eq!(UserMessage::new(0, &[], None).unwrap().data().len(), 0);
    }

    #[test]
    fn wire_round_trip_for_every_length() {
        for len in 0..=MAX_WORDS {
            let data: [u64; 4] = [u64::MAX, 0x0123_4567_89AB_CDEF, 3, u64::MAX - 1];
            let m = UserMessage::new(0xDEAD_BEEF_0000_0001, &data[..len], Some(0x1_0002)).unwrap();
            assert_eq!(UserMessage::decode(&m.encode()), Ok(m));
            let none = UserMessage::new(1, &data[..len], None).unwrap();
            assert_eq!(UserMessage::decode(&none.encode()), Ok(none));
        }
    }

    #[test]
    fn decode_is_little_endian_with_the_documented_offsets() {
        let m = UserMessage::new(0x0102, &[0x0A], Some(0x0B)).unwrap();
        let b = m.encode();
        assert_eq!(&b[0..8], &[2, 1, 0, 0, 0, 0, 0, 0]);
        assert_eq!(b[8], 1, "len");
        assert_eq!(b[16], 0x0A, "words[0]");
        assert_eq!(b[48], 0x0B, "cap");
    }

    #[test]
    fn decode_rejects_bad_len_and_dirty_padding() {
        let mut b = UserMessage::new(1, &[5], None).unwrap().encode();
        b[8] = 5;
        assert_eq!(UserMessage::decode(&b), Err(MsgError::TooLong));
        b[8..16].copy_from_slice(&u64::MAX.to_le_bytes());
        assert_eq!(UserMessage::decode(&b), Err(MsgError::TooLong), "huge len");
        b[8..16].copy_from_slice(&1u64.to_le_bytes());
        b[24] = 1; // words[1], beyond len
        assert_eq!(UserMessage::decode(&b), Err(MsgError::DirtyPadding));
        let dirty = UserMessage {
            len: 0,
            words: [0, 0, 0, 1],
            ..UserMessage::default()
        };
        assert_eq!(dirty.validate(), Err(MsgError::DirtyPadding));
        let long = UserMessage {
            len: 9,
            ..UserMessage::default()
        };
        assert_eq!(long.validate(), Err(MsgError::TooLong));
    }

    #[test]
    fn delivery_encoding_carries_flags_and_round_trips() {
        let d = Delivery {
            label: 3,
            len: 2,
            words: [7, 8, 0, 0],
            cap: Some(0x2_0003),
            cap_lost: false,
        };
        assert_eq!(Delivery::decode(&d.encode()), Ok(d));
        let lost = Delivery {
            cap: None,
            cap_lost: true,
            ..d
        };
        let b = lost.encode();
        assert_eq!(b[WIRE_IN], 1);
        assert_eq!(Delivery::decode(&b), Ok(lost));
        assert_eq!(
            &d.encode()[..WIRE_IN],
            &UserMessage::new(3, &[7, 8], Some(0x2_0003))
                .unwrap()
                .encode()[..]
        );
        assert_eq!(d.data(), &[7, 8]);
    }
}
