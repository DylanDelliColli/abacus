//! The reusable provider contract suite.
//!
//! The point of this module: an expectation stated here holds for ANY
//! [`WorkProvider`], so the `br` process adapter (ABACUS-omw.2) proves
//! its conformance by calling [`run_work_graph_suite`] with a
//! fixture-driven provider instead of restating these cases.
//!
//! Every assertion here goes through the FACADE and reads only what the
//! core port returns. Nothing inspects provider internals — that is what
//! makes the suite portable. Interaction-level checks that need
//! introspection (how many commands were issued, what the stored status
//! became) stay in the hermetic fake's own tests, where introspection
//! actually exists.

use abacus_core::ports::{
    CloseReason, MutationOutcome, ObservedCloseReason, WorkError, WorkGraphPort, WorkRevision,
    WorkStatus,
};
use abacus_core::{BeadId, OperationId};

use crate::adapter::WorkProvider;
use crate::facade::{MAX_SUMMARY_LEN, WorkFacade};

/// How the provider under test should behave for one scenario.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Behavior {
    /// Mutations apply and acknowledge normally.
    Normal,
    /// The mutation lands but the acknowledgement is lost.
    AmbiguousApplied,
    /// The mutation does not land and the outcome is unknown.
    AmbiguousLost,
    /// The provider fails with a normalized error.
    Fails(WorkError),
}

/// One provider state the suite asks an implementation to materialize.
///
/// An adapter maps this onto whatever its substrate needs — an in-memory
/// map for the fake, a checked-in fixture directory for `br`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Scenario {
    /// The single bead the scenario is about.
    pub bead: BeadId,
    /// Its status before the facade is exercised.
    pub status: WorkStatus,
    /// Seed for its starting revision; the suite reads the revision back
    /// rather than assuming a representation.
    pub tick: u32,
    pub behavior: Behavior,
}

impl Scenario {
    pub fn new(bead: BeadId, status: WorkStatus, tick: u32, behavior: Behavior) -> Self {
        Self {
            bead,
            status,
            tick,
            behavior,
        }
    }
}

fn operation() -> OperationId {
    OperationId::new("op-contract-1").expect("valid operation id")
}

/// Read the bead's current revision through the port, so the suite never
/// assumes how an implementation derives revisions.
fn current_revision<P: WorkProvider>(facade: &WorkFacade<P>, bead: &BeadId) -> WorkRevision {
    facade
        .inspect(bead)
        .expect("inspect must succeed for a present bead")
        .revision
}

/// Run every provider-agnostic expectation.
///
/// `build` is called once per scenario and must return a provider in
/// exactly that state. Panics with a descriptive message on the first
/// violated expectation, so it reads like an ordinary test failure.
pub fn run_work_graph_suite<P, F>(build: F)
where
    P: WorkProvider,
    F: Fn(&Scenario) -> P,
{
    let bead = BeadId::new("ABACUS-omw.1").expect("valid bead id");

    applies_on_matching_revision(&build, &bead);
    refuses_a_stale_expected_revision(&build, &bead);
    is_idempotent_when_the_effect_is_present(&build, &bead);
    reconciles_an_ambiguous_landed_mutation(&build, &bead);
    fails_loud_on_an_ambiguous_lost_mutation(&build, &bead);
    propagates_a_normalized_provider_error(&build, &bead);
    closes_with_a_curated_reason(&build, &bead);
    bounds_the_audit_summary(&build, &bead);
    treats_a_closed_bead_as_terminal(&build, &bead);
    never_reopens_a_closed_bead(&build, &bead);
}

