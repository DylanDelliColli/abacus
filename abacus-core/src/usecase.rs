//! The use-case composition module (ADR-0001 amendment, 2026-08-05).
//!
//! Functions generic over the provider-neutral port traits that
//! **sequence** already-defined port and domain decisions. This module
//! adds no dependency, holds no provider knowledge, and duplicates no
//! transition policy: lifecycle rules, gates, and refusals stay where
//! they are. Compensation and reconciliation are explicit outcomes
//! returned to the caller — never hidden retries or background repair
//! (CONTEXT I12).
//!
//! `abacus-cli` calls these functions; the hermetic vertical journey
//! drives them. Neither re-implements them.

use crate::OperationId;
use crate::ports::{
    ApplicationAttempt, ApplicationOutcome, ApplicationReceipt, DecisionRecord, MutationOutcome,
    PendingApplication, StateApplied, StateError, WorkError, WorkGraphPort, WorkProjection,
    WorkRevision, WorkflowStatePort,
};

/// Why a committed decision's provider projection is not yet
/// confirmed. Both variants leave the projection in the derived
/// pending set for explicit, caller-invoked reconciliation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProjectionUnresolved {
    /// The provider refused definitively; the effect did not happen.
    Failed(WorkError),
    /// The outcome is unknown: the mutation may have landed. Inspect
    /// before any retry — never re-issue blindly.
    Ambiguous,
}

/// Outcome of projecting ONE committed decision onto the work graph.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProjectionOutcome {
    /// The provider effect is confirmed and its receipt is recorded,
    /// clearing this projection from the pending set.
    Projected { after: WorkRevision },
    /// The attempt is recorded immutably, but no receipt exists: the
    /// projection stays pending and reconcilable.
    Unresolved {
        attempt: OperationId,
        reason: ProjectionUnresolved,
    },
}

/// Outcome of the Acceptance saga (ADR-0001 §3).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AcceptanceOutcome {
    /// Whether the authorizing decision was committed now or was
    /// already committed by an identical earlier call.
    pub decision: StateApplied,
    /// The projection result, or `None` when the committed decision
    /// carries no work-graph projection at all.
    pub projection: Option<ProjectionOutcome>,
}

/// Project one pending application onto the work graph and record what
/// happened.
///
/// Ordering is the saga's whole point: the authorizing decision is
/// already committed, so this only *confirms* a projection. The
/// application attempt is recorded immutably whatever happened; only a
/// confirmed success also records the receipt that clears the pending
/// set. An ambiguous outcome records the attempt and stops — the
/// caller reconciles explicitly.
pub fn project_pending<S, W>(
    state: &S,
    work: &W,
    pending: &PendingApplication,
    attempt_operation: &OperationId,
) -> Result<ProjectionOutcome, StateError>
where
    S: WorkflowStatePort + ?Sized,
    W: WorkGraphPort + ?Sized,
{
    let expected = pending
        .authorized_revision
        .clone()
        .unwrap_or_else(|| current_revision_placeholder(&pending.projection));

    let mutation = match &pending.projection {
        WorkProjection::MarkInProgress => {
            work.mark_in_progress(&pending.bead, &pending.operation, &expected)
        }
        WorkProjection::Close { reason } => {
            work.close(&pending.bead, *reason, &pending.operation, &expected)
        }
    };

    let outcome = match mutation {
        Ok(MutationOutcome::Applied { before, after, .. }) => {
            ApplicationOutcome::Applied { before, after }
        }
        Ok(MutationOutcome::EffectAlreadyPresent { status, revision }) => {
            ApplicationOutcome::EffectAlreadyPresent { status, revision }
        }
        Err(WorkError::AmbiguousOutcome) => ApplicationOutcome::Ambiguous,
        Err(error) => ApplicationOutcome::Failed { error },
    };

    let attempt = ApplicationAttempt {
        id: attempt_operation.clone(),
        target: pending.operation.clone(),
        outcome: outcome.clone(),
    };
    state.record_application_attempt(&attempt)?;

    // Only a confirmed effect yields the receipt that clears the
    // projection. `EffectAlreadyPresent` counts: the effect IS present,
    // which is exactly what the receipt attests.
    let after = match outcome {
        ApplicationOutcome::Applied { after, .. } => after,
        ApplicationOutcome::EffectAlreadyPresent { revision, .. } => revision,
        ApplicationOutcome::Ambiguous => {
            return Ok(ProjectionOutcome::Unresolved {
                attempt: attempt_operation.clone(),
                reason: ProjectionUnresolved::Ambiguous,
            });
        }
        ApplicationOutcome::Failed { error } => {
            return Ok(ProjectionOutcome::Unresolved {
                attempt: attempt_operation.clone(),
                reason: ProjectionUnresolved::Failed(error),
            });
        }
    };

    state.record_application_receipt(&ApplicationReceipt {
        target: pending.operation.clone(),
        attempt: attempt_operation.clone(),
        after: after.clone(),
    })?;
    Ok(ProjectionOutcome::Projected { after })
}

