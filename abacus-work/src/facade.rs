//! The work facade: core ports implemented over the provider seam.
//!
//! This is where the module's depth lives. Provider adapters report
//! observations; the facade decides what they mean:
//!
//! - expected-revision preconditions (optimistic concurrency);
//! - idempotent re-application of an already-present effect;
//! - read-before-write reconciliation of an ambiguous mutation;
//! - bounding of audit-safe summaries;
//! - rejection of advice whose graph binding is stale or ineligible.

use std::collections::BTreeSet;

use abacus_core::ports::{
    AdviceDegradation, AdviceOutcome, BeadSnapshot, BeadStatusView, CloseReason, MutationOutcome,
    ObservedCloseReason, WorkAdvicePort, WorkError, WorkGraphPort, WorkRevision, WorkStatus,
};
use abacus_core::{BeadId, OperationId, ScopeKey};

use crate::adapter::{
    AdviceProvider, ProviderMutation, RawBeadSnapshot, RawBeadStatusView, TargetStatus,
    WorkProvider,
};
use crate::scope_labels::normalize_scope_labels;

/// Upper bound on a normalized mutation summary. Provider text is
/// already curated by the adapter; this is the last structural guard so
/// an unbounded provider string cannot reach audit (module contract:
/// "refusal of arbitrary/unbounded provider text").
pub const MAX_SUMMARY_LEN: usize = 256;

/// Implements [`WorkGraphPort`] over any [`WorkProvider`].
#[derive(Debug, Clone)]
pub struct WorkFacade<P> {
    provider: P,
    declared_scope_keys: Vec<ScopeKey>,
}

impl<P: WorkProvider> WorkFacade<P> {
    pub fn new(provider: P) -> Self {
        Self {
            provider,
            declared_scope_keys: Vec::new(),
        }
    }

    /// Construct a facade with the repository-declared scope keys. Provider
    /// labels remain raw until this boundary, where malformed and conflicting
    /// labels become deterministic ABACUS refusals.
    pub fn with_scope_keys(provider: P, declared_scope_keys: Vec<ScopeKey>) -> Self {
        Self {
            provider,
            declared_scope_keys,
        }
    }

    pub fn provider(&self) -> &P {
        &self.provider
    }

    fn normalize(&self, bead: RawBeadSnapshot) -> Result<BeadSnapshot, WorkError> {
        let scope_map = normalize_scope_labels(
            &self.declared_scope_keys,
            bead.raw_labels.iter().map(String::as_str),
        )?;
        let priority = abacus_core::ports::Priority::new(bead.priority)
            .map_err(|_| WorkError::MalformedOutput)?;
        Ok(BeadSnapshot {
            id: bead.id,
            content_hash: bead.content_hash,
            scope_map,
            priority,
        })
    }

    fn inspect_normalized(&self, id: &BeadId) -> Result<BeadStatusView, WorkError> {
        let RawBeadStatusView {
            snapshot,
            status,
            revision,
        } = self.provider.inspect(id)?;
        Ok(BeadStatusView {
            snapshot: self.normalize(snapshot)?,
            status,
            revision,
        })
    }

    /// Shared decision-gated mutation path for both projections.
    fn drive(
        &self,
        id: &BeadId,
        target: TargetStatus,
        operation: &OperationId,
        expected: &WorkRevision,
    ) -> Result<MutationOutcome, WorkError> {
        // Read before write. One inspection serves both the idempotency
        // check and the precondition, so the ordinary path costs one
        // extra read rather than a speculative mutation.
        let view = self.inspect_normalized(id)?;

        // A closed bead is TERMINAL at this seam. Report the observed
        // facts and let core correlate them against the Ledger; never
        // mutate. Without this, `close` on an already-cancelled bead
        // would silently re-close it as accepted, and `mark_in_progress`
        // would silently REOPEN it — both are the "silent adoption or
        // reversal" the module contract forbids. Nothing has been
        // submitted by this call, and that provenance is part of the
        // outcome: `FoundBeforeSubmission` is never receipt-eligible,
        // so a foreign effect can never be adopted as this operation's.
        if matches!(view.status, WorkStatus::Closed { .. }) {
            return Ok(MutationOutcome::FoundBeforeSubmission {
                status: view.status,
                revision: view.revision,
            });
        }

        // Idempotency is checked BEFORE the revision precondition on
        // purpose: a landed effect has already advanced the revision, so
        // the caller's `expected` is necessarily stale. Checking the
        // precondition first would report `RevisionConflict` for a
        // benign replay and push the caller toward a retry that must
        // never happen.
        if already_satisfies(view.status, target) {
            return Ok(MutationOutcome::FoundBeforeSubmission {
                status: view.status,
                revision: view.revision,
            });
        }

        if view.revision != *expected {
            return Err(WorkError::RevisionConflict);
        }

        match self.provider.set_status(id, target, operation)? {
            ProviderMutation::Applied {
                before,
                after,
                summary,
            } => Ok(MutationOutcome::Applied {
                before,
                after,
                summary: bound_summary(summary),
            }),
            // The command may or may not have landed. Inspect once and
            // let the observed state decide. Never re-issue: a blind
            // retry of a landed mutation is the double-apply this whole
            // seam exists to prevent.
            ProviderMutation::Ambiguous => {
                // If reconciliation ITSELF fails, the underlying error
                // must NOT surface: `ProviderUnavailable` or `Busy` reads
                // as "nothing happened, safe to retry later", when the
                // mutation may already have landed. `AmbiguousOutcome`
                // carries the only actionable truth — outcome unknown,
                // inspect before any retry — so a transport failure
                // during reconciliation cannot be mistaken for a
                // no-op and turned into a double-apply.
                let Ok(observed) = self.inspect_normalized(id) else {
                    return Err(WorkError::AmbiguousOutcome);
                };
                if already_satisfies(observed.status, target) {
                    // THIS call submitted the mutation; the observation
                    // is still not proof it was ours (a foreign matching
                    // mutation can win the race before this inspection),
                    // so the provenance-typed outcome stays ambiguous
                    // and is never receipt-eligible.
                    Ok(MutationOutcome::ObservedAfterAmbiguousSubmission {
                        status: observed.status,
                        revision: observed.revision,
                    })
                } else {
                    Err(WorkError::AmbiguousOutcome)
                }
            }
        }
    }
}

