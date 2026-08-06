//! Journey path 3 of 4: Abort, and its compliant terminal carrier.
//!
//! The orchestrator issues a binding Abort Directive. The worker's
//! ordinary writes are then refused IN BAND - as a domain outcome
//! carrying the response envelope, never as a protocol error - and the
//! only terminal action available to it is explicit abort compliance.
//! The audit record for that compliance names the honest initiator: a
//! recovered worker binding, not a fabricated capability the call
//! never exercised.
//!
//! This is the directive path the journey exists to pressure: a worker
//! that has been told to stop must not be able to keep contributing,
//! and must not be able to end its Attempt by any route that skips the
//! Abort it was given.

use abacus_core::ports::{
    AssignDecision, AssignmentOpening, AssignmentRecord, AttemptRecord, AuditClass, AuditInitiator,
    AuditQuery, AuditSubject, CredentialProvisioning, FencedAction, FencedCall, ReportOutcome,
    StateApplied, StateError, WorkflowStatePort,
};
use abacus_core::usecase::record_report;
use abacus_core::{
    ActorId, AssignmentId, AttemptId, AuthorityClass, AuthoritySnapshot, BeadId, BoundedText,
    CapabilityId, CommitId, ContentHash, CredentialId, DecisionActor, DirectiveGateRefusal,
    DirectiveKind, EditScope, FencingToken, Lease, OperationId, ProfileName, ReportKind, ScopeExpr,
    ScopeMap, SemanticPhase, SignalBody, SignalDraft, SignalId, SubjectRef, Timestamp, WorkPath,
    assignment::AttemptPolicy,
    evidence::{AcceptancePolicy, Argv, PathSet, PolicyForm, VerificationSet},
};
use abacus_state::{InMemoryState, ManualClock};

fn assignment_id() -> AssignmentId {
    AssignmentId::new("asg-journey-3").expect("valid assignment id")
}

fn attempt_id() -> AttemptId {
    AttemptId::new("att-journey-3").expect("valid attempt id")
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
            bead: BeadId::new("ABACUS-journey.3").expect("valid bead id"),
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
            id: attempt_id(),
            assignment: assignment_id(),
            lease: Lease {
                token: FencingToken(1),
                expires_at: Timestamp(1_000),
            },
        },
        authorizing: AssignDecision {
            operation: op("op-journey-3-assign"),
            assignment: assignment_id(),
            first_attempt: attempt_id(),
            authority: authority(lead(), "state:assign"),
        },
        bead_revision: abacus_core::ports::WorkRevision(hash('e')),
        worker_credential: CredentialProvisioning {
            id: CredentialId::new("cred-journey-3").expect("valid credential id"),
            digest: hash('f'),
        },
    }
}

/// The orchestrator's binding Abort, addressed to the live Attempt.
fn abort_directive() -> SignalDraft {
    SignalDraft {
        id: SignalId::new("sig-journey-3-abort").expect("valid signal id"),
        sender: authority(lead(), "state:directive"),
        subject: SubjectRef::Attempt(attempt_id()),
        body: SignalBody::Directive {
            assignment: assignment_id(),
            attempt: attempt_id(),
            kind: DirectiveKind::Abort {
                reason: BoundedText::new("scope superseded upstream").expect("valid reason"),
            },
        },
    }
}

fn worker_call(operation: &str) -> FencedCall {
    FencedCall {
        assignment: assignment_id(),
        attempt: attempt_id(),
        actor: worker().actor,
        token: FencingToken(1),
        operation: op(operation),
    }
}

fn worker_action(operation: &str) -> FencedAction {
    FencedAction {
        call: worker_call(operation),
        responds_to: None,
    }
}

fn progress(id: &str) -> SignalDraft {
    SignalDraft {
        id: SignalId::new(id).expect("valid signal id"),
        sender: authority(worker(), "state:report"),
        subject: SubjectRef::Attempt(attempt_id()),
        body: SignalBody::Report {
            attempt: attempt_id(),
            kind: ReportKind::Progress {
                phase: SemanticPhase::Verifying,
                summary: None,
            },
        },
    }
}