/// The Acceptance saga: commit the authorizing decision, then project
/// it onto the work graph under its own operation identity.
///
/// The decision is committed FIRST and independently: it authorizes
/// the provider mutation, and a projection failure never unwinds it
/// (there is no `accepting` state to roll back to). A decision whose
/// projection did not confirm simply remains in the derived pending
/// set until reconciled.
pub fn accept_handoff<S, W>(
    state: &S,
    work: &W,
    decision: &DecisionRecord,
    attempt_operation: &OperationId,
) -> Result<AcceptanceOutcome, StateError>
where
    S: WorkflowStatePort + ?Sized,
    W: WorkGraphPort + ?Sized,
{
    let applied = state.record_decision(decision)?;
    let pending = state
        .pending_applications()?
        .into_iter()
        .find(|candidate| candidate.operation == decision.operation);
    let projection = match pending {
        None => None,
        Some(pending) => Some(project_pending(state, work, &pending, attempt_operation)?),
    };
    Ok(AcceptanceOutcome {
        decision: applied,
        projection,
    })
}

/// Explicitly reconcile every projection still lacking a receipt.
///
/// Caller-invoked by an operator or authorized decision actor (I12:
/// no timer, watcher, or background sweep). Each pending projection
/// gets its own fresh attempt operation, supplied by the caller in
/// order, so a reconciliation attempt is never confused with the
/// original.
pub fn reconcile_pending<S, W>(
    state: &S,
    work: &W,
    attempt_operations: &[OperationId],
) -> Result<Vec<(OperationId, ProjectionOutcome)>, StateError>
where
    S: WorkflowStatePort + ?Sized,
    W: WorkGraphPort + ?Sized,
{
    let pending = state.pending_applications()?;
    let mut results = Vec::with_capacity(pending.len().min(attempt_operations.len()));
    for (item, attempt) in pending.iter().zip(attempt_operations) {
        let outcome = project_pending(state, work, item, attempt)?;
        results.push((item.operation.clone(), outcome));
    }
    Ok(results)
}

