//! Assignment and Attempt lifecycle states with pure transition validation.
//!
//! The state machines encode CONTEXT §2/§4 and architecture §5-§6:
//! a Handoff submission moves the Attempt (never the Assignment) from
//! `Active` to `Submitted`; Accept/Reject decide only a `Submitted`
//! Attempt; a Submission refusal changes no state; Accepted and
//! Cancelled Assignments are terminal (I10 — corrections are new
//! records). Lease expiry is a *time-derived fact* supplied by the
//! caller (I13) and gates only reclamation — nothing silently
//! reassigns, and a `Submitted` Attempt is never reclaimed: completed
//! work awaiting decision is resolved by decision or revocation, not
//! recovery.
//!
//! Richer records (leases, fencing tokens, evidence binding) belong to
//! the assignment/attempt model; this module owns the state vocabulary
//! and legal-transition tables everything else composes.

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
    /// An immutable Handoff is recorded; awaiting Accept/Reject. Not
    /// ended — it still blocks a successor Attempt — but no longer
    /// freely mutating.
    Submitted,
    /// Ended successfully with its Assignment's Acceptance.
    Accepted,
    /// Explicit Rejection of the recorded Handoff; ends only this
    /// Attempt.
    Rejected,
    /// Explicit revocation by the Assignment's decision actor.
    Revoked,
    /// Terminal `Expired`: reached by explicit reclamation after
    /// lease expiry (CONTEXT: a failed, rejected, expired, or revoked
    /// Attempt ends; nothing expires an Attempt implicitly — Reclaim is
    /// the fenced decision action, Expired the resulting state).
    /// Partial product preserved.
    Expired,
}

impl AttemptState {
    pub fn is_ended(self) -> bool {
        !matches!(self, Self::Active | Self::Submitted)
    }
}

/// Actions on an Attempt.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AttemptAction {
    /// Record a valid Handoff (fenced worker action).
    Submit,
    Accept,
    Reject,
    Revoke,
    /// Valid only from `Active` once the caller-supplied lease-expiry
    /// fact is true.
    Reclaim,
}

/// Distinct transition failures (normalized error categories).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TransitionError {
    /// The record is ended/terminal; history is never rewritten (I10).
    TerminalState,
    /// Accept/Reject require a recorded Handoff (`Submitted`).
    NotSubmitted,
    /// A `Submitted` Attempt has a recorded Handoff awaiting decision;
    /// it is accepted or rejected, never revoked (architecture §5-§6).
    HandoffPending,
    /// Submit requires `Active`; a Handoff is already recorded.
    AlreadySubmitted,
    /// Reclamation recovers abandoned `Active` work only; a `Submitted`
    /// Attempt is decided or revoked, never reclaimed.
    NotReclaimable,
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
        AttemptAction::Submit => match state {
            AttemptState::Active => Ok(AttemptState::Submitted),
            _ => Err(TransitionError::AlreadySubmitted),
        },
        AttemptAction::Accept => match state {
            AttemptState::Submitted => Ok(AttemptState::Accepted),
            _ => Err(TransitionError::NotSubmitted),
        },
        AttemptAction::Reject => match state {
            AttemptState::Submitted => Ok(AttemptState::Rejected),
            _ => Err(TransitionError::NotSubmitted),
        },
        AttemptAction::Revoke => match state {
            AttemptState::Active => Ok(AttemptState::Revoked),
            _ => Err(TransitionError::HandoffPending),
        },
        AttemptAction::Reclaim => match state {
            AttemptState::Active if lease_expired => Ok(AttemptState::Expired),
            AttemptState::Active => Err(TransitionError::LeaseNotExpired),
            _ => Err(TransitionError::NotReclaimable),
        },
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
    const ATTEMPT_STATES: [AttemptState; 6] = [
        AttemptState::Active,
        AttemptState::Submitted,
        AttemptState::Accepted,
        AttemptState::Rejected,
        AttemptState::Revoked,
        AttemptState::Expired,
    ];
    const ATTEMPT_ACTIONS: [AttemptAction; 5] = [
        AttemptAction::Submit,
        AttemptAction::Accept,
        AttemptAction::Reject,
        AttemptAction::Revoke,
        AttemptAction::Reclaim,
    ];

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

    /// Exhaustive table over every (state, action, lease_expired)
    /// triple — the full Handoff lifecycle: submit only from Active,
    /// accept/reject only from Submitted, revoke only from Active (a
    /// recorded Handoff is decided, never revoked), reclaim only from
    /// expired Active.
    #[test]
    fn attempt_transition_table() {
        for state in ATTEMPT_STATES {
            for action in ATTEMPT_ACTIONS {
                for lease_expired in [false, true] {
                    let result = attempt_transition(state, action, lease_expired);
                    let expected = match (state, action) {
                        (AttemptState::Active, AttemptAction::Submit) => {
                            Ok(AttemptState::Submitted)
                        }
                        (AttemptState::Submitted, AttemptAction::Submit) => {
                            Err(TransitionError::AlreadySubmitted)
                        }
                        (AttemptState::Submitted, AttemptAction::Accept) => {
                            Ok(AttemptState::Accepted)
                        }
                        (AttemptState::Submitted, AttemptAction::Reject) => {
                            Ok(AttemptState::Rejected)
                        }
                        (AttemptState::Active, AttemptAction::Accept)
                        | (AttemptState::Active, AttemptAction::Reject) => {
                            Err(TransitionError::NotSubmitted)
                        }
                        (AttemptState::Active, AttemptAction::Revoke) => Ok(AttemptState::Revoked),
                        (AttemptState::Submitted, AttemptAction::Revoke) => {
                            Err(TransitionError::HandoffPending)
                        }
                        (AttemptState::Active, AttemptAction::Reclaim) if lease_expired => {
                            Ok(AttemptState::Expired)
                        }
                        (AttemptState::Active, AttemptAction::Reclaim) => {
                            Err(TransitionError::LeaseNotExpired)
                        }
                        (AttemptState::Submitted, AttemptAction::Reclaim) => {
                            Err(TransitionError::NotReclaimable)
                        }
                        _ => Err(TransitionError::TerminalState),
                    };
                    assert_eq!(
                        result, expected,
                        "{state:?} + {action:?} (expired={lease_expired})"
                    );
                }
            }
        }
    }

    /// Submitted is live (blocks a successor Attempt) but not ended.
    #[test]
    fn terminality() {
        assert!(!AssignmentState::Active.is_terminal());
        assert!(AssignmentState::Accepted.is_terminal());
        assert!(AssignmentState::Cancelled.is_terminal());
        assert!(!AttemptState::Active.is_ended());
        assert!(!AttemptState::Submitted.is_ended());
        for state in [
            AttemptState::Accepted,
            AttemptState::Rejected,
            AttemptState::Revoked,
            AttemptState::Expired,
        ] {
            assert!(state.is_ended(), "{state:?}");
        }
    }

    /// A live lease never blocks decision actions; expiry alone
    /// reclaims nothing and gates only reclamation.
    #[test]
    fn lease_fact_gates_only_reclamation() {
        for action in [AttemptAction::Submit, AttemptAction::Revoke] {
            assert_eq!(
                attempt_transition(AttemptState::Active, action, false),
                attempt_transition(AttemptState::Active, action, true),
                "{action:?} must ignore lease state"
            );
        }
        assert_eq!(
            attempt_transition(AttemptState::Submitted, AttemptAction::Accept, true),
            attempt_transition(AttemptState::Submitted, AttemptAction::Accept, false),
        );
    }
}