/// A closed bead is terminal: a close carrying a DIFFERENT curated
/// reason must report the observed facts, never overwrite the recorded
/// outcome. Silently re-closing a cancellation as an acceptance is the
/// "silent adoption or reversal" the module contract forbids, and core
/// can only fail loud if this seam surfaces what it actually observed.
fn treats_a_closed_bead_as_terminal<P: WorkProvider>(
    build: &impl Fn(&Scenario) -> P,
    bead: &BeadId,
) {
    let cancelled = WorkStatus::Closed {
        observed_reason: ObservedCloseReason::CancelledObsolete,
    };
    let facade = WorkFacade::new(build(&Scenario::new(
        bead.clone(),
        cancelled,
        4,
        Behavior::Normal,
    )));
    let expected = current_revision(&facade, bead);

    match facade.close(bead, CloseReason::AcceptedHandoff, &operation(), &expected) {
        Ok(MutationOutcome::EffectAlreadyPresent { status, .. }) => {
            assert_eq!(
                status, cancelled,
                "the OBSERVED close reason must be reported, not the requested one"
            );
        }
        other => panic!("a closed bead is terminal; expected observed facts, got {other:?}"),
    }

    assert_eq!(
        facade.inspect(bead).expect("bead is still present").status,
        cancelled,
        "the recorded cancellation must survive intact"
    );
}

/// The same terminality in the other direction: a decision-gated
/// transition toward in-progress must never reopen a closed bead.
fn never_reopens_a_closed_bead<P: WorkProvider>(build: &impl Fn(&Scenario) -> P, bead: &BeadId) {
    let closed = WorkStatus::Closed {
        observed_reason: ObservedCloseReason::AcceptedHandoff,
    };
    let facade = WorkFacade::new(build(&Scenario::new(
        bead.clone(),
        closed,
        6,
        Behavior::Normal,
    )));
    let expected = current_revision(&facade, bead);

    let outcome = facade.mark_in_progress(bead, &operation(), &expected);
    assert!(
        matches!(outcome, Ok(MutationOutcome::EffectAlreadyPresent { .. })),
        "expected observed facts for a terminal bead, got {outcome:?}"
    );
    assert_eq!(
        facade.inspect(bead).expect("bead is still present").status,
        closed,
        "a closed bead must not be reopened"
    );
}

fn applies_on_matching_revision<P: WorkProvider>(build: &impl Fn(&Scenario) -> P, bead: &BeadId) {
    let facade = WorkFacade::new(build(&Scenario::new(
        bead.clone(),
        WorkStatus::Open,
        3,
        Behavior::Normal,
    )));
    let expected = current_revision(&facade, bead);

    match facade.mark_in_progress(bead, &operation(), &expected) {
        Ok(MutationOutcome::Applied { before, after, .. }) => {
            assert_eq!(
                before, expected,
                "reported `before` must be the precondition"
            );
            assert_ne!(
                after, before,
                "an applied mutation must advance the revision"
            );
        }
        other => panic!("expected Applied on a matching revision, got {other:?}"),
    }
}

fn refuses_a_stale_expected_revision<P: WorkProvider>(
    build: &impl Fn(&Scenario) -> P,
    bead: &BeadId,
) {
    let facade = WorkFacade::new(build(&Scenario::new(
        bead.clone(),
        WorkStatus::Open,
        5,
        Behavior::Normal,
    )));
    // Any revision that is not the current one is stale by definition.
    let current = current_revision(&facade, bead);
    let stale = WorkRevision(crate::fake::hash(9_999));
    assert_ne!(
        stale, current,
        "test fixture must supply a truly stale revision"
    );

    assert_eq!(
        facade.mark_in_progress(bead, &operation(), &stale),
        Err(WorkError::RevisionConflict),
        "a stale precondition must refuse before mutating"
    );
}

fn is_idempotent_when_the_effect_is_present<P: WorkProvider>(
    build: &impl Fn(&Scenario) -> P,
    bead: &BeadId,
) {
    let facade = WorkFacade::new(build(&Scenario::new(
        bead.clone(),
        WorkStatus::InProgress,
        2,
        Behavior::Normal,
    )));
    let current = current_revision(&facade, bead);

    match facade.mark_in_progress(bead, &operation(), &current) {
        Ok(MutationOutcome::EffectAlreadyPresent { status, .. }) => {
            assert_eq!(status, WorkStatus::InProgress);
        }
        other => panic!("expected EffectAlreadyPresent for a present effect, got {other:?}"),
    }
}

