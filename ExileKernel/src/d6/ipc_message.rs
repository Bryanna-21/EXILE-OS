//! IPC message representation and validation (Mission D6, file 5).
//! SPDX-License-Identifier: Apache-2.0
//!
//! # Wire format (version 1, little endian)
//! ```text
//! offset size field
//!      0    1 version        must be MSG_VERSION
//!      1    1 kind           1 Data, 2 Request, 3 Response, 4 Notification, 5 Error
//!      2    2 flags          bit0 MOVE_CAPS, bit1 NEEDS_REPLY; all other bits must be 0
//!      4    2 payload_len    <= MAX_PAYLOAD
//!      6    1 transfer_count <= MAX_TRANSFERS
//!      7    1 reserved       must be 0
//!      8    8 tag            opaque correlation value chosen by the sender
//!     16  8*n transfers      { handle: u32, rights: u32 } per capability to transfer
//!      .  len payload        exactly payload_len bytes, nothing may follow
//! ```
//! Parsing is total: any byte string either yields a valid [`Message`] or a specific [`Error`]; it
//! never panics, never reads outside the slice, and never trusts a length field before checking it
//! against both the format limit and the bytes actually present. All size arithmetic is checked.
//!
//! # What is *not* on the wire
//! The sender identity and the message id are **never** taken from user bytes. The kernel stamps
//! `sender` (the owner of the handle table the send came through) and `id` (a per-channel
//! sequence number) when it accepts the message. A user cannot impersonate another process by
//! writing a field, because there is no field to write.
//!
//! # Capability-transfer metadata
//! A sender lists `(handle, rights)` pairs. `handle` is a handle in the *sender's* table and is only
//! ever resolved there; `rights` is the maximum the sender wants to delegate (the kernel
//! intersects it with what the sender actually holds, see `ipc_transfer.rs`). After the kernel
//! stages a transfer, the message carries the staged capability ids (`staged`) internally; on
//! delivery the `handle` fields are rewritten to the receiver's new handle values and `rights` to
//! the rights actually granted.
#![allow(dead_code)]

use crate::d6::capability::{CapId, Rights};
use crate::d6::object::{Error, Owner};

pub const MSG_VERSION: u8 = 1;
pub const MAX_PAYLOAD: usize = 128;
pub const MAX_TRANSFERS: usize = 4;
pub const HEADER_LEN: usize = 16;
pub const TRANSFER_LEN: usize = 8;
pub const MAX_WIRE_LEN: usize = HEADER_LEN + MAX_TRANSFERS * TRANSFER_LEN + MAX_PAYLOAD;

/// Message class. `KernelNotice` is reserved for messages the kernel itself posts (for example
/// "driver terminated") and can never be sent by a process.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum MsgKind {
    Data = 1,
    Request = 2,
    Response = 3,
    Notification = 4,
    Error = 5,
    KernelNotice = 0x80,
}

impl MsgKind {
    /// Validates a kind byte coming from user space.
    pub const fn from_user(b: u8) -> Result<MsgKind, Error> {
        match b {
            1 => Ok(MsgKind::Data),
            2 => Ok(MsgKind::Request),
            3 => Ok(MsgKind::Response),
            4 => Ok(MsgKind::Notification),
            5 => Ok(MsgKind::Error),
            _ => Err(Error::BadKind),
        }
    }
    pub const fn is_user_sendable(self) -> bool {
        !matches!(self, MsgKind::KernelNotice)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MsgFlags(u16);

impl MsgFlags {
    pub const NONE: MsgFlags = MsgFlags(0);
    /// Transferred capabilities are moved: the sender's handles are closed on success.
    pub const MOVE_CAPS: MsgFlags = MsgFlags(1 << 0);
    /// The sender expects a response.
    pub const NEEDS_REPLY: MsgFlags = MsgFlags(1 << 1);
    const ALL: u16 = 0b11;

    pub const fn from_bits(bits: u16) -> Result<MsgFlags, Error> {
        if bits & !Self::ALL != 0 {
            Err(Error::BadFlags)
        } else {
            Ok(MsgFlags(bits))
        }
    }
    pub const fn bits(self) -> u16 {
        self.0
    }
    pub const fn contains(self, o: MsgFlags) -> bool {
        self.0 & o.0 == o.0
    }
    pub const fn union(self, o: MsgFlags) -> MsgFlags {
        MsgFlags(self.0 | o.0)
    }
}

/// Kernel-assigned, per-channel, strictly increasing message sequence number.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct MsgId(pub u64);

/// One entry of a sender's transfer list.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TransferRequest {
    pub handle: u32,
    pub rights: Rights,
}

const NO_TRANSFER: TransferRequest = TransferRequest {
    handle: 0,
    rights: Rights::NONE,
};

/// Header fields as they appear on the wire.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Header {
    pub kind: MsgKind,
    pub flags: MsgFlags,
    pub payload_len: usize,
    pub transfer_count: usize,
    pub tag: u64,
}

