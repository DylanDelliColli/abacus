//! Journey path 4 of 4: Acceptance whose application is ambiguous,
//! followed by explicit reconciliation.
//!
//! The sequence is pinned EXACTLY (reviewer tightening, 2026-08-05),
//! superseding any weaker "acceptance OR ambiguity" reading: commit the
//! Acceptance decision, the close application returns ambiguous, the
//! projection remains pending, an EXPLICIT reconciliation observes the
//! effect, and the receipt clears the pending set.
//!
//! A happy Acceptance alone proves the easy half and misses the seam
//! this journey exists to pressure: the window in which the Ledger has
//! decided but the work graph has not confirmed, which is where a
//! double-apply or a manufactured receipt would hide.

use abacus_core::ports::{
    AssignDecision, AssignmentOpening, AssignmentRecord, AttemptRecord, AuditApplicationOutcome,
    AuditKind, AuditQuery, CredentialProvisioning, DecisionReason, EvidenceOutcome, FencedAction,
    FencedCall, HandoffRecord, OperationSet, StateApplied, SubmissionOutcome, WorkGraphPort,
    WorkStatus, WorkflowStatePort,
};
use abacus_core::usecase::{
    AcceptanceDecision, ProjectionOutcome, ProjectionUnresolved, RedriveOutcome, accept_handoff,
    record_evidence, redrive_pending, submit_handoff,
};
use abacus_core::{
    ActorId, AssignmentId, AttemptId, AuthorityClass, AuthoritySnapshot, BeadId, CapabilityId,
    CommitId, ContentHash, CredentialId, DecisionActor, EditScope, Evidence, FencingToken,
    HandoffId, Lease, OperationId, ProfileName, ScopeExpr, ScopeMap, Timestamp, WorkPath,
    WorkspaceDigest,
    assignment::AttemptPolicy,
    evidence::{
        AcceptancePolicy, Argv, FileDigestSet, PathSet, PolicyForm, VerificationOutcome,
        VerificationSet,
    },
};
use abacus_state::{InMemoryState, ManualClock};
use abacus_work::{FakeWorkProvider, Script, WorkFacade};

fn bead() -> BeadId {
    BeadId::new("ABACUS-journey.4").expect("valid bead id")
}

fn assignment_id() -> AssignmentId {
    AssignmentId::new("asg-journey-4").expect("valid assignment id")
}

