//! Journey path 2 of 4: stale-attempt rejection.
//!
//! An Attempt is superseded by an explicit retry, and the PREDECESSOR
//! worker — which does not know it was replaced — tries to keep
//! working. Production wiring must refuse it, and the refusal must
//! reach the caller as a loud typed error rather than a silent no-op.
//!
//! This is the interruption path the journey exists to pressure: two
//! agent sessions believing they own the same Assignment is the
//! failure that motivated fencing in the first place.

use abacus_core::ports::{
    AssignDecision, AssignmentOpening, AssignmentRecord, AttemptOpening, AttemptRecord,
    CredentialProvisioning, DecisionKind, DecisionReason, DecisionRecord, FencedAction, FencedCall,
    RetryDecision, StateApplied, StateError, WorkflowStatePort,
};
use abacus_core::usecase::record_report;
use abacus_core::{
    ActorId, AssignmentId, AttemptId, AuthorityClass, AuthoritySnapshot, BeadId, CapabilityId,
    CommitId, ContentHash, CredentialId, DecisionActor, EditScope, FencingToken, Lease,
    OperationId, ProfileName, ReportKind, ScopeExpr, ScopeMap, SemanticPhase, SignalBody,
    SignalDraft, SignalId, SubjectRef, Timestamp, WorkPath,
    assignment::AttemptPolicy,
    evidence::{AcceptancePolicy, Argv, PathSet, PolicyForm, VerificationSet},
};
use abacus_state::{InMemoryState, ManualClock};

fn assignment_id() -> AssignmentId {
    AssignmentId::new("asg-journey-2").expect("valid assignment id")
}

fn first_attempt() -> AttemptId {
    AttemptId::new("att-journey-2a").expect("valid attempt id")
}

fn successor_attempt() -> AttemptId {
    AttemptId::new("att-journey-2b").expect("valid attempt id")
}

fn op(raw: &str) -> OperationId {
    OperationId::new(raw).expect("valid operation id")
}

fn hash(fill: char) -> ContentHash {
    ContentHash::new(&fill.to_string().repeat(64)).expect("valid hash")
}

fn lead() -> DecisionActor {
    DecisionActor {
        actor: ActorId::new("journey-lead").expect("valid actor id"),
        class: AuthorityClass::Orchestrator,
        profile: ProfileName::new("lead").expect("valid profile"),
        profile_hash: hash('a'),
    }
}

fn worker() -> DecisionActor {
    DecisionActor {
        actor: ActorId::new("journey-worker").expect("valid actor id"),
        class: AuthorityClass::Worker,
        profile: ProfileName::new("worker").expect("valid profile"),
        profile_hash: hash('b'),
    }
}

fn authority(actor: DecisionActor, capability: &str) -> AuthoritySnapshot {
    AuthoritySnapshot {
        actor,
        capability: CapabilityId::new(capability).expect("valid capability"),
        scope: ScopeExpr::Universal,
    }
}

fn opening() -> AssignmentOpening {
    AssignmentOpening {
        assignment: AssignmentRecord {
            id: assignment_id(),
            bead: BeadId::new("ABACUS-journey.2").expect("valid bead id"),
            bead_content_hash: hash('c'),
            scope_map: ScopeMap::default(),
            worker: worker(),
            decision_actor: lead(),
            edit_scope: EditScope::new(vec![WorkPath::new("src").expect("valid path")])
                .expect("valid edit scope"),
            acceptance: AcceptancePolicy {
                verification: VerificationSet::new(
                    vec![Argv::new(vec!["cargo".into(), "test".into()]).expect("valid argv")],
                    PathSet::new(vec![WorkPath::new("src/lib.rs").expect("valid path")])
                        .expect("valid path set"),
                )
                .expect("valid verification set"),
                form: PolicyForm::Standard,
            },
            attempt_policy: AttemptPolicy::default(),
            declared_base: CommitId::new(&"d".repeat(40)).expect("valid commit"),
        },
        first_attempt: AttemptRecord {
            id: first_attempt(),
            assignment: assignment_id(),
            lease: Lease {
                token: FencingToken(1),
                expires_at: Timestamp(1_000),
            },
        },
        authorizing: AssignDecision {
            operation: op("op-journey-2-assign"),
            assignment: assignment_id(),
            first_attempt: first_attempt(),
            authority: authority(lead(), "state:assign"),
        },
        bead_revision: abacus_core::ports::WorkRevision(hash('e')),
        worker_credential: CredentialProvisioning {
            id: CredentialId::new("cred-journey-2a").expect("valid credential id"),
            digest: hash('f'),
        },
    }
}