/// `HEADER_LEN + TRANSFER_LEN * transfers + payload`, overflow-checked.
pub fn encoded_len(payload_len: usize, transfer_count: usize) -> Result<usize, Error> {
    let t = transfer_count
        .checked_mul(TRANSFER_LEN)
        .ok_or(Error::SizeOverflow)?;
    HEADER_LEN
        .checked_add(t)
        .and_then(|n| n.checked_add(payload_len))
        .ok_or(Error::SizeOverflow)
}

fn le16(b: &[u8], at: usize) -> Result<u16, Error> {
    let s = b.get(at..at.checked_add(2).ok_or(Error::SizeOverflow)?);
    s.map(|s| u16::from_le_bytes([s[0], s[1]])).ok_or(Error::Truncated)
}
fn le32(b: &[u8], at: usize) -> Result<u32, Error> {
    let s = b.get(at..at.checked_add(4).ok_or(Error::SizeOverflow)?);
    s.map(|s| u32::from_le_bytes([s[0], s[1], s[2], s[3]])).ok_or(Error::Truncated)
}
fn le64(b: &[u8], at: usize) -> Result<u64, Error> {
    let s = b.get(at..at.checked_add(8).ok_or(Error::SizeOverflow)?);
    s.map(|s| u64::from_le_bytes([s[0], s[1], s[2], s[3], s[4], s[5], s[6], s[7]]))
        .ok_or(Error::Truncated)
}

impl Header {
    /// Parses and validates the fixed header at the start of `bytes`.
    pub fn decode(bytes: &[u8]) -> Result<Header, Error> {
        if bytes.len() < HEADER_LEN {
            return Err(Error::Truncated);
        }
        if bytes[0] != MSG_VERSION {
            return Err(Error::BadVersion);
        }
        let kind = MsgKind::from_user(bytes[1])?;
        let flags = MsgFlags::from_bits(le16(bytes, 2)?)?;
        let payload_len = le16(bytes, 4)? as usize;
        let transfer_count = bytes[6] as usize;
        if bytes[7] != 0 {
            return Err(Error::ReservedNonZero);
        }
        if payload_len > MAX_PAYLOAD {
            return Err(Error::PayloadTooLarge);
        }
        if transfer_count > MAX_TRANSFERS {
            return Err(Error::TooManyTransfers);
        }
        if flags.contains(MsgFlags::MOVE_CAPS) && transfer_count == 0 {
            return Err(Error::BadFlags);
        }
        Ok(Header {
            kind,
            flags,
            payload_len,
            transfer_count,
            tag: le64(bytes, 8)?,
        })
    }
}

/// A validated message. `Copy` and fixed-size: queues hold these by value, so a full queue never
/// needs an allocation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Message {
    pub id: MsgId,
    /// Kernel-stamped. Never read from user bytes.
    pub sender: Owner,
    pub kind: MsgKind,
    pub flags: MsgFlags,
    pub tag: u64,
    payload_len: u16,
    payload: [u8; MAX_PAYLOAD],
    transfer_count: u8,
    requests: [TransferRequest; MAX_TRANSFERS],
    /// Capabilities staged for delivery (filled by `ipc_transfer`).
    pub(crate) staged: [Option<CapId>; MAX_TRANSFERS],
}