fn attempt_id() -> AttemptId {
    AttemptId::new("att-journey-4").expect("valid attempt id")
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

fn authority(capability: &str) -> AuthoritySnapshot {
    AuthoritySnapshot {
        actor: lead(),
        capability: CapabilityId::new(capability).expect("valid capability"),
        scope: ScopeExpr::Universal,
    }
}

fn opening() -> AssignmentOpening {
    AssignmentOpening {
        assignment: AssignmentRecord {
            id: assignment_id(),
            bead: bead(),
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
            operation: op("op-journey-4-assign"),
            assignment: assignment_id(),
            first_attempt: attempt_id(),
            authority: authority("state:assign"),
        },
        bead_revision: abacus_core::ports::WorkRevision(hash('e')),
        worker_credential: CredentialProvisioning {
            id: CredentialId::new("cred-journey-4").expect("valid credential id"),
            digest: hash('f'),
        },
    }
}

fn worker_action(operation: &str) -> FencedAction {
    FencedAction {
        call: FencedCall {
            assignment: assignment_id(),
            attempt: attempt_id(),
            actor: worker().actor,
            token: FencingToken(1),
            operation: op(operation),
        },
        responds_to: None,
    }
}

fn verification() -> VerificationSet {
    VerificationSet::new(
        vec![Argv::new(vec!["cargo".into(), "test".into()]).expect("valid argv")],
        PathSet::new(vec![WorkPath::new("src/lib.rs").expect("valid path")])
            .expect("valid path set"),
    )
    .expect("valid verification set")
}

/// The worker's passing verification evidence, bound to the commit it
/// was produced at.
fn evidence() -> Evidence {
    let verification = verification();
    Evidence::new(
        verification.commands()[0].clone(),
        verification,
        0,
        VerificationOutcome::Pass,
        CommitId::new(&"9".repeat(40)).expect("valid commit"),
        WorkspaceDigest::new(&"1".repeat(64)).expect("valid workspace digest"),
        WorkspaceDigest::new(&"2".repeat(64)).expect("valid workspace digest"),
        None,
        FileDigestSet::default(),
        None,
    )
    .expect("coherent evidence")
}

/// The worker's immutable completion claim, binding the commit it hands
/// off and the evidence operations that justify it.
fn handoff() -> HandoffRecord {
    HandoffRecord {
        id: HandoffId::new("hnd-journey-4").expect("valid handoff id"),
        attempt: attempt_id(),
        commit: CommitId::new(&"9".repeat(40)).expect("valid commit"),
        expected_base: CommitId::new(&"d".repeat(40)).expect("valid commit"),
        clean_tree: WorkspaceDigest::new(&"3".repeat(64)).expect("valid workspace digest"),
        changed_paths: PathSet::new(vec![WorkPath::new("src/lib.rs").expect("valid path")])
            .expect("valid path set"),
        evidence_operations: OperationSet::new(vec![op("op-journey-4-evidence")])
            .expect("valid operation set"),
        attestation: hash('8'),
    }
}

fn acceptance() -> AcceptanceDecision {
    AcceptanceDecision {
        operation: op("op-journey-4-accept"),
        assignment: assignment_id(),
        authority: authority("state:accept"),
        handoff: HandoffId::new("hnd-journey-4").expect("valid handoff id"),
        reason: DecisionReason::new("verification passed").expect("valid reason"),
        resolves: None,
    }
}

#[test]
fn an_ambiguous_acceptance_application_stays_pending_until_reconciled() {
    // The provider will accept the mark-in-progress normally, then LOSE
    // the acknowledgement of the close - the mutation lands, the answer
    // does not come back.
    let work = WorkFacade::new(
        FakeWorkProvider::with_bead(&bead(), WorkStatus::Open, 1)
            .scripted(Script::new().ambiguous_lost()),
    );
    let state = InMemoryState::new(ManualClock::new(Timestamp(0)));
    assert_eq!(
        state.open_assignment(&opening()),
        Ok(StateApplied::Applied),
        "the Assignment opens, minting its mark-in-progress projection"
    );

    // 0. The work actually happens: the worker appends its passing
    // evidence and submits the Handoff that Acceptance will decide.
    // There is no accepting work that was never submitted - Scribe
    // derives the decided Attempt from the immutable Handoff record.
    assert_eq!(
        record_evidence(&state, &worker_action("op-journey-4-evidence"), &evidence())
            .expect("evidence append runs")
            .0,
        EvidenceOutcome::Recorded
    );
    let (submission, _) = submit_handoff(&state, &worker_action("op-journey-4-submit"), &handoff())
        .expect("handoff submission runs");
    assert_eq!(
        submission,
        SubmissionOutcome::Recorded {
            handoff: HandoffId::new("hnd-journey-4").expect("valid handoff id")
        },
        "a clean, evidenced Handoff is recorded"
    );

    // 1. Commit the Acceptance decision and drive its close projection.
    // The decision commits FIRST and independently: a projection that
    // does not confirm never unwinds it.
    let outcome = accept_handoff(&state, &work, &acceptance(), &op("op-journey-4-apply-1"))
        .expect("the saga runs");
    assert_eq!(outcome.decision, StateApplied::Applied);

    // 2. The close application is ambiguous. Production must record the
    // attempt and STOP - never manufacture a receipt for an unknown
    // outcome, and never blindly re-issue.
    assert_eq!(
        outcome.projection,
        Some(ProjectionOutcome::Unresolved {
            attempt: op("op-journey-4-apply-1"),
            reason: ProjectionUnresolved::Ambiguous,
        }),
        "an unknown outcome is recorded as unknown"
    );
    // Read through the real audit surface - the journey asserts what
    // production exposes, and never asks for a convenience accessor.
    let audited = state
        .audit_events(&AuditQuery {
            subject: None,
            class: None,
            from: None,
            through: None,
        })
        .expect("audit query succeeds");
    assert!(
        audited.iter().any(|event| matches!(
            &event.kind,
            AuditKind::ApplicationAttemptRecorded {
                outcome: AuditApplicationOutcome::Ambiguous
            }
        )),
        "the Ledger's attempt history states what this attempt actually \
         proved: nothing"
    );

    // 3. The projection REMAINS PENDING - this is the reconcilable
    // window the journey exists to pressure.
    let pending = state
        .pending_applications()
        .expect("pending query succeeds");
    assert_eq!(
        pending.len(),
        1,
        "the unconfirmed projection stays actionable"
    );
    assert_eq!(pending[0].operation, op("op-journey-4-accept"));

    // 4. EXPLICIT reconciliation - caller-invoked, under a FRESH attempt
    // identity, never a timer, watcher, or background sweep. The
    // provider now answers normally and the effect is observed.
    let redrive = redrive_pending(
        &state,
        &work,
        &op("op-journey-4-accept"),
        &op("op-journey-4-apply-2"),
    )
    .expect("reconciliation runs");

    // 5. The receipt clears the pending set.
    match redrive {
        RedriveOutcome::Driven(ProjectionOutcome::Projected { .. }) => {}
        other => panic!("expected reconciliation to confirm the effect, got {other:?}"),
    }
    assert!(
        state
            .pending_applications()
            .expect("pending query succeeds")
            .is_empty(),
        "a confirmed receipt clears the projection"
    );
    assert_eq!(
        work.inspect(&bead()).expect("bead is present").status,
        WorkStatus::Closed {
            observed_reason: abacus_core::ports::ObservedCloseReason::AcceptedHandoff
        },
        "the work graph reflects the accepted Handoff"
    );
}
