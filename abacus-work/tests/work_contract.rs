//! Contract suite for the work facade.
//!
//! These run against the hermetic fake, but every assertion is stated
//! about the FACADE's obligations, so the `br` adapter bead
//! (ABACUS-omw.2) can re-run this suite against its fixture-driven
//! provider and inherit the same guarantees.
//!
//! Integration layer: composes the real crate through its public API
//! (`WorkFacade` over a `WorkProvider`), not internal helpers.

use abacus_core::ports::{
    AdviceDegradation, AdviceOutcome, CloseReason, MutationOutcome, ObservedCloseReason,
    WorkAdvicePort, WorkError, WorkGraphPort, WorkStatus,
};
use abacus_core::{BeadId, OperationId};
use abacus_work::contract::run_work_graph_suite;
use abacus_work::fake::{Call, FakeAdvisor, FakeWorkProvider, Script, rev};
use abacus_work::{AdviceFacade, MAX_SUMMARY_LEN, WorkFacade};

fn bead() -> BeadId {
    BeadId::new("ABACUS-omw.1").expect("valid bead id")
}

fn operation() -> OperationId {
    OperationId::new("op-omw-1").expect("valid operation id")
}

/// Count how many mutation commands the facade actually issued. The
/// reconciliation path must never issue a second one.
fn mutation_count(calls: &[Call]) -> usize {
    calls
        .iter()
        .filter(|call| matches!(call, Call::SetStatus { .. }))
        .count()
}

/// The portable suite, run against the hermetic fake.
///
/// This is the same entry point ABACUS-omw.2's `br` adapter will call
/// with a fixture-driven provider. Its passing here is what makes the
/// suite's reusability a verified property rather than an intention:
/// nothing inside `run_work_graph_suite` names `FakeWorkProvider`.
#[test]
fn the_portable_contract_suite_passes_against_the_fake() {
    run_work_graph_suite(FakeWorkProvider::from_scenario);
}

#[test]
fn ready_reports_open_work_with_its_revision() {
    let provider = FakeWorkProvider::with_bead(&bead(), WorkStatus::Open, 7);
    let facade = WorkFacade::new(provider);

    let (revision, snapshots) = facade.ready().expect("ready succeeds");

    assert_eq!(revision, rev(7));
    assert_eq!(snapshots.len(), 1);
    assert_eq!(snapshots[0].id, bead());
}

#[test]
fn ready_propagates_normalized_provider_failure() {
    let provider = FakeWorkProvider::with_bead(&bead(), WorkStatus::Open, 1)
        .failing_ready(WorkError::ProviderUnavailable);
    let facade = WorkFacade::new(provider);

    assert_eq!(facade.ready(), Err(WorkError::ProviderUnavailable));
}

#[test]
fn mark_in_progress_applies_on_matching_revision() {
    let provider = FakeWorkProvider::with_bead(&bead(), WorkStatus::Open, 3);
    let facade = WorkFacade::new(provider);

    let outcome = facade
        .mark_in_progress(&bead(), &operation(), &rev(3))
        .expect("mutation succeeds");

    match outcome {
        MutationOutcome::Applied { before, after, .. } => {
            assert_eq!(before, rev(3));
            assert_ne!(after, rev(3), "revision must advance");
        }
        other => panic!("expected Applied, got {other:?}"),
    }
    assert_eq!(
        facade.provider().status_of(&bead()),
        Some(WorkStatus::InProgress)
    );
}

#[test]
fn stale_expected_revision_refuses_without_mutating() {
    let provider = FakeWorkProvider::with_bead(&bead(), WorkStatus::Open, 5);
    let facade = WorkFacade::new(provider);

    let result = facade.mark_in_progress(&bead(), &operation(), &rev(4));

    assert_eq!(result, Err(WorkError::RevisionConflict));
    assert_eq!(
        mutation_count(&facade.provider().calls()),
        0,
        "a precondition failure must not reach the provider"
    );
    assert_eq!(facade.provider().status_of(&bead()), Some(WorkStatus::Open));
}

#[test]
fn already_in_target_state_is_idempotent() {
    let provider = FakeWorkProvider::with_bead(&bead(), WorkStatus::InProgress, 2);
    let facade = WorkFacade::new(provider);

    let outcome = facade
        .mark_in_progress(&bead(), &operation(), &rev(2))
        .expect("idempotent replay succeeds");

    match outcome {
        MutationOutcome::FoundBeforeSubmission { status, revision } => {
            assert_eq!(status, WorkStatus::InProgress);
            assert_eq!(revision, rev(2));
        }
        other => panic!("expected FoundBeforeSubmission, got {other:?}"),
    }
    assert_eq!(
        mutation_count(&facade.provider().calls()),
        0,
        "an already-present effect must not be reapplied"
    );
}

