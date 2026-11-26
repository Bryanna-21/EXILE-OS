//! Capability rights (Mission B3, capability enforcement). SPDX-License-Identifier: Apache-2.0
//!
//! A [`Rights`] value is a small bit set that says what the holder of a capability may do with
//! the kernel object behind it. Rights are *monotonically decreasing* along any chain of
//! derivations: a derived capability can never carry a right its source did not have
//! ([`Rights::is_subset_of`], [`Rights::attenuate`]). There is no "all rights" ambient value:
//! each object [`Kind`] has its own set of meaningful rights ([`Rights::valid_for`]) and a
//! capability may only carry rights from that set.
//!
//! | Right | Endpoint | Notification |
//! |---|---|---|
//! | `SEND` / `RECV` | send / receive messages | not valid |
//! | `SIGNAL` / `WAIT` | not valid | OR bits into / wait for the signal word |
//! | `GRANT` | may derive copies for *other* owners (i.e. transfer in a message) | same |
//! | `REVOKE` | may revoke every capability derived from this one | same |
//!
//! Values that arrive from user space are untrusted 64-bit words; [`Rights::from_user`] rejects
//! unknown bits, empty sets and rights that do not belong to the object kind instead of masking
//! them away, so a caller that asks for something it cannot have learns about it.
#![allow(dead_code)]

/// Kind of kernel object a capability refers to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Endpoint,
    Notification,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RightsError {
    /// Bits outside the defined rights, or more than 16 bits supplied.
    Unknown,
    /// A capability must carry at least one right.
    Empty,
    /// A right that is not meaningful for the object kind (e.g. `SEND` on a notification).
    WrongKind,
}

/// Bit set of rights. The representation is private so an invalid value cannot be built outside
/// this module except through the checked constructors.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Hash)]
pub struct Rights(u16);

impl Rights {
    pub const NONE: Rights = Rights(0);
    pub const SEND: Rights = Rights(1 << 0);
    pub const RECV: Rights = Rights(1 << 1);
    pub const SIGNAL: Rights = Rights(1 << 2);
    pub const WAIT: Rights = Rights(1 << 3);
    pub const GRANT: Rights = Rights(1 << 4);
    pub const REVOKE: Rights = Rights(1 << 5);
    /// Every defined right (not a valid capability value: the kinds are disjoint).
    pub const DEFINED: Rights = Rights(0b11_1111);

    pub const fn bits(self) -> u16 {
        self.0
    }

    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }

    pub const fn contains(self, other: Rights) -> bool {
        self.0 & other.0 == other.0
    }

    pub const fn is_subset_of(self, other: Rights) -> bool {
        self.0 & !other.0 == 0
    }

    pub const fn union(self, other: Rights) -> Rights {
        Rights(self.0 | other.0)
    }

    pub const fn intersect(self, other: Rights) -> Rights {
        Rights(self.0 & other.0)
    }

    pub const fn difference(self, other: Rights) -> Rights {
        Rights(self.0 & !other.0)
    }

    /// The rights that make sense for `kind`.
    pub const fn valid_for(kind: Kind) -> Rights {
        match kind {
            Kind::Endpoint => Rights(Self::SEND.0 | Self::RECV.0 | Self::GRANT.0 | Self::REVOKE.0),
            Kind::Notification => {
                Rights(Self::SIGNAL.0 | Self::WAIT.0 | Self::GRANT.0 | Self::REVOKE.0)
            }
        }
    }

    /// The rights of the first (root) capability the kernel mints for a new object.
    pub const fn root_for(kind: Kind) -> Rights {
        Self::valid_for(kind)
    }

    /// Checked constructor from raw bits: only defined bits are accepted.
    pub const fn from_bits(bits: u16) -> Option<Rights> {
        if bits & !Self::DEFINED.0 == 0 {
            Some(Rights(bits))
        } else {
            None
        }
    }

    /// Checked constructor for untrusted input: defined bits only, non-empty, and every right
    /// must be valid for `kind`.
    pub fn from_user(bits: u64, kind: Kind) -> Result<Rights, RightsError> {
        let bits = u16::try_from(bits).map_err(|_| RightsError::Unknown)?;
        let r = Rights::from_bits(bits).ok_or(RightsError::Unknown)?;
        r.validate_for(kind)
    }

    /// `Ok(self)` when non-empty and a subset of [`Rights::valid_for`] `kind`.
    pub fn validate_for(self, kind: Kind) -> Result<Rights, RightsError> {
        if self.is_empty() {
            return Err(RightsError::Empty);
        }
        if !self.is_subset_of(Self::valid_for(kind)) {
            return Err(RightsError::WrongKind);
        }
        Ok(self)
    }

    /// The part of `self` that survives masking with `mask` (never grows).
    pub const fn attenuate(self, mask: Rights) -> Rights {
        self.intersect(mask)
    }
}

impl core::ops::BitOr for Rights {
    type Output = Rights;
    fn bitor(self, rhs: Rights) -> Rights {
        self.union(rhs)
    }
}

