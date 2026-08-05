//! Assignment/Attempt separation rules and decision-actor snapshots.
//!
//! An Assignment binds one bead to one worker and names its exact
//! decision actor (CONTEXT §2); it may contain several *sequential*
//! Attempts, appended only by explicit fenced decision (I18 — never by
//! a timer or transition). Acceptance rechecks the bead-content hash
//! bound at creation: "the bead closed is the bead planned"
//! (ADR-0001 §9.1). Full persistence shape belongs to `abacus-state`;
//! this module owns the pure rules those records must obey.

use crate::authority::AuthorityClass;
use crate::content::ContentHash;
use crate::id::{ActorId, CapabilityId, ProfileName};
use crate::lifecycle::{AssignmentState, AttemptState};
use crate::scope::ScopeExpr;

/// The concrete identity recorded on every decision (I17): actor,
/// class, profile, and the profile content hash in force.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecisionActor {
    pub actor: ActorId,
    pub class: AuthorityClass,
    pub profile: ProfileName,
    pub profile_hash: ContentHash,
}

/// The single generic authority snapshot (I17, ADR-0002): who, as
/// what, exercising which capability over which canonical scope.
/// Produced by `ValidatedProfileSet::authorize` from the current
/// activation — never assembled from caller-supplied parts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthoritySnapshot {
    pub actor: DecisionActor,
    pub capability: CapabilityId,
    pub scope: ScopeExpr,
}

/// Distinct Attempt-sequencing failures.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AttemptSequenceError {
    /// The Assignment is terminal; nothing may be appended (I10).
    AssignmentTerminal,
    /// The previous Attempt is still active: Attempts are sequential,
    /// never parallel.
    PriorAttemptActive,
}

/// May a new fenced Attempt be appended? The caller supplies the
/// Assignment's state and the last Attempt's state, if any. The *right*
/// to append is the decision actor's (checked by authorization); this
/// rule is only about sequence legality.
pub fn next_attempt_allowed(
    assignment: AssignmentState,
    last_attempt: Option<AttemptState>,
) -> Result<(), AttemptSequenceError> {
    if assignment.is_terminal() {
        return Err(AttemptSequenceError::AssignmentTerminal);
    }
    match last_attempt {
        Some(state) if !state.is_ended() => Err(AttemptSequenceError::PriorAttemptActive),
        _ => Ok(()),
    }
}

/// Bounded per-Assignment Attempt cap: 1..=100.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct AttemptCap(u8);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AttemptCapError {
    Zero,
    TooLarge,
}

impl AttemptCap {
    pub fn new(value: u8) -> Result<Self, AttemptCapError> {
        match value {
            0 => Err(AttemptCapError::Zero),
            1..=100 => Ok(Self(value)),
            _ => Err(AttemptCapError::TooLarge),
        }
    }

    pub fn value(self) -> u8 {
        self.0
    }
}

/// The Assignment's authored attempt policy (I18): `None` cap means
/// unbounded only through repeated explicit decisions, never a hidden
/// retry loop.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct AttemptPolicy {
    pub cap: Option<AttemptCap>,
}

/// The explicit-retry cap was reached; a further Attempt requires the
/// policy itself to change by explicit decision.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct AttemptCapReached;

/// Pure explicit-retry cap check (state contract: the core-validated
/// optional per-Assignment cap applies on retry). `existing_attempts`
/// counts all prior Attempts, ended or live.
pub fn retry_within_cap(
    policy: &AttemptPolicy,
    existing_attempts: u32,
) -> Result<(), AttemptCapReached> {
    match policy.cap {
        Some(cap) if existing_attempts >= u32::from(cap.value()) => Err(AttemptCapReached),
        _ => Ok(()),
    }
}

/// Acceptance-time bead-content recheck (core invariant 17).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct BeadHashMismatch;

pub fn recheck_bead_hash(
    bound: &ContentHash,
    current: &ContentHash,
) -> Result<(), BeadHashMismatch> {
    if bound == current {
        Ok(())
    } else {
        Err(BeadHashMismatch)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_attempt_needs_only_a_live_assignment() {
        assert_eq!(next_attempt_allowed(AssignmentState::Active, None), Ok(()));
        assert_eq!(
            next_attempt_allowed(AssignmentState::Accepted, None),
            Err(AttemptSequenceError::AssignmentTerminal)
        );
        assert_eq!(
            next_attempt_allowed(AssignmentState::Cancelled, None),
            Err(AttemptSequenceError::AssignmentTerminal)
        );
    }

    #[test]
    fn attempts_are_strictly_sequential() {
        for live in [AttemptState::Active, AttemptState::Submitted] {
            assert_eq!(
                next_attempt_allowed(AssignmentState::Active, Some(live)),
                Err(AttemptSequenceError::PriorAttemptActive)
            );
        }
        for ended in [
            AttemptState::Accepted,
            AttemptState::Rejected,
            AttemptState::Revoked,
            AttemptState::Expired,
            AttemptState::Aborted,
        ] {
            assert_eq!(
                next_attempt_allowed(AssignmentState::Active, Some(ended)),
                Ok(())
            );
        }
    }

    #[test]
    fn attempt_cap_is_bounded_and_checked() {
        assert_eq!(AttemptCap::new(0), Err(AttemptCapError::Zero));
        assert_eq!(AttemptCap::new(101), Err(AttemptCapError::TooLarge));
        let policy = AttemptPolicy {
            cap: Some(AttemptCap::new(2).unwrap()),
        };
        assert_eq!(retry_within_cap(&policy, 1), Ok(()));
        assert_eq!(retry_within_cap(&policy, 2), Err(AttemptCapReached));
        // No cap: unbounded only through explicit decisions (I18).
        assert_eq!(retry_within_cap(&AttemptPolicy::default(), 500), Ok(()));
    }

    #[test]
    fn bead_hash_recheck() {
        let a = ContentHash::new(&"a".repeat(64)).unwrap();
        let b = ContentHash::new(&"b".repeat(64)).unwrap();
        assert_eq!(recheck_bead_hash(&a, &a), Ok(()));
        assert_eq!(recheck_bead_hash(&a, &b), Err(BeadHashMismatch));
    }
}