#[test]
fn ambiguous_outcome_that_landed_is_observed_not_confirmed() {
    let provider = FakeWorkProvider::with_bead(&bead(), WorkStatus::Open, 1)
        .scripted(Script::new().ambiguous_applied());
    let facade = WorkFacade::new(provider);

    let outcome = facade
        .mark_in_progress(&bead(), &operation(), &rev(1))
        .expect("reconciliation resolves the ambiguity");

    // This call DID submit; the observation is typed as exactly that -
    // observed after an ambiguous submission - because a foreign
    // matching mutation could have won the race, and only Applied is
    // receipt-eligible downstream.
    match outcome {
        MutationOutcome::ObservedAfterAmbiguousSubmission { status, .. } => {
            assert_eq!(status, WorkStatus::InProgress);
        }
        other => panic!("expected ObservedAfterAmbiguousSubmission, got {other:?}"),
    }
    assert_eq!(
        mutation_count(&facade.provider().calls()),
        1,
        "reconciliation must re-inspect, never re-issue the mutation"
    );
}

#[test]
fn ambiguous_outcome_that_was_lost_fails_loud() {
    let provider = FakeWorkProvider::with_bead(&bead(), WorkStatus::Open, 1)
        .scripted(Script::new().ambiguous_lost());
    let facade = WorkFacade::new(provider);

    let result = facade.mark_in_progress(&bead(), &operation(), &rev(1));

    assert_eq!(
        result,
        Err(WorkError::AmbiguousOutcome),
        "an unlanded ambiguous mutation must fail loud, not silently retry"
    );
    assert_eq!(mutation_count(&facade.provider().calls()), 1);
    assert_eq!(facade.provider().status_of(&bead()), Some(WorkStatus::Open));
}

#[test]
fn a_failed_reconciliation_reports_ambiguity_not_the_transport_error() {
    // The mutation DID land, then the reconciling inspect failed. If the
    // caller saw `ProviderUnavailable` it would read as "nothing
    // happened, retry later" and double-apply. Only `AmbiguousOutcome`
    // carries the actionable truth: inspect before any retry.
    let provider = FakeWorkProvider::with_bead(&bead(), WorkStatus::Open, 1)
        .scripted(Script::new().ambiguous_applied())
        .failing_inspect_after_mutation(WorkError::ProviderUnavailable);
    let facade = WorkFacade::new(provider);

    assert_eq!(
        facade.mark_in_progress(&bead(), &operation(), &rev(1)),
        Err(WorkError::AmbiguousOutcome),
        "a transport failure during reconciliation must not read as a safe-to-retry no-op"
    );
    assert_eq!(
        mutation_count(&facade.provider().calls()),
        1,
        "reconciliation must not re-issue the mutation even when it fails"
    );
}

#[test]
fn close_projects_curated_reason_to_observed_reason() {
    let provider = FakeWorkProvider::with_bead(&bead(), WorkStatus::Open, 9);
    let facade = WorkFacade::new(provider);

    facade
        .close(&bead(), CloseReason::AcceptedHandoff, &operation(), &rev(9))
        .expect("close succeeds");

    assert_eq!(
        facade.provider().status_of(&bead()),
        Some(WorkStatus::Closed {
            observed_reason: ObservedCloseReason::AcceptedHandoff
        })
    );
}

#[test]
fn close_is_idempotent_only_for_the_matching_observed_reason() {
    let cancelled = WorkStatus::Closed {
        observed_reason: ObservedCloseReason::CancelledObsolete,
    };
    let provider = FakeWorkProvider::with_bead(&bead(), cancelled, 4);
    let facade = WorkFacade::new(provider);

    // Already closed for a DIFFERENT curated reason. The seam must NOT
    // re-close it as accepted: it reports the OBSERVED reason so core
    // can correlate against its Ledger decision and fail loud. Silently
    // overwriting a cancellation with an acceptance is exactly the
    // "silent adoption or reversal" the module contract forbids.
    let outcome = facade
        .close(&bead(), CloseReason::AcceptedHandoff, &operation(), &rev(4))
        .expect("provider is reachable");

    match outcome {
        MutationOutcome::FoundBeforeSubmission { status, .. } => {
            assert_eq!(
                status,
                WorkStatus::Closed {
                    observed_reason: ObservedCloseReason::CancelledObsolete
                },
                "the OBSERVED reason must be reported, not the requested one"
            );
        }
        other => panic!("a closed bead is terminal; expected observed facts, got {other:?}"),
    }

    assert_eq!(
        mutation_count(&facade.provider().calls()),
        0,
        "a terminal bead must never be mutated"
    );
    assert_eq!(
        facade.provider().status_of(&bead()),
        Some(cancelled),
        "the cancellation must survive intact"
    );
}