/// True when `status` already satisfies `target`, making the mutation a
/// no-op that must report its observed facts rather than reapply.
fn already_satisfies(status: WorkStatus, target: TargetStatus) -> bool {
    match (status, target) {
        (WorkStatus::InProgress, TargetStatus::InProgress) => true,
        (WorkStatus::Closed { observed_reason }, TargetStatus::Closed(reason)) => {
            same_reason(observed_reason, reason)
        }
        _ => false,
    }
}

/// A close is only a replay of the SAME decision when the observed
/// reason matches the curated one. `UnrecognizedProviderReason` never
/// matches: an out-of-band close is not this operation's effect, and
/// treating it as one would silently adopt a foreign mutation.
fn same_reason(observed: ObservedCloseReason, curated: CloseReason) -> bool {
    matches!(
        (observed, curated),
        (
            ObservedCloseReason::AcceptedHandoff,
            CloseReason::AcceptedHandoff
        ) | (
            ObservedCloseReason::CancelledObsolete,
            CloseReason::CancelledObsolete
        )
    )
}

/// Bound an audit summary without splitting a UTF-8 character.
fn bound_summary(raw: String) -> String {
    if raw.len() <= MAX_SUMMARY_LEN {
        return raw;
    }
    let mut end = MAX_SUMMARY_LEN;
    while end > 0 && !raw.is_char_boundary(end) {
        end -= 1;
    }
    raw[..end].to_owned()
}

impl<P: WorkProvider> WorkGraphPort for WorkFacade<P> {
    fn ready(&self) -> Result<(WorkRevision, Vec<BeadSnapshot>), WorkError> {
        let (revision, raw) = self.provider.ready()?;
        raw.into_iter()
            .map(|bead| self.normalize(bead))
            .collect::<Result<Vec<_>, _>>()
            .map(|beads| (revision, beads))
    }

    fn inspect(&self, id: &BeadId) -> Result<BeadStatusView, WorkError> {
        self.inspect_normalized(id)
    }

    fn mark_in_progress(
        &self,
        id: &BeadId,
        operation: &OperationId,
        expected: &WorkRevision,
    ) -> Result<MutationOutcome, WorkError> {
        self.drive(id, TargetStatus::InProgress, operation, expected)
    }