impl Message {
    /// Builds a message from trusted kernel code (payload still length-checked).
    pub fn new(
        sender: Owner,
        kind: MsgKind,
        flags: MsgFlags,
        tag: u64,
        payload: &[u8],
    ) -> Result<Message, Error> {
        if payload.len() > MAX_PAYLOAD {
            return Err(Error::PayloadTooLarge);
        }
        if flags.contains(MsgFlags::MOVE_CAPS) {
            return Err(Error::BadFlags);
        }
        let mut buf = [0u8; MAX_PAYLOAD];
        buf[..payload.len()].copy_from_slice(payload);
        Ok(Message {
            id: MsgId(0),
            sender,
            kind,
            flags,
            tag,
            payload_len: payload.len() as u16,
            payload: buf,
            transfer_count: 0,
            requests: [NO_TRANSFER; MAX_TRANSFERS],
            staged: [None; MAX_TRANSFERS],
        })
    }

    /// Attaches a transfer list. Every entry is validated: a zero handle is an invalid
    /// capability reference, and duplicates are refused here so no later stage sees them.
    pub fn with_transfers(mut self, list: &[TransferRequest]) -> Result<Message, Error> {
        if list.len() > MAX_TRANSFERS {
            return Err(Error::TooManyTransfers);
        }
        for (i, t) in list.iter().enumerate() {
            if t.handle == 0 {
                return Err(Error::BadHandle);
            }
            if list[..i].iter().any(|u| u.handle == t.handle) {
                return Err(Error::DuplicateTransfer);
            }
        }
        self.requests = [NO_TRANSFER; MAX_TRANSFERS];
        self.requests[..list.len()].copy_from_slice(list);
        self.transfer_count = list.len() as u8;
        Ok(self)
    }

    /// Marks the transfers as moves (the sender's handles are closed after queueing). Needs at
    /// least one transfer.
    pub fn with_move(mut self) -> Result<Message, Error> {
        if self.transfer_count == 0 {
            return Err(Error::BadFlags);
        }
        self.flags = self.flags.union(MsgFlags::MOVE_CAPS);
        Ok(self)
    }

    /// Parses an untrusted byte string. `sender` is supplied by the kernel from the identity of
    /// the calling process.
    pub fn from_wire(sender: Owner, bytes: &[u8]) -> Result<Message, Error> {
        let h = Header::decode(bytes)?;
        let total = encoded_len(h.payload_len, h.transfer_count)?;
        if bytes.len() < total {
            return Err(Error::Truncated);
        }
        if bytes.len() > total {
            return Err(Error::TrailingBytes);
        }
        let mut list = [NO_TRANSFER; MAX_TRANSFERS];
        for (i, slot) in list.iter_mut().enumerate().take(h.transfer_count) {
            let at = HEADER_LEN + i * TRANSFER_LEN;
            *slot = TransferRequest {
                handle: le32(bytes, at)?,
                rights: Rights::from_bits(le32(bytes, at + 4)?)?,
            };
        }
        let pay_at = HEADER_LEN + h.transfer_count * TRANSFER_LEN;
        let payload = bytes.get(pay_at..total).ok_or(Error::Truncated)?;
        Message::new(
            sender,
            h.kind,
            MsgFlags::from_bits(h.flags.bits() & !MsgFlags::MOVE_CAPS.bits())?,
            h.tag,
            payload,
        )
        .and_then(|mut m| {
            m.flags = h.flags;
            m.with_transfers(&list[..h.transfer_count])
        })
    }

    pub fn payload(&self) -> &[u8] {
        &self.payload[..self.payload_len as usize]
    }

    pub fn transfers(&self) -> &[TransferRequest] {
        &self.requests[..self.transfer_count as usize]
    }

    pub fn transfer_count(&self) -> usize {
        self.transfer_count as usize
    }

    pub fn wire_len(&self) -> usize {
        HEADER_LEN + TRANSFER_LEN * self.transfer_count as usize + self.payload_len as usize
    }

    /// Writes the wire form into `out`; fails without writing if `out` is too small.
    pub fn encode(&self, out: &mut [u8]) -> Result<usize, Error> {
        let n = encoded_len(self.payload_len as usize, self.transfer_count as usize)?;
        let out = out.get_mut(..n).ok_or(Error::BufferTooSmall)?;
        out[0] = MSG_VERSION;
        out[1] = self.kind as u8;
        out[2..4].copy_from_slice(&self.flags.bits().to_le_bytes());
        out[4..6].copy_from_slice(&self.payload_len.to_le_bytes());
        out[6] = self.transfer_count;
        out[7] = 0;
        out[8..16].copy_from_slice(&self.tag.to_le_bytes());
        for (i, t) in self.transfers().iter().enumerate() {
            let at = HEADER_LEN + i * TRANSFER_LEN;
            out[at..at + 4].copy_from_slice(&t.handle.to_le_bytes());
            out[at + 4..at + 8].copy_from_slice(&t.rights.bits().to_le_bytes());
        }
        let pay_at = HEADER_LEN + self.transfer_count as usize * TRANSFER_LEN;
        out[pay_at..n].copy_from_slice(self.payload());
        Ok(n)
    }