#[test]
fn a_closed_bead_is_never_silently_reopened() {
    let closed = WorkStatus::Closed {
        observed_reason: ObservedCloseReason::AcceptedHandoff,
    };
    let provider = FakeWorkProvider::with_bead(&bead(), closed, 6);
    let facade = WorkFacade::new(provider);

    let outcome = facade
        .mark_in_progress(&bead(), &operation(), &rev(6))
        .expect("provider is reachable");

    assert!(
        matches!(outcome, MutationOutcome::FoundBeforeSubmission { .. }),
        "expected observed facts for a terminal bead, got {outcome:?}"
    );
    assert_eq!(
        facade.provider().status_of(&bead()),
        Some(closed),
        "marking a closed bead in progress must not reopen it"
    );
    assert_eq!(mutation_count(&facade.provider().calls()), 0);
}

#[test]
fn provider_errors_reach_the_caller_normalized() {
    let provider = FakeWorkProvider::with_bead(&bead(), WorkStatus::Open, 1)
        .scripted(Script::new().fail(WorkError::Busy));
    let facade = WorkFacade::new(provider);

    assert_eq!(
        facade.mark_in_progress(&bead(), &operation(), &rev(1)),
        Err(WorkError::Busy)
    );
}

#[test]
fn advice_bound_to_the_asked_revision_is_returned() {
    let advisor = FakeAdvisor::advising(vec![bead()], rev(6));
    let facade = AdviceFacade::new(advisor);

    let outcome = facade.advise(&rev(6), &[bead()]);

    match outcome {
        AdviceOutcome::Advice { order, bound_to } => {
            assert_eq!(order, vec![bead()]);
            assert_eq!(bound_to, rev(6));
        }
        other => panic!("expected Advice, got {other:?}"),
    }
}

#[test]
fn advice_bound_to_a_stale_revision_is_rejected() {
    let advisor = FakeAdvisor::advising(vec![bead()], rev(5));
    let facade = AdviceFacade::new(advisor);

    assert_eq!(
        facade.advise(&rev(6), &[bead()]),
        AdviceOutcome::Degraded {
            reason: AdviceDegradation::Malformed
        },
        "advice analyzed against a different graph revision is stale"
    );
}

#[test]
fn advice_naming_an_ineligible_bead_is_rejected() {
    let other = BeadId::new("ABACUS-omw.2").expect("valid bead id");
    let advisor = FakeAdvisor::advising(vec![other], rev(6));
    let facade = AdviceFacade::new(advisor);

    assert_eq!(
        facade.advise(&rev(6), &[bead()]),
        AdviceOutcome::Degraded {
            reason: AdviceDegradation::Malformed
        },
        "advice may only order beads the caller presented as ready"
    );
}

#[test]
fn advice_repeating_a_bead_is_rejected() {
    let advisor = FakeAdvisor::advising(vec![bead(), bead()], rev(6));
    let facade = AdviceFacade::new(advisor);

    assert_eq!(
        facade.advise(&rev(6), &[bead()]),
        AdviceOutcome::Degraded {
            reason: AdviceDegradation::Malformed
        },
        "a duplicate ordering is not a valid ranking"
    );
}

#[test]
fn advisor_degradation_is_a_noted_outcome_not_an_error() {
    let facade = AdviceFacade::new(FakeAdvisor::degraded(AdviceDegradation::Timeout));

    assert_eq!(
        facade.advise(&rev(1), &[bead()]),
        AdviceOutcome::Degraded {
            reason: AdviceDegradation::Timeout
        }
    );
}

#[test]
fn mutation_summary_is_bounded() {
    let provider = FakeWorkProvider::with_bead(&bead(), WorkStatus::Open, 1);
    let facade = WorkFacade::new(provider);

    let outcome = facade
        .mark_in_progress(&bead(), &operation(), &rev(1))
        .expect("mutation succeeds");

    if let MutationOutcome::Applied { summary, .. } = outcome {
        assert!(summary.len() <= MAX_SUMMARY_LEN);
    }
}
