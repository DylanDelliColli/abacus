//! Journey path 1 of 4: the complete happy path.
//!
//! Ready selection through Assignment opening and its confirmed
//! mark-in-progress projection, driven ENTIRELY through production
//! composition (`abacus_core::usecase`) over the canonical fakes. This
//! file supplies inputs and assertions only: every transition, gate,
//! and receipt decision belongs to production code.

use abacus_core::ports::{
    AdviceDisposition, AssignDecision, AssignmentOpening, AssignmentRecord, AttemptRecord,
    StateApplied, WorkGraphPort, WorkStatus, WorkflowStatePort,
};
use abacus_core::usecase::{AssignmentOutcome, ProjectionOutcome, assign_ready, select_ready};
use abacus_core::{
    ActorId, AssignmentId, AttemptId, AuthorityClass, AuthoritySnapshot, BeadId, CapabilityId,
    CommitId, ContentHash, DecisionActor, EditScope, FencingToken, Lease, OperationId, ProfileName,
    ScopeExpr, Timestamp, WorkPath,
    assignment::AttemptPolicy,
    evidence::{AcceptancePolicy, Argv, PathSet, PolicyForm, VerificationSet},
};
use abacus_state::{InMemoryState, ManualClock};
use abacus_work::{AdviceFacade, FakeAdvisor, FakeWorkProvider, WorkFacade};

fn bead() -> BeadId {
    BeadId::new("ABACUS-journey.1").expect("valid bead id")
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

/// The opening bundle the orchestrator composes from a selected
/// snapshot. Deliberately literal: no builder, no DSL.
fn opening_from(
    snapshot: &abacus_core::ports::BeadSnapshot,
    revision: &abacus_core::ports::WorkRevision,
) -> AssignmentOpening {
    let assignment = AssignmentId::new("asg-journey-1").expect("valid assignment id");
    let attempt = AttemptId::new("att-journey-1").expect("valid attempt id");
    AssignmentOpening {
        assignment: AssignmentRecord {
            id: assignment.clone(),
            bead: snapshot.id.clone(),
            // Snapshotted from what the graph actually offered - the
            // production binding check rejects anything else.
            bead_content_hash: snapshot.content_hash.clone(),
            scope_map: snapshot.scope_map.clone(),
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
            id: attempt.clone(),
            assignment: assignment.clone(),
            lease: Lease {
                token: FencingToken(1),
                expires_at: Timestamp(1_000),
            },
        },
        authorizing: AssignDecision {
            operation: op("op-journey-assign"),
            assignment,
            first_attempt: attempt,
            authority: AuthoritySnapshot {
                actor: lead(),
                capability: CapabilityId::new("state:assign").expect("valid capability"),
                scope: ScopeExpr::Universal,
            },
        },
        bead_revision: revision.clone(),
    }
}

#[test]
fn ready_selection_through_confirmed_assignment_projection() {
    // Canonical fakes: one open bead, an advisor bound to the graph, and
    // the in-memory Ledger under a manual clock.
    let work = WorkFacade::new(FakeWorkProvider::with_bead(&bead(), WorkStatus::Open, 1));
    let graph_revision = work.inspect(&bead()).expect("bead is present").revision;
    let advice = AdviceFacade::new(FakeAdvisor::advising(vec![bead()], graph_revision.clone()));
    let state = InMemoryState::new(ManualClock::new(Timestamp(0)));

    // 1. Ready selection, advice-gated by production policy.
    let selection = select_ready(&work, &advice).expect("ready selection succeeds");
    assert_eq!(selection.order, vec![bead()], "the advisor's order governs");
    assert_eq!(
        selection.advice,
        AdviceDisposition::Followed,
        "advice bound to the bracketed revision is followed, and the \
         disposition is surfaced rather than erased"
    );

    // 2. Assignment opening, bound to the exact selected snapshot, with
    // its mark-in-progress projection driven under the SAME authorizing
    // operation - all inside production composition.
    let snapshot = selection
        .ready
        .iter()
        .find(|candidate| candidate.id == bead())
        .expect("the selected bead is in the ready set");
    let opening = opening_from(snapshot, &selection.revision);

    let outcome = assign_ready(
        &state,
        &work,
        &selection,
        &opening,
        &op("op-journey-apply-1"),
    )
    .expect("assignment succeeds");

    assert_eq!(
        outcome,
        AssignmentOutcome::Opened {
            opening: StateApplied::Applied,
            projection: Some(ProjectionOutcome::Projected {
                after: work.inspect(&bead()).expect("bead is present").revision,
            }),
        }
    );

    // 3. The machine's end state: the graph moved, and the Ledger holds
    // no unconfirmed projection.
    assert_eq!(
        work.inspect(&bead()).expect("bead is present").status,
        WorkStatus::InProgress,
        "the work graph reflects the committed decision"
    );
    assert!(
        state
            .pending_applications()
            .expect("pending query succeeds")
            .is_empty(),
        "a confirmed projection leaves nothing pending"
    );
}
