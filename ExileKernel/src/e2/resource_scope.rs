//! Explicit managed cleanup stack. Failed releases remain tracked for retry.
use super::device_id::DeviceId;
use super::resource_ledger::{ClaimToken, LedgerError};
pub trait ResourceReleaser {
    fn release(&mut self, token: ClaimToken, owner: DeviceId) -> Result<(), LedgerError>;
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScopeError {
    Full,
    Duplicate,
    NotFound,
    ReleaseFailed(LedgerError),
}
pub struct ResourceScope<const N: usize> {
    owner: DeviceId,
    // Tracked tokens are kept contiguous in acquisition order. This makes
    // release_all a true LIFO unwind, including after a partial failure.
    tokens: [Option<ClaimToken>; N],
    len: usize,
}

impl<const N: usize> ResourceScope<N> {
    pub const fn new(owner: DeviceId) -> Self {
        Self {
            owner,
            tokens: [None; N],
            len: 0,
        }
    }

    pub fn track(&mut self, token: ClaimToken) -> Result<(), ScopeError> {
        if self.tokens[..self.len]
            .iter()
            .any(|tracked| *tracked == Some(token))
        {
            return Err(ScopeError::Duplicate);
        }
        if self.len == N {
            return Err(ScopeError::Full);
        }

        self.tokens[self.len] = Some(token);
        self.len += 1;
        Ok(())
    }

    pub const fn len(&self) -> usize {
        self.len
    }

    /// Releases in reverse acquisition order.
    ///
    /// On the first failure, the failed token and all earlier acquisitions
    /// remain tracked. This preserves correct dependency order for retries.
    pub fn release_all<R: ResourceReleaser>(&mut self, releaser: &mut R) -> Result<(), ScopeError> {
        while self.len > 0 {
            let index = self.len - 1;
            let token = self.tokens[index].ok_or(ScopeError::NotFound)?;

            match releaser.release(token, self.owner) {
                Ok(()) => {
                    self.tokens[index] = None;
                    self.len -= 1;
                }
                Err(error) => return Err(ScopeError::ReleaseFailed(error)),
            }
        }

        Ok(())
    }

    pub fn check_invariants(&self) -> bool {
        self.len <= N
            && self.tokens[..self.len].iter().all(Option::is_some)
            && self.tokens[self.len..].iter().all(Option::is_none)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::e2::device_id::BusKind;
    struct OrderedReleaser {
        fail_on: Option<ClaimToken>,
        released: [Option<ClaimToken>; 3],
        len: usize,
    }
    impl ResourceReleaser for OrderedReleaser {
        fn release(&mut self, t: ClaimToken, _: DeviceId) -> Result<(), LedgerError> {
            if self.fail_on == Some(t) {
                return Err(LedgerError::Stale);
            }
            self.released[self.len] = Some(t);
            self.len += 1;
            Ok(())
        }
    }
    #[test]
    fn partial_failure_preserves_lifo_order_for_retry() {
        let owner = DeviceId::new(BusKind::Virtual, 0, 1, 1).unwrap();
        let mut s = ResourceScope::<3>::new(owner);
        let a = ClaimToken::from_parts(0, 1);
        let b = ClaimToken::from_parts(1, 1);
        let c = ClaimToken::from_parts(2, 1);
        s.track(a).unwrap();
        s.track(b).unwrap();
        let mut r = OrderedReleaser {
            fail_on: Some(b),
            released: [None; 3],
            len: 0,
        };
        assert!(matches!(
            s.release_all(&mut r),
            Err(ScopeError::ReleaseFailed(_))
        ));
        assert_eq!(r.len, 0);
        assert_eq!(s.len(), 2);
        s.track(c).unwrap();
        r.fail_on = None;
        s.release_all(&mut r).unwrap();
        assert_eq!(&r.released[..r.len], &[Some(c), Some(b), Some(a)]);
        assert_eq!(s.len(), 0);
        assert!(s.check_invariants());
    }
}