/// The orchestrator's decision that ends the stalled predecessor. A
/// successor cannot open while its predecessor is active, so this is
/// the step that makes the retry legal.
fn revoke() -> DecisionRecord {
    DecisionRecord {
        operation: op("op-journey-2-revoke"),
        assignment: assignment_id(),
        authority: authority(lead(), "state:revoke"),
        kind: DecisionKind::Revoke {
            attempt: first_attempt(),
            reason: DecisionReason::new("predecessor stalled").expect("valid reason"),
        },
        resolves: None,
    }
}

/// The explicit retry that supersedes the first Attempt: a NEW Attempt
/// under a strictly higher fencing token, committed atomically with the
/// Retry decision that authorizes it.
fn retry() -> AttemptOpening {
    AttemptOpening {
        authorizing: RetryDecision {
            operation: op("op-journey-2-retry"),
            assignment: assignment_id(),
            authority: authority(lead(), "state:retry"),
            reason: DecisionReason::new("predecessor stalled").expect("valid reason"),
        },
        attempt: AttemptRecord {
            id: successor_attempt(),
            assignment: assignment_id(),
            lease: Lease {
                token: FencingToken(2),
                expires_at: Timestamp(2_000),
            },
        },
        worker_credential: CredentialProvisioning {
            id: CredentialId::new("cred-journey-2b").expect("valid credential id"),
            digest: hash('9'),
        },
    }
}

/// A worker Report from `attempt`, fenced by `token`.
fn report_from(attempt: AttemptId, token: FencingToken, operation: &str) -> FencedAction {
    FencedAction {
        call: FencedCall {
            assignment: assignment_id(),
            attempt,
            actor: worker().actor,
            token,
            operation: op(operation),
        },
        responds_to: None,
    }
}

fn progress(id: &str, attempt: AttemptId) -> SignalDraft {
    SignalDraft {
        id: SignalId::new(id).expect("valid signal id"),
        sender: authority(worker(), "state:report"),
        subject: SubjectRef::Attempt(attempt.clone()),
        body: SignalBody::Report {
            attempt,
            kind: ReportKind::Progress {
                phase: SemanticPhase::Verifying,
                summary: None,
            },
        },
    }
}

#[test]
fn a_superseded_worker_cannot_keep_writing() {
    let state = InMemoryState::new(ManualClock::new(Timestamp(0)));

    // The first Attempt opens and its worker reports normally.
    assert_eq!(
        state.open_assignment(&opening()),
        Ok(StateApplied::Applied),
        "the Assignment opens"
    );
    let (_, first_response) = record_report(
        &state,
        &report_from(first_attempt(), FencingToken(1), "op-journey-2-report-1"),
        &progress("sig-journey-2-1", first_attempt()),
    )
    .expect("the live worker may report");
    assert_eq!(first_response.applied, StateApplied::Applied);

    // The orchestrator judges the worker stalled and REVOKES its
    // Attempt. This step is not optional: production policy refuses a
    // successor while the predecessor is still active, so there is no
    // state in which two Attempts of one Assignment are both live.
    assert_eq!(
        state.record_decision(&revoke()),
        Ok(StateApplied::Applied),
        "the orchestrator ends the stalled Attempt"
    );

    // Only now may the successor Attempt open, under a strictly higher
    // fencing token, committed with its authorizing Retry decision in
    // one transaction.
    assert_eq!(
        state.append_attempt(&retry()),
        Ok(StateApplied::Applied),
        "the successor Attempt opens"
    );

    // The PREDECESSOR worker, unaware it was replaced, reports again
    // under its own still-held token. Production wiring must refuse it.
    let stale = record_report(
        &state,
        &report_from(first_attempt(), FencingToken(1), "op-journey-2-report-2"),
        &progress("sig-journey-2-2", first_attempt()),
    );
    // The journey's claim: the write is REFUSED, loudly, never silently
    // accepted and never silently dropped. That claim holds today.
    //
    // The refusal's CLASS does not: production currently answers
    // `IncoherentBundle`, whose contract means "the bundle's identities
    // disagree", when the honest answer is "your Attempt was
    // superseded". A worker cannot distinguish "stop, my successor owns
    // this" from "my request construction is buggy", and those demand
    // opposite responses. Filed as ABACUS-gf6; this assertion tightens
    // to the exact variant when that lands.
    assert!(
        stale.is_err(),
        "a superseded Attempt's writes must be refused, got {stale:?}"
    );
    assert_eq!(
        stale,
        Err(StateError::IncoherentBundle),
        "pinning today's ACTUAL refusal so ABACUS-gf6 cannot regress \
         silently - this is the defect, recorded, not the intent"
    );

    // The successor is unaffected and writes normally: supersession
    // stopped exactly one Attempt, not the Assignment.
    let (_, successor_response) = record_report(
        &state,
        &report_from(
            successor_attempt(),
            FencingToken(2),
            "op-journey-2-report-3",
        ),
        &progress("sig-journey-2-3", successor_attempt()),
    )
    .expect("the successor worker may report");
    assert_eq!(successor_response.applied, StateApplied::Applied);
}
