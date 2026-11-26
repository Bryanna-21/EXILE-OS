//! Deterministic exact-ID and class-based driver matching.
use super::device::{DeviceClass, DeviceDescriptor};
use super::driver::{DriverDescriptor, DriverId};
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MatchRule {
    Exact { vendor: u16, product: u16 },
    Class(DeviceClass),
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MatchCandidate {
    pub driver: DriverId,
    pub priority: u16,
    pub specificity: u8,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MatchResult {
    NoMatch,
    Unique(MatchCandidate),
    Ambiguous { first: DriverId, second: DriverId },
}
pub fn candidate(d: &DeviceDescriptor, driver: &DriverDescriptor) -> Option<MatchCandidate> {
    let specificity = driver
        .matches
        .iter()
        .filter_map(|rule| match *rule {
            MatchRule::Exact { vendor, product } if vendor == d.vendor && product == d.product => {
                Some(2)
            }
            MatchRule::Class(c) if c == d.class => Some(1),
            _ => None,
        })
        .max()?;
    Some(MatchCandidate {
        driver: driver.id,
        priority: driver.priority,
        specificity,
    })
}
pub fn choose(candidates: &[MatchCandidate]) -> MatchResult {
    let mut best: Option<MatchCandidate> = None;
    for c in candidates {
        if best.is_none_or(|b| (c.specificity, c.priority) > (b.specificity, b.priority)) {
            best = Some(*c)
        }
    }
    let Some(b) = best else {
        return MatchResult::NoMatch;
    };
    let mut tied = candidates
        .iter()
        .filter(|c| (c.specificity, c.priority) == (b.specificity, b.priority));
    let first = tied.next().map(|c| c.driver).unwrap_or(b.driver);
    if let Some(second) = tied.next() {
        MatchResult::Ambiguous {
            first,
            second: second.driver,
        }
    } else {
        MatchResult::Unique(b)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::e2::device_id::{BusKind, DeviceId};
    #[test]
    fn deterministic_priority_and_ties() {
        let id = DeviceId::new(BusKind::Pci, 0, 1, 1).unwrap();
        let d = DeviceDescriptor {
            id,
            parent: None,
            class: DeviceClass::Storage,
            vendor: 1,
            product: 2,
            revision: 0,
            state: super::super::device::DeviceState::Discovered,
            failure: None,
        };
        let a = DriverDescriptor {
            id: DriverId(1),
            name: "a",
            priority: 1,
            matches: &[MatchRule::Exact {
                vendor: 1,
                product: 2,
            }],
            probe: |_| Ok(true),
            start: |_| Ok(()),
            stop: |_| Ok(()),
            unbind: |_| Ok(()),
        };
        assert_eq!(candidate(&d, &a).unwrap().specificity, 2);
        assert_eq!(choose(&[]), MatchResult::NoMatch);
        let c = MatchCandidate {
            driver: DriverId(1),
            priority: 1,
            specificity: 2,
        };
        assert!(matches!(
            choose(&[
                c,
                MatchCandidate {
                    driver: DriverId(2),
                    ..c
                }
            ]),
            MatchResult::Ambiguous { .. }
        ));
    }
}
