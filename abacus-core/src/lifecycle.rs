//! Assignment and Attempt lifecycle states with pure transition validation.
//!
//! The state machines encode CONTEXT §2 and §4: Accepted and Cancelled
//! Assignments are terminal (I10 — corrections are new records, never
//! rewrites); rejection, revocation, reclamation, and acceptance end an
//! Attempt while its Assignment survives; lease expiry is a
//! *time-derived fact* supplied by the caller, never observed by core
//! (I13), and gates only reclamation — nothing silently reassigns.
//! A Submission refusal is deliberately absent here: it is an audited
//! outcome that changes no lifecycle state (CONTEXT §2).
//!
//! Richer records (leases, fencing tokens, evidence binding) belong to
//! the assignment/attempt model (ABACUS-9NH.3); this module owns only
//! the state vocabulary and the legal-transition tables that everything
//! else composes.

/// Lifecycle of one Assignment (bead ↔ worker binding).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AssignmentState {
    Active,
    /// Terminal: one immutable Acceptance decision (ADR-0001 §3 saga).
    Accepted,
    /// Terminal: explicit fenced cancellation of an obsolete Assignment.
    Cancelled,
}

impl AssignmentState {
    pub fn is_terminal(self) -> bool {
        !matches!(self, Self::Active)
    }
}

/// Exact-decision actions on an Assignment.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AssignmentAction {
    Accept,
    Cancel,
}

/// Lifecycle of one Attempt under an Assignment.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AttemptState {
    Active,
    /// Ended successfully with its Assignment's Acceptance.
    Accepted,
    /// Explicit Rejection of a recorded Handoff; ends only this Attempt.
    Rejected,
    /// Explicit revocation by the Assignment's decision actor.
    Revoked,
    /// Explicit reclamation after lease expiry; partial product preserved.
    Reclaimed,
}

impl AttemptState {
    pub fn is_ended(self) -> bool {
        !matches!(self, Self::Active)
    }
}

/// Exact-decision actions on an Attempt.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AttemptAction {
    Accept,
    Reject,
    Revoke,
    /// Valid only once the caller-supplied lease-expiry fact is true.
    Reclaim,
}

/// Distinct transition failures (core contract: normalized error
/// categories; every refusal is loud and specific).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TransitionError {
    /// The record is terminal; history is never rewritten (I10).
    TerminalState,
    /// Reclamation attempted while the lease is still live.
    LeaseNotExpired,
}

/// Validate an Assignment action against its current state.
pub fn assignment_transition(
    state: AssignmentState,
    action: AssignmentAction,
) -> Result<AssignmentState, TransitionError> {
    if state.is_terminal() {
        return Err(TransitionError::TerminalState);
    }
    Ok(match action {
        AssignmentAction::Accept => AssignmentState::Accepted,
        AssignmentAction::Cancel => AssignmentState::Cancelled,
    })
}

/// Validate an Attempt action against its current state. `lease_expired`
/// is an input fact (I13): core never reads a clock.
pub fn attempt_transition(
    state: AttemptState,
    action: AttemptAction,
    lease_expired: bool,
) -> Result<AttemptState, TransitionError> {
    if state.is_ended() {
        return Err(TransitionError::TerminalState);
    }
    match action {
        AttemptAction::Accept => Ok(AttemptState::Accepted),
        AttemptAction::Reject => Ok(AttemptState::Rejected),
        AttemptAction::Revoke => Ok(AttemptState::Revoked),
        AttemptAction::Reclaim => {
            if lease_expired {
                Ok(AttemptState::Reclaimed)
            } else {
                Err(TransitionError::LeaseNotExpired)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ASSIGNMENT_STATES: [AssignmentState; 3] = [
        AssignmentState::Active,
        AssignmentState::Accepted,
        AssignmentState::Cancelled,
    ];
    const ASSIGNMENT_ACTIONS: [AssignmentAction; 2] =
        [AssignmentAction::Accept, AssignmentAction::Cancel];
    const ATTEMPT_STATES: [AttemptState; 5] = [
        AttemptState::Active,
        AttemptState::Accepted,
        AttemptState::Rejected,
        AttemptState::Revoked,
        AttemptState::Reclaimed,
    ];
    const ATTEMPT_ACTIONS: [AttemptAction; 4] = [
        AttemptAction::Accept,
        AttemptAction::Reject,
        AttemptAction::Revoke,
        AttemptAction::Reclaim,
    ];

    /// Exhaustive table over every (state, action) pair.
    #[test]
    fn assignment_transition_table() {
        for state in ASSIGNMENT_STATES {
            for action in ASSIGNMENT_ACTIONS {
                let result = assignment_transition(state, action);
                let expected = match (state, action) {
                    (AssignmentState::Active, AssignmentAction::Accept) => {
                        Ok(AssignmentState::Accepted)
                    }
                    (AssignmentState::Active, AssignmentAction::Cancel) => {
                        Ok(AssignmentState::Cancelled)
                    }
                    _ => Err(TransitionError::TerminalState),
                };
                assert_eq!(result, expected, "{state:?} + {action:?}");
            }
        }
    }

    /// Exhaustive table over every (state, action, lease_expired) triple.
    #[test]
    fn attempt_transition_table() {
        for state in ATTEMPT_STATES {
            for action in ATTEMPT_ACTIONS {
                for lease_expired in [false, true] {
                    let result = attempt_transition(state, action, lease_expired);
                    let expected = match (state, action) {
                        (AttemptState::Active, AttemptAction::Accept) => Ok(AttemptState::Accepted),
                        (AttemptState::Active, AttemptAction::Reject) => Ok(AttemptState::Rejected),
                        (AttemptState::Active, AttemptAction::Revoke) => Ok(AttemptState::Revoked),
                        (AttemptState::Active, AttemptAction::Reclaim) if lease_expired => {
                            Ok(AttemptState::Reclaimed)
                        }
                        (AttemptState::Active, AttemptAction::Reclaim) => {
                            Err(TransitionError::LeaseNotExpired)
                        }
                        _ => Err(TransitionError::TerminalState),
                    };
                    assert_eq!(result, expected, "{state:?} + {action:?} (expired={lease_expired})");
                }
            }
        }
    }

    /// Terminality is exactly the non-Active states, on both machines.
    #[test]
    fn terminality() {
        assert!(!AssignmentState::Active.is_terminal());
        assert!(AssignmentState::Accepted.is_terminal());
        assert!(AssignmentState::Cancelled.is_terminal());
        assert!(!AttemptState::Active.is_ended());
        for state in [
            AttemptState::Accepted,
            AttemptState::Rejected,
            AttemptState::Revoked,
            AttemptState::Reclaimed,
        ] {
            assert!(state.is_ended(), "{state:?}");
        }
    }

    /// A live lease never blocks the explicit decision actions — only
    /// reclamation waits on expiry, and expiry alone reclaims nothing.
    #[test]
    fn lease_fact_gates_only_reclamation() {
        for action in [AttemptAction::Accept, AttemptAction::Reject, AttemptAction::Revoke] {
            assert_eq!(
                attempt_transition(AttemptState::Active, action, false),
                attempt_transition(AttemptState::Active, action, true),
                "{action:?} must ignore lease state"
            );
        }
    }
}