#[test]
fn a_binding_abort_refuses_work_in_band_and_only_compliance_ends_the_attempt() {
    let state = InMemoryState::new(ManualClock::new(Timestamp(0)));
    assert_eq!(
        state.open_assignment(&opening()),
        Ok(StateApplied::Applied),
        "the Assignment opens"
    );

    // Before the Abort the worker reports normally.
    let (before, _) = record_report(
        &state,
        &worker_action("op-journey-3-report-1"),
        &progress("sig-journey-3-1"),
    )
    .expect("the live worker may report");
    assert!(
        matches!(before, ReportOutcome::Recorded { .. }),
        "an unaborted worker's Report is recorded, got {before:?}"
    );

    // The orchestrator issues the binding Abort.
    let (_, applied) = state
        .append_signal(&abort_directive())
        .expect("the orchestrator may direct its Attempt");
    assert_eq!(applied, StateApplied::Applied);

    // The worker's next Report is refused IN BAND: a domain outcome
    // naming the binding Abort, NOT a protocol or authority error - and
    // the response envelope still arrives, which is how the worker
    // learns why it was stopped.
    let (refused, response) = record_report(
        &state,
        &worker_action("op-journey-3-report-2"),
        &progress("sig-journey-3-2"),
    )
    .expect("an Abort refusal is a domain outcome, never an Err");
    assert_eq!(
        refused,
        ReportOutcome::Refused {
            reason: DirectiveGateRefusal::AbortInForce
        },
        "the refusal names the Abort so the worker can act on it"
    );
    assert_eq!(
        response.binding_directives.len(),
        1,
        "the response mechanically surfaces the binding Directive - the \
         worker never has to ask what stopped it"
    );

    // The ONLY terminal action available is explicit abort compliance.
    let compliance = state
        .fenced_abort_attempt(&worker_call("op-journey-3-comply"))
        .expect("a worker under a binding Abort may comply");
    assert_eq!(compliance.applied, StateApplied::Applied);

    // Compliance is idempotent: a lost response must not produce a
    // second terminal action.
    let replay = state
        .fenced_abort_attempt(&worker_call("op-journey-3-comply"))
        .expect("exact replay is absorbed");
    assert_eq!(
        replay.applied,
        StateApplied::AlreadyApplied,
        "an identical retry returns the stored outcome, not a new terminal"
    );

    // The audit record for compliance names the HONEST initiator: the
    // worker binding recovered from durable Assignment state. It does
    // not fabricate a capability or scope the call never exercised.
    let events = state
        .audit_events(&AuditQuery {
            subject: Some(AuditSubject::Workflow(SubjectRef::Attempt(attempt_id()))),
            class: Some(AuditClass::Attempt),
            from: None,
            through: None,
        })
        .expect("audit query succeeds");
    let compliance_event = events
        .iter()
        .find(|event| matches!(&event.initiator, AuditInitiator::WorkerBinding { .. }))
        .expect("abort compliance is audited under a worker binding");
    match &compliance_event.initiator {
        AuditInitiator::WorkerBinding {
            actor,
            assignment,
            attempt,
        } => {
            assert_eq!(
                actor,
                &worker(),
                "the recorded actor is the Assignment's bound worker, \
                 recovered from state rather than asserted by the caller"
            );
            assert_eq!(assignment, &assignment_id());
            assert_eq!(attempt, &attempt_id());
        }
        other => panic!("expected a recovered worker binding, got {other:?}"),
    }
}

#[test]
fn abort_compliance_is_unavailable_without_a_binding_abort() {
    let state = InMemoryState::new(ManualClock::new(Timestamp(0)));
    assert_eq!(
        state.open_assignment(&opening()),
        Ok(StateApplied::Applied),
        "the Assignment opens"
    );

    // No Abort has been issued. The terminal carrier must not be a
    // back door for voluntary self-cancellation: a worker cannot end
    // its own Attempt just by claiming compliance.
    assert_eq!(
        state.fenced_abort_attempt(&worker_call("op-journey-3-false-comply")),
        Err(StateError::AbortNotInForce),
        "voluntary worker self-cancellation stays unrepresentable"
    );
}