    /// Rewrites transfer entry `i` to what the receiver actually got (delivery step).
    pub(crate) fn set_delivered(&mut self, i: usize, handle: u32, rights: Rights) {
        if i < self.transfer_count as usize {
            self.requests[i] = TransferRequest { handle, rights };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::vec::Vec;

    fn p(n: u64) -> Owner {
        Owner::process(n).unwrap()
    }

    fn wire(kind: u8, flags: u16, payload: &[u8], xfers: &[(u32, u32)], tag: u64) -> Vec<u8> {
        let mut v = Vec::new();
        v.push(MSG_VERSION);
        v.push(kind);
        v.extend_from_slice(&flags.to_le_bytes());
        v.extend_from_slice(&(payload.len() as u16).to_le_bytes());
        v.push(xfers.len() as u8);
        v.push(0);
        v.extend_from_slice(&tag.to_le_bytes());
        for (h, r) in xfers {
            v.extend_from_slice(&h.to_le_bytes());
            v.extend_from_slice(&r.to_le_bytes());
        }
        v.extend_from_slice(payload);
        v
    }

    #[test]
    fn minimal_and_maximal_messages_round_trip() {
        let w = wire(1, 0, b"", &[], 0);
        let m = Message::from_wire(p(1), &w).unwrap();
        assert_eq!(m.payload(), b"");
        assert_eq!(m.sender, p(1));
        let mut out = [0u8; MAX_WIRE_LEN];
        assert_eq!(m.encode(&mut out), Ok(HEADER_LEN));
        assert_eq!(&out[..HEADER_LEN], &w[..]);

        let payload = [0xABu8; MAX_PAYLOAD];
        let xs: Vec<(u32, u32)> = (1..=MAX_TRANSFERS as u32).map(|h| (h, Rights::SEND.bits())).collect();
        let w = wire(2, 0b11, &payload, &xs, u64::MAX);
        assert_eq!(w.len(), MAX_WIRE_LEN);
        let m = Message::from_wire(p(9), &w).unwrap();
        assert_eq!(m.kind, MsgKind::Request);
        assert_eq!(m.tag, u64::MAX);
        assert_eq!(m.transfer_count(), MAX_TRANSFERS);
        assert!(m.flags.contains(MsgFlags::MOVE_CAPS) && m.flags.contains(MsgFlags::NEEDS_REPLY));
        let mut out = [0u8; MAX_WIRE_LEN];
        assert_eq!(m.encode(&mut out), Ok(MAX_WIRE_LEN));
        assert_eq!(&out[..], &w[..]);
    }

    #[test]
    fn rejects_every_malformed_header() {
        let ok = wire(1, 0, b"hi", &[], 7);
        assert!(Message::from_wire(p(1), &ok).is_ok());
        for n in 0..HEADER_LEN {
            assert_eq!(Message::from_wire(p(1), &ok[..n]).err(), Some(Error::Truncated), "len {n}");
        }
        let mut b = ok.clone();
        b[0] = 2;
        assert_eq!(Message::from_wire(p(1), &b).err(), Some(Error::BadVersion));
        b[0] = 0;
        assert_eq!(Message::from_wire(p(1), &b).err(), Some(Error::BadVersion));
        for k in [0u8, 6, 0x7F, 0x80, 0xFF] {
            let b = wire(k, 0, b"", &[], 0);
            assert_eq!(Message::from_wire(p(1), &b).err(), Some(Error::BadKind), "kind {k:#x}");
        }
        for f in [4u16, 0x8000, 0xFFFF] {
            let b = wire(1, f, b"", &[], 0);
            assert_eq!(Message::from_wire(p(1), &b).err(), Some(Error::BadFlags));
        }
        let mut b = ok.clone();
        b[7] = 1;
        assert_eq!(Message::from_wire(p(1), &b).err(), Some(Error::ReservedNonZero));
    }

    #[test]
    fn rejects_bad_lengths_and_trailing_bytes() {
        let big = [0u8; MAX_PAYLOAD + 1];
        assert_eq!(
            Message::from_wire(p(1), &wire(1, 0, &big, &[], 0)).err(),
            Some(Error::PayloadTooLarge)
        );
        let mut b = wire(1, 0, b"abcd", &[], 0);
        assert_eq!(Message::from_wire(p(1), &b[..b.len() - 1]).err(), Some(Error::Truncated));
        b.push(0);
        assert_eq!(Message::from_wire(p(1), &b).err(), Some(Error::TrailingBytes));
        // A header that claims 0xFFFF payload bytes is refused before any read.
        let mut b = wire(1, 0, b"", &[], 0);
        b[4] = 0xFF;
        b[5] = 0xFF;
        assert_eq!(Message::from_wire(p(1), &b).err(), Some(Error::PayloadTooLarge));
        // Transfer count beyond the maximum.
        let mut b = wire(1, 0, b"", &[], 0);
        b[6] = (MAX_TRANSFERS + 1) as u8;
        assert_eq!(Message::from_wire(p(1), &b).err(), Some(Error::TooManyTransfers));
        b[6] = 255;
        assert_eq!(Message::from_wire(p(1), &b).err(), Some(Error::TooManyTransfers));
        // Claimed transfers that are not present.
        let mut b = wire(1, 0, b"", &[], 0);
        b[6] = 2;
        assert_eq!(Message::from_wire(p(1), &b).err(), Some(Error::Truncated));
    }

    #[test]
    fn size_arithmetic_is_overflow_safe() {
        assert_eq!(encoded_len(0, 0), Ok(HEADER_LEN));
        assert_eq!(encoded_len(usize::MAX, 0), Err(Error::SizeOverflow));
        assert_eq!(encoded_len(0, usize::MAX), Err(Error::SizeOverflow));
        assert_eq!(encoded_len(usize::MAX - HEADER_LEN, 1), Err(Error::SizeOverflow));
        assert_eq!(encoded_len(usize::MAX - HEADER_LEN, 0), Ok(usize::MAX));
        assert_eq!(le32(&[0; 8], usize::MAX), Err(Error::SizeOverflow));
        assert_eq!(le64(&[0; 8], 1), Err(Error::Truncated));
    }

    #[test]
    fn transfer_metadata_is_validated() {
        // Unknown rights bits.
        let b = wire(1, 0, b"", &[(1, 1 << 20)], 0);
        assert_eq!(Message::from_wire(p(1), &b).err(), Some(Error::UnknownRights));
        // Handle zero is not a capability reference.
        let b = wire(1, 0, b"", &[(0, Rights::SEND.bits())], 0);
        assert_eq!(Message::from_wire(p(1), &b).err(), Some(Error::BadHandle));
        // The same handle twice.
        let b = wire(1, 0, b"", &[(3, 1), (3, 2)], 0);
        assert_eq!(Message::from_wire(p(1), &b).err(), Some(Error::DuplicateTransfer));
        // MOVE_CAPS without transfers.
        let b = wire(1, 1, b"", &[], 0);
        assert_eq!(Message::from_wire(p(1), &b).err(), Some(Error::BadFlags));
        // Valid.
        let b = wire(1, 1, b"x", &[(3, 1), (4, 2)], 0);
        let m = Message::from_wire(p(1), &b).unwrap();
        assert_eq!(m.transfers().len(), 2);
        assert_eq!(m.transfers()[1].handle, 4);
    }

    #[test]
    fn sender_identity_is_not_user_controlled() {
        // Whatever the bytes contain, the sender is the kernel-supplied owner.
        let mut b = wire(1, 0, b"", &[], 0);
        for i in 0..HEADER_LEN {
            let keep = b[i];
            for v in [0u8, 1, 0xFF] {
                b[i] = v;
                if let Ok(m) = Message::from_wire(p(5), &b) {
                    assert_eq!(m.sender, p(5));
                    assert_eq!(m.id, MsgId(0), "id is assigned by the channel, not the wire");
                }
            }
            b[i] = keep;
        }
    }

    #[test]
    fn kernel_notice_cannot_be_sent_by_users() {
        assert!(!MsgKind::KernelNotice.is_user_sendable());
        assert_eq!(MsgKind::from_user(0x80), Err(Error::BadKind));
        // The kernel itself may construct one.
        let m = Message::new(Owner::KERNEL, MsgKind::KernelNotice, MsgFlags::NONE, 3, b"gone").unwrap();
        assert_eq!(m.sender, Owner::KERNEL);
        // ...but it still cannot be encoded to a form that parses back as a user message.
        let mut out = [0u8; 64];
        let n = m.encode(&mut out).unwrap();
        assert_eq!(Message::from_wire(p(1), &out[..n]).err(), Some(Error::BadKind));
    }

    #[test]
    fn encode_never_writes_partially() {
        let m = Message::new(p(1), MsgKind::Data, MsgFlags::NONE, 1, b"hello").unwrap();
        let mut small = [0xEEu8; HEADER_LEN + 4];
        assert_eq!(m.encode(&mut small), Err(Error::BufferTooSmall));
        assert!(small.iter().all(|&b| b == 0xEE));
        let mut exact = [0u8; HEADER_LEN + 5];
        assert_eq!(m.encode(&mut exact), Ok(HEADER_LEN + 5));
        assert_eq!(m.wire_len(), HEADER_LEN + 5);
    }

    #[test]
    fn builder_rejects_oversize_and_bad_transfer_lists() {
        let big = [0u8; MAX_PAYLOAD + 1];
        assert_eq!(
            Message::new(p(1), MsgKind::Data, MsgFlags::NONE, 0, &big).err(),
            Some(Error::PayloadTooLarge)
        );
        assert!(Message::new(p(1), MsgKind::Data, MsgFlags::NONE, 0, &big[..MAX_PAYLOAD]).is_ok());
        let m = Message::new(p(1), MsgKind::Data, MsgFlags::NONE, 0, b"").unwrap();
        let t = |h| TransferRequest { handle: h, rights: Rights::SEND };
        assert_eq!(m.with_transfers(&[t(1); 5]).err(), Some(Error::TooManyTransfers));
        assert_eq!(m.with_transfers(&[t(0)]).err(), Some(Error::BadHandle));
        assert_eq!(m.with_transfers(&[t(2), t(2)]).err(), Some(Error::DuplicateTransfer));
        assert_eq!(m.with_transfers(&[t(1), t(2)]).unwrap().transfer_count(), 2);
        // MOVE_CAPS is only valid via a transfer list, not on a bare kernel message.
        assert_eq!(
            Message::new(p(1), MsgKind::Data, MsgFlags::MOVE_CAPS, 0, b"").err(),
            Some(Error::BadFlags)
        );
    }

    /// Deterministic fuzz with a recorded seed: arbitrary bytes never panic, and whatever parses
    /// re-encodes to exactly the same bytes.
    #[test]
    fn random_bytes_never_panic_and_valid_ones_round_trip() {
        const SEED: u64 = 0x6D69_7373_696F_6E36;
        let mut x = SEED;
        let mut next = move || {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            x
        };
        let mut valid = 0;
        for _ in 0..20_000 {
            let len = (next() % (MAX_WIRE_LEN as u64 + 8)) as usize;
            let mut b: Vec<u8> = (0..len).map(|_| next() as u8).collect();
            // Bias half of the inputs towards a plausible header.
            if len >= HEADER_LEN && next() % 2 == 0 {
                b[0] = MSG_VERSION;
                b[1] = (next() % 6) as u8;
                b[2] = (next() % 4) as u8;
                b[3] = 0;
                b[5] = 0;
                b[6] = (next() % 5) as u8;
                b[7] = 0;
                let n = encoded_len(b[4] as usize % (MAX_PAYLOAD + 1), b[6] as usize).unwrap();
                b[4] = (n - HEADER_LEN - b[6] as usize * TRANSFER_LEN).min(MAX_PAYLOAD) as u8;
                b.resize(encoded_len(b[4] as usize, b[6] as usize).unwrap(), 0);
            }
            if let Ok(m) = Message::from_wire(p(1), &b) {
                valid += 1;
                let mut out = [0u8; MAX_WIRE_LEN];
                let n = m.encode(&mut out).unwrap();
                assert_eq!(&out[..n], &b[..], "seed {SEED:#x}");
            }
        }
        assert!(valid > 50, "the fuzzer must reach valid messages (got {valid}, seed {SEED:#x})");
    }
}