    fn close(
        &self,
        id: &BeadId,
        reason: CloseReason,
        operation: &OperationId,
        expected: &WorkRevision,
    ) -> Result<MutationOutcome, WorkError> {
        self.drive(id, TargetStatus::Closed(reason), operation, expected)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fake::{Call, FakeWorkProvider};

    #[test]
    fn idempotency_matrix_is_exhaustive() {
        let accepted = WorkStatus::Closed {
            observed_reason: ObservedCloseReason::AcceptedHandoff,
        };
        let cancelled = WorkStatus::Closed {
            observed_reason: ObservedCloseReason::CancelledObsolete,
        };
        let foreign = WorkStatus::Closed {
            observed_reason: ObservedCloseReason::UnrecognizedProviderReason,
        };

        assert!(already_satisfies(
            WorkStatus::InProgress,
            TargetStatus::InProgress
        ));
        assert!(already_satisfies(
            accepted,
            TargetStatus::Closed(CloseReason::AcceptedHandoff)
        ));
        assert!(already_satisfies(
            cancelled,
            TargetStatus::Closed(CloseReason::CancelledObsolete)
        ));

        // Open is never already-satisfied.
        assert!(!already_satisfies(
            WorkStatus::Open,
            TargetStatus::InProgress
        ));
        assert!(!already_satisfies(
            WorkStatus::Open,
            TargetStatus::Closed(CloseReason::AcceptedHandoff)
        ));
        // Cross-paired close reasons are distinct effects.
        assert!(!already_satisfies(
            accepted,
            TargetStatus::Closed(CloseReason::CancelledObsolete)
        ));
        assert!(!already_satisfies(
            cancelled,
            TargetStatus::Closed(CloseReason::AcceptedHandoff)
        ));
        // An out-of-band close is never adopted as our effect.
        assert!(!already_satisfies(
            foreign,
            TargetStatus::Closed(CloseReason::AcceptedHandoff)
        ));
        assert!(!already_satisfies(
            foreign,
            TargetStatus::Closed(CloseReason::CancelledObsolete)
        ));
        // A closed bead does not satisfy an in-progress target.
        assert!(!already_satisfies(accepted, TargetStatus::InProgress));
        // ...nor does in-progress satisfy a close.
        assert!(!already_satisfies(
            WorkStatus::InProgress,
            TargetStatus::Closed(CloseReason::AcceptedHandoff)
        ));
    }

    #[test]
    fn facade_normalizes_provider_labels_before_reads_or_mutations() {
        let id = BeadId::new("ABACUS-test").expect("valid bead id");
        let provider = FakeWorkProvider::with_bead(&id, WorkStatus::Open, 1)
            .with_raw_labels(&id, vec!["area:auth".to_owned(), "area:billing".to_owned()]);
        let facade = WorkFacade::with_scope_keys(
            provider,
            vec![abacus_core::ScopeKey::new("area").expect("valid scope key")],
        );

        let conflict = WorkError::ScopeLabelConflict {
            key: "area".to_owned(),
        };
        assert_eq!(facade.ready(), Err(conflict.clone()));
        assert_eq!(facade.inspect(&id), Err(conflict.clone()));

        let expected = abacus_core::ports::ExpectedWorkObservation {
            status: WorkStatus::Open,
            revision: crate::fake::rev(1),
            operation: OperationId::new("op-normalization-test").expect("valid operation id"),
        };
        assert_eq!(
            facade.compare_observation(&id, &expected),
            Err(conflict.clone())
        );
        assert_eq!(
            facade.mark_in_progress(&id, &expected.operation, &expected.revision),
            Err(conflict)
        );
        assert!(
            facade
                .provider()
                .calls()
                .iter()
                .all(|call| !matches!(call, Call::SetStatus { .. })),
            "scope-label refusal must occur before any provider mutation"
        );
    }

    #[test]
    fn short_summary_is_unchanged() {
        let raw = "ABACUS-omw.1 -> InProgress".to_owned();
        assert_eq!(bound_summary(raw.clone()), raw);
    }

    #[test]
    fn long_summary_is_truncated_to_the_bound() {
        let raw = "a".repeat(MAX_SUMMARY_LEN * 2);
        let bounded = bound_summary(raw);
        assert_eq!(bounded.len(), MAX_SUMMARY_LEN);
    }

    #[test]
    fn truncation_never_splits_a_multibyte_character() {
        // 'é' is two bytes, so the naive byte cut at MAX_SUMMARY_LEN
        // lands mid-character and would panic on a raw slice.
        let raw = "é".repeat(MAX_SUMMARY_LEN);
        let bounded = bound_summary(raw);

        assert!(bounded.len() <= MAX_SUMMARY_LEN);
        // Round-trips as valid UTF-8 with no replacement character.
        assert!(!bounded.contains('\u{fffd}'));
        assert!(bounded.chars().all(|c| c == 'é'));
    }

    #[test]
    fn boundary_length_summary_is_kept_whole() {
        let raw = "b".repeat(MAX_SUMMARY_LEN);
        assert_eq!(bound_summary(raw.clone()), raw);
    }
}

/// Implements [`WorkAdvicePort`] over any [`AdviceProvider`].
#[derive(Debug, Clone)]
pub struct AdviceFacade<A> {
    advisor: A,
}

impl<A: AdviceProvider> AdviceFacade<A> {
    pub fn new(advisor: A) -> Self {
        Self { advisor }
    }
}

impl<A: AdviceProvider> WorkAdvicePort for AdviceFacade<A> {
    fn advise(&self, revision: &WorkRevision, ready: &[BeadId]) -> AdviceOutcome {
        let analysis = match self.advisor.analyze(revision, ready) {
            Ok(analysis) => analysis,
            // Degradation is a noted outcome, never an error: core owns
            // the deterministic fallback (I8).
            Err(reason) => return AdviceOutcome::Degraded { reason },
        };

        // Advice is only meaningful against the graph the caller asked
        // about. A different analyzed revision means the advisor raced
        // the graph, so the ordering is stale by construction.
        if analysis.analyzed != *revision {
            return AdviceOutcome::Degraded {
                reason: AdviceDegradation::Malformed,
            };
        }

        // An advisor may only rank what it was given, and may rank each
        // bead once. Anything else is a malformed ranking, not a hint
        // worth partially trusting.
        let mut seen = BTreeSet::new();
        for id in &analysis.order {
            if !ready.contains(id) || !seen.insert(id.as_str()) {
                return AdviceOutcome::Degraded {
                    reason: AdviceDegradation::Malformed,
                };
            }
        }

        AdviceOutcome::Advice {
            order: analysis.order,
            bound_to: analysis.analyzed,
        }
    }
}