impl core::ops::BitAnd for Rights {
    type Output = Rights;
    fn bitand(self, rhs: Rights) -> Rights {
        self.intersect(rhs)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL: [Rights; 6] = [
        Rights::SEND,
        Rights::RECV,
        Rights::SIGNAL,
        Rights::WAIT,
        Rights::GRANT,
        Rights::REVOKE,
    ];

    #[test]
    fn rights_are_distinct_single_bits() {
        let mut seen = 0u16;
        for r in ALL {
            assert_eq!(r.bits().count_ones(), 1);
            assert_eq!(seen & r.bits(), 0, "overlapping right");
            seen |= r.bits();
        }
        assert_eq!(seen, Rights::DEFINED.bits());
    }

    #[test]
    fn kinds_have_disjoint_io_rights_and_share_management_rights() {
        let ep = Rights::valid_for(Kind::Endpoint);
        let nf = Rights::valid_for(Kind::Notification);
        assert!(ep.contains(Rights::SEND | Rights::RECV | Rights::GRANT | Rights::REVOKE));
        assert!(nf.contains(Rights::SIGNAL | Rights::WAIT | Rights::GRANT | Rights::REVOKE));
        assert!(!ep.contains(Rights::SIGNAL) && !ep.contains(Rights::WAIT));
        assert!(!nf.contains(Rights::SEND) && !nf.contains(Rights::RECV));
        assert_eq!(Rights::root_for(Kind::Endpoint), ep);
        assert_eq!(Rights::root_for(Kind::Notification), nf);
    }

    #[test]
    fn from_bits_rejects_every_undefined_bit() {
        for bit in 6..16 {
            assert_eq!(Rights::from_bits(1 << bit), None, "bit {bit}");
            assert_eq!(Rights::from_bits(Rights::SEND.bits() | (1 << bit)), None);
        }
        assert_eq!(Rights::from_bits(0), Some(Rights::NONE));
        assert_eq!(Rights::from_bits(0b11_1111), Some(Rights::DEFINED));
    }

    #[test]
    fn from_user_validates_width_emptiness_and_kind() {
        let e = Kind::Endpoint;
        let n = Kind::Notification;
        assert_eq!(Rights::from_user(1, e), Ok(Rights::SEND));
        assert_eq!(Rights::from_user(0, e), Err(RightsError::Empty));
        assert_eq!(Rights::from_user(1 << 16, e), Err(RightsError::Unknown));
        assert_eq!(Rights::from_user(u64::MAX, e), Err(RightsError::Unknown));
        assert_eq!(Rights::from_user(1 << 6, e), Err(RightsError::Unknown));
        assert_eq!(
            Rights::from_user(Rights::SIGNAL.bits() as u64, e),
            Err(RightsError::WrongKind)
        );
        assert_eq!(
            Rights::from_user(Rights::SEND.bits() as u64, n),
            Err(RightsError::WrongKind)
        );
        // one wrong right poisons an otherwise valid set
        let mixed = (Rights::SEND | Rights::WAIT).bits() as u64;
        assert_eq!(Rights::from_user(mixed, e), Err(RightsError::WrongKind));
        assert_eq!(Rights::from_user(mixed, n), Err(RightsError::WrongKind));
    }

    #[test]
    fn attenuation_never_grows_over_every_pair_of_sets() {
        for a in 0u16..64 {
            for b in 0u16..64 {
                let (ra, rb) = (Rights::from_bits(a).unwrap(), Rights::from_bits(b).unwrap());
                let m = ra.attenuate(rb);
                assert!(m.is_subset_of(ra) && m.is_subset_of(rb));
                assert_eq!(m, ra & rb);
                assert_eq!(ra.is_subset_of(rb), ra.difference(rb).is_empty());
                assert_eq!(ra.contains(rb), rb.is_subset_of(ra));
                assert_eq!(ra.union(rb).bits(), a | b);
            }
        }
    }

    #[test]
    fn validate_for_accepts_exactly_the_nonempty_subsets_of_the_kind() {
        for kind in [Kind::Endpoint, Kind::Notification] {
            let valid = Rights::valid_for(kind);
            for bits in 0u16..64 {
                let r = Rights::from_bits(bits).unwrap();
                let got = r.validate_for(kind);
                if r.is_empty() {
                    assert_eq!(got, Err(RightsError::Empty));
                } else if r.is_subset_of(valid) {
                    assert_eq!(got, Ok(r));
                } else {
                    assert_eq!(got, Err(RightsError::WrongKind));
                }
            }
        }
    }

    #[test]
    fn default_is_empty_and_empty_is_subset_of_everything() {
        assert_eq!(Rights::default(), Rights::NONE);
        assert!(Rights::NONE.is_empty());
        assert!(Rights::NONE.is_subset_of(Rights::NONE));
        assert!(Rights::NONE.is_subset_of(Rights::SEND));
        assert!(!Rights::SEND.is_subset_of(Rights::NONE));
    }
}