fn reconciles_an_ambiguous_landed_mutation<P: WorkProvider>(
    build: &impl Fn(&Scenario) -> P,
    bead: &BeadId,
) {
    let facade = WorkFacade::new(build(&Scenario::new(
        bead.clone(),
        WorkStatus::Open,
        1,
        Behavior::AmbiguousApplied,
    )));
    let expected = current_revision(&facade, bead);

    match facade.mark_in_progress(bead, &operation(), &expected) {
        Ok(MutationOutcome::EffectAlreadyPresent { status, .. }) => {
            assert_eq!(
                status,
                WorkStatus::InProgress,
                "reconciliation must observe the landed effect"
            );
        }
        other => panic!("expected reconciliation to EffectAlreadyPresent, got {other:?}"),
    }
}

fn fails_loud_on_an_ambiguous_lost_mutation<P: WorkProvider>(
    build: &impl Fn(&Scenario) -> P,
    bead: &BeadId,
) {
    let facade = WorkFacade::new(build(&Scenario::new(
        bead.clone(),
        WorkStatus::Open,
        1,
        Behavior::AmbiguousLost,
    )));
    let expected = current_revision(&facade, bead);

    assert_eq!(
        facade.mark_in_progress(bead, &operation(), &expected),
        Err(WorkError::AmbiguousOutcome),
        "an unlanded ambiguous mutation must fail loud rather than retry"
    );
}

fn propagates_a_normalized_provider_error<P: WorkProvider>(
    build: &impl Fn(&Scenario) -> P,
    bead: &BeadId,
) {
    let facade = WorkFacade::new(build(&Scenario::new(
        bead.clone(),
        WorkStatus::Open,
        1,
        Behavior::Fails(WorkError::Busy),
    )));
    let expected = current_revision(&facade, bead);

    assert_eq!(
        facade.mark_in_progress(bead, &operation(), &expected),
        Err(WorkError::Busy),
        "a normalized provider error must reach the caller unchanged"
    );

    // Enforces the `set_status` Err contract: `Err` asserts the mutation
    // definitively did not take effect, and the facade skips
    // reconciliation on that basis. An adapter that returns `Err` for a
    // mutation that DID land would make a retry look safe and
    // double-apply, so conformance is checked here rather than trusted.
    let after = facade.inspect(bead).expect("bead is still present");
    assert_eq!(
        after.status,
        WorkStatus::Open,
        "a provider error must leave the bead unmutated"
    );
    assert_eq!(
        after.revision, expected,
        "a provider error must not advance the revision"
    );
}

fn closes_with_a_curated_reason<P: WorkProvider>(build: &impl Fn(&Scenario) -> P, bead: &BeadId) {
    let facade = WorkFacade::new(build(&Scenario::new(
        bead.clone(),
        WorkStatus::Open,
        9,
        Behavior::Normal,
    )));
    let expected = current_revision(&facade, bead);

    let outcome = facade.close(bead, CloseReason::AcceptedHandoff, &operation(), &expected);
    assert!(
        matches!(outcome, Ok(MutationOutcome::Applied { .. })),
        "close on a matching revision must apply, got {outcome:?}"
    );

    let after = facade.inspect(bead).expect("bead is present after close");
    assert!(
        matches!(after.status, WorkStatus::Closed { .. }),
        "the bead must actually be closed, got {:?}",
        after.status
    );
}

fn bounds_the_audit_summary<P: WorkProvider>(build: &impl Fn(&Scenario) -> P, bead: &BeadId) {
    let facade = WorkFacade::new(build(&Scenario::new(
        bead.clone(),
        WorkStatus::Open,
        1,
        Behavior::Normal,
    )));
    let expected = current_revision(&facade, bead);

    if let Ok(MutationOutcome::Applied { summary, .. }) =
        facade.mark_in_progress(bead, &operation(), &expected)
    {
        assert!(
            summary.len() <= MAX_SUMMARY_LEN,
            "audit summary must be bounded, got {} bytes",
            summary.len()
        );
    }
}