/// A projection recorded without an authorized revision cannot carry a
/// meaningful precondition; the provider's own revision check governs.
/// Kept explicit rather than silently defaulting.
fn current_revision_placeholder(_projection: &WorkProjection) -> WorkRevision {
    WorkRevision(crate::ContentHash::new(&"0".repeat(64)).expect("zero revision is valid 64-hex"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ports::*;
    use crate::*;
    use std::cell::RefCell;

    fn op(raw: &str) -> OperationId {
        OperationId::new(raw).expect("valid operation id")
    }

    fn rev(fill: char) -> WorkRevision {
        WorkRevision(ContentHash::new(&fill.to_string().repeat(64)).expect("valid hash"))
    }

    fn bead() -> BeadId {
        BeadId::new("ABACUS-usecase.1").expect("valid bead id")
    }

    fn pending_close() -> PendingApplication {
        PendingApplication {
            operation: op("op-accept"),
            assignment: AssignmentId::new("asg-1").expect("valid assignment"),
            bead: bead(),
            projection: WorkProjection::Close {
                reason: CloseReason::AcceptedHandoff,
            },
            committed_at: Seq(1),
            authorized_revision: Some(rev('e')),
        }
    }

    /// Records what the saga asked the work seam to do, and answers
    /// with a scripted outcome.
    struct ScriptedWork {
        answer: RefCell<Option<Result<MutationOutcome, WorkError>>>,
        closes: RefCell<Vec<(BeadId, CloseReason, OperationId, WorkRevision)>>,
    }

    impl ScriptedWork {
        fn new(answer: Result<MutationOutcome, WorkError>) -> Self {
            Self {
                answer: RefCell::new(Some(answer)),
                closes: RefCell::new(Vec::new()),
            }
        }
    }

    impl WorkGraphPort for ScriptedWork {
        fn ready(&self) -> Result<(WorkRevision, Vec<BeadSnapshot>), WorkError> {
            unimplemented!("the saga never lists ready work")
        }

        fn inspect(&self, _id: &BeadId) -> Result<BeadStatusView, WorkError> {
            unimplemented!("the saga never inspects directly")
        }

        fn mark_in_progress(
            &self,
            _id: &BeadId,
            _operation: &OperationId,
            _expected: &WorkRevision,
        ) -> Result<MutationOutcome, WorkError> {
            unimplemented!("this scenario projects a close")
        }

        fn close(
            &self,
            id: &BeadId,
            reason: CloseReason,
            operation: &OperationId,
            expected: &WorkRevision,
        ) -> Result<MutationOutcome, WorkError> {
            self.closes.borrow_mut().push((
                id.clone(),
                reason,
                operation.clone(),
                expected.clone(),
            ));
            self.answer
                .borrow_mut()
                .take()
                .expect("the saga issues at most one mutation per projection")
        }
    }

    #[derive(Default)]
    struct RecordingState {
        pending: RefCell<Vec<PendingApplication>>,
        attempts: RefCell<Vec<ApplicationAttempt>>,
        receipts: RefCell<Vec<ApplicationReceipt>>,
        decisions: RefCell<Vec<DecisionRecord>>,
    }

    impl WorkflowStatePort for RecordingState {
        fn record_decision(&self, record: &DecisionRecord) -> Result<StateApplied, StateError> {
            self.decisions.borrow_mut().push(record.clone());
            Ok(StateApplied::Applied)
        }

        fn record_application_attempt(
            &self,
            attempt: &ApplicationAttempt,
        ) -> Result<StateApplied, StateError> {
            self.attempts.borrow_mut().push(attempt.clone());
            Ok(StateApplied::Applied)
        }

        fn record_application_receipt(
            &self,
            receipt: &ApplicationReceipt,
        ) -> Result<StateApplied, StateError> {
            self.receipts.borrow_mut().push(receipt.clone());
            // A receipt clears its projection from the derived set.
            self.pending
                .borrow_mut()
                .retain(|item| item.operation != receipt.target);
            Ok(StateApplied::Applied)
        }

        fn pending_applications(&self) -> Result<Vec<PendingApplication>, StateError> {
            Ok(self.pending.borrow().clone())
        }

        fn open_assignment(&self, _: &AssignmentOpening) -> Result<StateApplied, StateError> {
            unimplemented!()
        }
        fn append_attempt(&self, _: &AttemptOpening) -> Result<StateApplied, StateError> {
            unimplemented!()
        }
        fn activate_profile(&self, _: &ActivationOpening) -> Result<StateApplied, StateError> {
            unimplemented!()
        }
        fn deactivate_profile(
            &self,
            _: &OperationId,
            _: &ActorId,
            _: &ProfileName,
        ) -> Result<StateApplied, StateError> {
            unimplemented!()
        }
        fn append_signal(&self, _: &SignalDraft) -> Result<(Signal, StateApplied), StateError> {
            unimplemented!()
        }
        fn fenced_report(
            &self,
            _: &FencedAction,
            _: &SignalDraft,
        ) -> Result<(ReportOutcome, FencedResponse), StateError> {
            unimplemented!()
        }
        fn fenced_evidence(
            &self,
            _: &FencedAction,
            _: &Evidence,
        ) -> Result<(EvidenceOutcome, FencedResponse), StateError> {
            unimplemented!()
        }
        fn fenced_submit_handoff(
            &self,
            _: &FencedAction,
            _: &HandoffRecord,
        ) -> Result<(SubmissionOutcome, FencedResponse), StateError> {
            unimplemented!()
        }
        fn fenced_abort_attempt(&self, _: &FencedCall) -> Result<FencedResponse, StateError> {
            unimplemented!()
        }
        fn renew_lease(
            &self,
            _: &FencedCall,
            _: Timestamp,
        ) -> Result<(Lease, FencedResponse), StateError> {
            unimplemented!()
        }
        fn persist_envelope(
            &self,
            _: &OperationId,
            _: &LaunchSubject,
            _: &EnvelopeSnapshot,
        ) -> Result<StateApplied, StateError> {
            unimplemented!()
        }
        fn envelope(&self, _: &LaunchSubject) -> Result<EnvelopeSnapshot, StateError> {
            unimplemented!()
        }
        fn bind_runtime_handle(
            &self,
            _: &OperationId,
            _: &LaunchSubject,
            _: &RuntimeHandle,
        ) -> Result<StateApplied, StateError> {
            unimplemented!()
        }
        fn unbind_runtime_handle(
            &self,
            _: &OperationId,
            _: &LaunchSubject,
        ) -> Result<StateApplied, StateError> {
            unimplemented!()
        }
        fn runtime_handle(&self, _: &LaunchSubject) -> Result<Option<RuntimeHandle>, StateError> {
            unimplemented!()
        }
        fn record_runtime_observation(
            &self,
            _: &OperationId,
            _: &RuntimeObservationRecord,
        ) -> Result<StateApplied, StateError> {
            unimplemented!()
        }
        fn runtime_observation(
            &self,
            _: &OperationId,
        ) -> Result<RuntimeObservationRecord, StateError> {
            unimplemented!()
        }
        fn assignment(&self, _: &AssignmentId) -> Result<AssignmentView, StateError> {
            unimplemented!()
        }
        fn evidence_for(&self, _: &AttemptId) -> Result<Vec<EvidenceRecord>, StateError> {
            unimplemented!()
        }
        fn signals_for(&self, _: &AttemptId) -> Result<Vec<Signal>, StateError> {
            unimplemented!()
        }
        fn verify_launch_subject(
            &self,
            _: &LaunchSubject,
            _: &ContentHash,
        ) -> Result<(), StateError> {
            unimplemented!()
        }
        fn handoff(&self, _: &HandoffId) -> Result<HandoffRecord, StateError> {
            unimplemented!()
        }
        fn decision(&self, _: &OperationId) -> Result<DecisionRecord, StateError> {
            unimplemented!()
        }
        fn active_occupants(&self, _: &ProfileName) -> Result<Vec<ActorId>, StateError> {
            unimplemented!()
        }
        fn unresolved_signals(&self, _: Option<&ActorId>) -> Result<Vec<Signal>, StateError> {
            unimplemented!()
        }
        fn audit_events(&self, _: &AuditQuery) -> Result<Vec<AuditEvent>, StateError> {
            unimplemented!()
        }
    }

    #[test]
    fn a_confirmed_projection_records_attempt_then_receipt() {
        let state = RecordingState::default();
        state.pending.borrow_mut().push(pending_close());
        let work = ScriptedWork::new(Ok(MutationOutcome::Applied {
            before: rev('e'),
            after: rev('9'),
            summary: "closed".to_owned(),
        }));

        let outcome = project_pending(&state, &work, &pending_close(), &op("app-1"))
            .expect("projection runs");
        assert_eq!(outcome, ProjectionOutcome::Projected { after: rev('9') });

        // The authorizing operation - not the attempt's own id - drives
        // the provider mutation, under the authorized revision.
        let closes = work.closes.borrow();
        assert_eq!(closes.len(), 1);
        assert_eq!(closes[0].2, op("op-accept"));
        assert_eq!(closes[0].3, rev('e'));
        assert_eq!(closes[0].1, CloseReason::AcceptedHandoff);

        assert_eq!(state.attempts.borrow().len(), 1);
        assert_eq!(state.receipts.borrow().len(), 1);
        assert_eq!(state.receipts.borrow()[0].attempt, op("app-1"));
        assert!(
            state.pending.borrow().is_empty(),
            "a receipt clears its projection"
        );
    }

    #[test]
    fn an_already_present_effect_still_earns_its_receipt() {
        let state = RecordingState::default();
        state.pending.borrow_mut().push(pending_close());
        let work = ScriptedWork::new(Ok(MutationOutcome::EffectAlreadyPresent {
            status: WorkStatus::Closed {
                observed_reason: ObservedCloseReason::AcceptedHandoff,
            },
            revision: rev('7'),
        }));

        assert_eq!(
            project_pending(&state, &work, &pending_close(), &op("app-2")).expect("runs"),
            ProjectionOutcome::Projected { after: rev('7') },
            "the effect IS present, which is what the receipt attests"
        );
        assert_eq!(state.receipts.borrow().len(), 1);
    }

    #[test]
    fn an_ambiguous_mutation_records_the_attempt_and_no_receipt() {
        let state = RecordingState::default();
        state.pending.borrow_mut().push(pending_close());
        let work = ScriptedWork::new(Err(WorkError::AmbiguousOutcome));

        assert_eq!(
            project_pending(&state, &work, &pending_close(), &op("app-3")).expect("runs"),
            ProjectionOutcome::Unresolved {
                attempt: op("app-3"),
                reason: ProjectionUnresolved::Ambiguous
            }
        );
        assert_eq!(
            state.attempts.borrow()[0].outcome,
            ApplicationOutcome::Ambiguous,
            "the attempt is recorded immutably whatever happened"
        );
        assert!(
            state.receipts.borrow().is_empty(),
            "an unknown outcome must never manufacture a receipt"
        );
        assert_eq!(
            state.pending.borrow().len(),
            1,
            "the projection stays pending and reconcilable"
        );
    }

    #[test]
    fn a_definite_failure_is_recorded_and_stays_pending() {
        let state = RecordingState::default();
        state.pending.borrow_mut().push(pending_close());
        let work = ScriptedWork::new(Err(WorkError::Busy));

        assert_eq!(
            project_pending(&state, &work, &pending_close(), &op("app-4")).expect("runs"),
            ProjectionOutcome::Unresolved {
                attempt: op("app-4"),
                reason: ProjectionUnresolved::Failed(WorkError::Busy)
            }
        );
        assert_eq!(
            state.attempts.borrow()[0].outcome,
            ApplicationOutcome::Failed {
                error: WorkError::Busy
            }
        );
        assert!(state.receipts.borrow().is_empty());
    }

    #[test]
    fn reconciliation_redrives_only_still_pending_projections() {
        let state = RecordingState::default();
        state.pending.borrow_mut().push(pending_close());
        let work = ScriptedWork::new(Ok(MutationOutcome::Applied {
            before: rev('e'),
            after: rev('9'),
            summary: "closed on reconcile".to_owned(),
        }));

        let results = reconcile_pending(&state, &work, &[op("app-retry")]).expect("reconciles");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].0, op("op-accept"));
        assert_eq!(
            results[0].1,
            ProjectionOutcome::Projected { after: rev('9') }
        );
        assert!(state.pending.borrow().is_empty());

        // Nothing is left pending, so a further reconciliation is a
        // no-op rather than a second mutation.
        let again = reconcile_pending(&state, &work, &[op("app-retry-2")]).expect("reconciles");
        assert!(again.is_empty());
    }
}
