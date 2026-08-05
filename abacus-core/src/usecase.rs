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

use crate::ports::{
    AdviceDisposition, ApplicationAttempt, ApplicationOutcome, ApplicationReceipt,
    AssignmentOpening, BeadSnapshot, CloseReason, DecisionKind, DecisionReason, DecisionRecord,
    EphemeralLaunchSecret, EvidenceOutcome, FencedAction, FencedResponse, HandoffRecord,
    LaunchAttempt, LaunchCorrelation, LaunchOutcome, LaunchSpec, LaunchSubject, MutationOutcome,
    ObservedCloseReason, PendingApplication, ReceiptCandidate, ReportOutcome, RuntimeError,
    RuntimePort, StateApplied, StateError, SubmissionOutcome, WorkAdvicePort, WorkError,
    WorkGraphPort, WorkProjection, WorkRevision, WorkStatus, WorkflowStatePort, appraise_advice,
};
use crate::{
    AssignmentId, AuthoritySnapshot, BeadId, Evidence, HandoffId, OperationId, SignalDraft,
    SignalId,
};

/// Why a committed decision's provider projection is not yet
/// confirmed. Every variant leaves the projection in the derived
/// pending set for explicit, caller-invoked reconciliation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProjectionUnresolved {
    /// The provider refused definitively; the effect did not happen.
    Failed(WorkError),
    /// The outcome is unknown: the mutation may have landed. Inspect
    /// before any retry — never re-issue blindly.
    Ambiguous,
    /// The provider reports an already-present effect this operation
    /// must not adopt as its own: found before anything was submitted
    /// (whether or not the facts match the intended projection — br
    /// cannot attest whose effect it is), or observed with facts that
    /// do not satisfy the projection. Re-driving cannot resolve it —
    /// resolution is a decision, not a retry (ADR-0001
    /// effect-provenance amendment).
    ConflictingEffect {
        status: WorkStatus,
        revision: WorkRevision,
    },
    /// This operation's own submission was ambiguous and a later
    /// observation found the intended facts in place: possibly
    /// applied, never confirmed. Stays pending; never a receipt.
    PossiblyApplied {
        status: WorkStatus,
        revision: WorkRevision,
    },
}

/// Outcome of projecting ONE committed decision onto the work graph.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProjectionOutcome {
    /// The provider effect is confirmed and its receipt is recorded,
    /// clearing this projection from the pending set.
    Projected { after: WorkRevision },
    /// The recoverable crash window: a previously recorded `Applied`
    /// attempt lacked only its receipt, which is now recorded directly
    /// — no provider mutation was issued and no fresh attempt identity
    /// was consumed. `attempt` names the ORIGINAL applied attempt the
    /// receipt credits.
    ReceiptRecovered {
        attempt: OperationId,
        after: WorkRevision,
    },
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
///
/// Deliberately NOT public: every `PendingApplication` fed here must be
/// state-derived. A caller-constructed value could pair a real target
/// operation with a substituted bead, projection, or revision — the
/// substituted bead would be mutated while the attempt and receipt
/// attest against the real target, and Scribe's receipt validation
/// (target/attempt/after) cannot see the substitution. Public callers
/// name a target operation instead ([`redrive_pending`]) and the
/// Ledger's own record is fetched.
fn project_pending<S, W>(
    state: &S,
    work: &W,
    pending: &PendingApplication,
    attempt_operation: &OperationId,
) -> Result<ProjectionOutcome, StateError>
where
    S: WorkflowStatePort + ?Sized,
    W: WorkGraphPort + ?Sized,
{
    // The recoverable crash window outranks everything else: a
    // previously recorded Applied attempt lacking only its receipt is
    // recovered directly — no provider mutation, no fresh attempt
    // identity (ADR-0001 effect-provenance amendment).
    if let Some(candidate) = &pending.receipt_candidate {
        return recover_receipt(state, pending, candidate);
    }

    let expected = match pending.authorized_revision.clone() {
        Some(revision) => revision,
        // The authorizing decision bound no bead revision (the Accept
        // path does not): the precondition is a fresh observation of
        // the graph. A failed observation is a definite non-effect —
        // no mutation was issued — recorded and left pending.
        None => match work.inspect(&pending.bead) {
            Ok(view) => view.revision,
            Err(error) => {
                state.record_application_attempt(&ApplicationAttempt {
                    id: attempt_operation.clone(),
                    target: pending.operation.clone(),
                    outcome: ApplicationOutcome::Failed {
                        error: error.clone(),
                    },
                })?;
                return Ok(ProjectionOutcome::Unresolved {
                    attempt: attempt_operation.clone(),
                    reason: ProjectionUnresolved::Failed(error),
                });
            }
        },
    };

    let mutation = match &pending.projection {
        WorkProjection::MarkInProgress => {
            work.mark_in_progress(&pending.bead, &pending.operation, &expected)
        }
        WorkProjection::Close { reason } => {
            work.close(&pending.bead, *reason, &pending.operation, &expected)
        }
    };

    // The recorded outcome mirrors the mutation's provenance, so the
    // Ledger's attempt history states what each attempt actually
    // proved (ADR-0001 effect-provenance amendment).
    let outcome = match &mutation {
        Ok(MutationOutcome::Applied { before, after, .. }) => ApplicationOutcome::Applied {
            before: before.clone(),
            after: after.clone(),
        },
        Ok(MutationOutcome::FoundBeforeSubmission { status, revision }) => {
            ApplicationOutcome::FoundPresent {
                status: *status,
                revision: revision.clone(),
            }
        }
        Ok(MutationOutcome::ObservedAfterAmbiguousSubmission { status, revision }) => {
            ApplicationOutcome::ObservedAfterAmbiguous {
                status: *status,
                revision: revision.clone(),
            }
        }
        Err(WorkError::AmbiguousOutcome) => ApplicationOutcome::Ambiguous,
        Err(error) => ApplicationOutcome::Failed {
            error: error.clone(),
        },
    };

    let attempt = ApplicationAttempt {
        id: attempt_operation.clone(),
        target: pending.operation.clone(),
        outcome,
    };
    state.record_application_attempt(&attempt)?;

    // Only a provider-attested application yields the receipt that
    // clears the projection (ADR-0001 effect-provenance amendment). An
    // effect found before submission cannot be proven ours however
    // well the facts match; an observation after our own ambiguous
    // submission is possibly applied, never confirmed. Both stay
    // pending — resolution is a decision, not a retry.
    let after = match mutation {
        Ok(MutationOutcome::Applied { after, .. }) => after,
        Ok(MutationOutcome::FoundBeforeSubmission { status, revision }) => {
            return Ok(ProjectionOutcome::Unresolved {
                attempt: attempt_operation.clone(),
                reason: ProjectionUnresolved::ConflictingEffect { status, revision },
            });
        }
        Ok(MutationOutcome::ObservedAfterAmbiguousSubmission { status, revision }) => {
            let reason = if effect_satisfies(&pending.projection, status) {
                ProjectionUnresolved::PossiblyApplied { status, revision }
            } else {
                ProjectionUnresolved::ConflictingEffect { status, revision }
            };
            return Ok(ProjectionOutcome::Unresolved {
                attempt: attempt_operation.clone(),
                reason,
            });
        }
        Err(WorkError::AmbiguousOutcome) => {
            return Ok(ProjectionOutcome::Unresolved {
                attempt: attempt_operation.clone(),
                reason: ProjectionUnresolved::Ambiguous,
            });
        }
        Err(error) => {
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

/// The typed authorizing input of the Acceptance saga: an Accept and
/// nothing else. Mirrors [`crate::ports::AssignDecision`]'s rule (S2):
/// a generic decision cannot invoke a saga whose contract is
/// Acceptance — Reject, Cancel, Revoke, Reclaim, and Transfer are
/// recorded through the state port directly and are structurally
/// unrepresentable here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AcceptanceDecision {
    pub operation: OperationId,
    pub assignment: AssignmentId,
    pub authority: AuthoritySnapshot,
    /// The immutable Handoff this acceptance decides; the decided
    /// Attempt is derived transactionally from it.
    pub handoff: HandoffId,
    pub reason: DecisionReason,
    /// A Signal this decision resolves, if any.
    pub resolves: Option<SignalId>,
}

/// The Acceptance saga: commit the authorizing Accept decision, then
/// project it onto the work graph under its own operation identity.
///
/// The decision is committed FIRST and independently: it authorizes
/// the provider mutation, and a projection failure never unwinds it
/// (there is no `accepting` state to roll back to). A decision whose
/// projection did not confirm simply remains in the derived pending
/// set until reconciled.
pub fn accept_handoff<S, W>(
    state: &S,
    work: &W,
    decision: &AcceptanceDecision,
    attempt_operation: &OperationId,
) -> Result<AcceptanceOutcome, StateError>
where
    S: WorkflowStatePort + ?Sized,
    W: WorkGraphPort + ?Sized,
{
    let record = DecisionRecord {
        operation: decision.operation.clone(),
        assignment: decision.assignment.clone(),
        authority: decision.authority.clone(),
        kind: DecisionKind::Accept {
            handoff: decision.handoff.clone(),
            reason: decision.reason.clone(),
        },
        resolves: decision.resolves.clone(),
    };
    let applied = state.record_decision(&record)?;
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

/// Outcome of one explicit reconciliation pass. A pass bounded by the
/// supplied attempt identities is legitimate; silently truncating one
/// is not, so the uncovered remainder is named.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReconciliationPass {
    /// Each redriven projection's authorizing operation and outcome.
    pub reconciled: Vec<(OperationId, ProjectionOutcome)>,
    /// Pending projections the supplied attempt identities did not
    /// cover, in the derived set's order. Non-empty means: mint more
    /// attempt operations and reconcile again.
    pub unattempted: Vec<OperationId>,
}

/// Explicitly reconcile projections still lacking a receipt.
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
) -> Result<ReconciliationPass, StateError>
where
    S: WorkflowStatePort + ?Sized,
    W: WorkGraphPort + ?Sized,
{
    let pending = state.pending_applications()?;
    let mut reconciled = Vec::with_capacity(pending.len());
    let mut unattempted = Vec::new();
    let mut attempts = attempt_operations.iter();
    for item in &pending {
        // Receipt recovery is mutation-free and consumes no fresh
        // attempt identity, so a candidate never competes with the
        // supplied attempt operations.
        if let Some(candidate) = &item.receipt_candidate {
            reconciled.push((
                item.operation.clone(),
                recover_receipt(state, item, candidate)?,
            ));
            continue;
        }
        match attempts.next() {
            Some(attempt) => {
                let outcome = project_pending(state, work, item, attempt)?;
                reconciled.push((item.operation.clone(), outcome));
            }
            None => unattempted.push(item.operation.clone()),
        }
    }
    Ok(ReconciliationPass {
        reconciled,
        unattempted,
    })
}

/// Record the receipt a previously `Applied` attempt already earned
/// (the recoverable crash window): mutation-free, crediting the
/// ORIGINAL attempt identity.
fn recover_receipt<S>(
    state: &S,
    pending: &PendingApplication,
    candidate: &ReceiptCandidate,
) -> Result<ProjectionOutcome, StateError>
where
    S: WorkflowStatePort + ?Sized,
{
    state.record_application_receipt(&ApplicationReceipt {
        target: pending.operation.clone(),
        attempt: candidate.attempt.clone(),
        after: candidate.after.clone(),
    })?;
    Ok(ProjectionOutcome::ReceiptRecovered {
        attempt: candidate.attempt.clone(),
        after: candidate.after.clone(),
    })
}

/// Outcome of explicitly redriving ONE pending projection by its
/// authorizing operation identity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RedriveOutcome {
    /// No projection with that identity is pending: it was never
    /// committed, or it already earned its receipt. Nothing was
    /// mutated or recorded.
    NotPending,
    Driven(ProjectionOutcome),
}

/// Explicitly redrive one pending projection, named by its authorizing
/// operation identity.
///
/// The Ledger's own pending record is fetched and driven — the caller
/// supplies no projection facts, so a substituted bead, projection, or
/// revision is unrepresentable at this surface.
pub fn redrive_pending<S, W>(
    state: &S,
    work: &W,
    target: &OperationId,
    attempt_operation: &OperationId,
) -> Result<RedriveOutcome, StateError>
where
    S: WorkflowStatePort + ?Sized,
    W: WorkGraphPort + ?Sized,
{
    let pending = state
        .pending_applications()?
        .into_iter()
        .find(|candidate| candidate.operation == *target);
    match pending {
        None => Ok(RedriveOutcome::NotPending),
        Some(pending) => Ok(RedriveOutcome::Driven(project_pending(
            state,
            work,
            &pending,
            attempt_operation,
        )?)),
    }
}

/// The advice-gated ready selection: the bracketed ready snapshot, the
/// order composition dispatches in, and how the advisor's answer was
/// disposed of.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReadySelection {
    /// The revision the ready set (and any accepted advice) is bound to.
    pub revision: WorkRevision,
    pub ready: Vec<BeadSnapshot>,
    /// The advisor's permutation only when bound to `revision` and
    /// exactly covering `ready`; the deterministic fallback otherwise
    /// (core invariant 6 — the single advice gate decides, here).
    pub order: Vec<BeadId>,
    /// The gate's disposition of the advisor's answer. Degradation and
    /// rejection are noted outcomes, never errors and never erased
    /// (CONTEXT: "the degradation is noted in output").
    pub advice: AdviceDisposition,
}

/// Select ready work: one bracketed read of the ready set, advice
/// solicited against exactly that revision, and the gate applied.
///
/// Advice is never authoritative and never required (I8): a degraded,
/// stale, or non-covering answer yields the deterministic fallback
/// order with the reason noted in the selection — a normal outcome
/// rather than an error.
pub fn select_ready<W, A>(work: &W, advice: &A) -> Result<ReadySelection, WorkError>
where
    W: WorkGraphPort + ?Sized,
    A: WorkAdvicePort + ?Sized,
{
    let (revision, ready) = work.ready()?;
    let eligible: Vec<BeadId> = ready.iter().map(|bead| bead.id.clone()).collect();
    let outcome = advice.advise(&revision, &eligible);
    let (order, disposition) = appraise_advice(&ready, &outcome, &revision);
    Ok(ReadySelection {
        revision,
        ready,
        order,
        advice: disposition,
    })
}

/// Why an opening bundle was refused BEFORE any state or work
/// mutation: it is not bound to the ready selection it claims to
/// realize (architecture §2.4: an Assignment is created from an
/// ELIGIBLE bead snapshot).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AssignRefusal {
    /// The opening's bead is not in the selection's ready set.
    BeadNotReady { bead: BeadId },
    /// The opening's authorized bead revision is not the revision the
    /// selection was bracketed at.
    RevisionMismatch {
        authorized: WorkRevision,
        selection: WorkRevision,
    },
    /// The opening's bead content hash is not the selected snapshot's.
    ContentHashMismatch { bead: BeadId },
    /// The opening's scope map is not the selected snapshot's.
    ScopeMapMismatch { bead: BeadId },
}

/// Outcome of assigning ready work (architecture §2.4–2.5).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AssignmentOutcome {
    /// Refused before any state or work mutation: the opening is not
    /// bound to the given ready selection.
    Refused { refusal: AssignRefusal },
    /// The opening bundle committed (or was already committed by an
    /// identical earlier call), and its mark-in-progress projection
    /// was driven when one remained pending.
    Opened {
        opening: StateApplied,
        /// `None` when no pending projection remained (an idempotent
        /// replay after the original projection already earned its
        /// receipt).
        projection: Option<ProjectionOutcome>,
    },
}

/// Assign ready work: verify the opening is bound to the given ready
/// selection, commit the opening bundle, then project its
/// mark-in-progress onto the work graph under the same authorizing
/// operation identity.
///
/// The binding is verified FIRST, against the exact snapshot the
/// selection read — bead membership, bracketed revision, content hash,
/// and scope map — and a mismatch refuses before anything mutates. An
/// opening for a bead the graph never offered, or carrying facts other
/// than the snapshot's, must not reach Scribe: Scribe checks bundle
/// coherence, not readiness. After the commit this mirrors
/// [`accept_handoff`]: a projection failure never unwinds the opening.
pub fn assign_ready<S, W>(
    state: &S,
    work: &W,
    selection: &ReadySelection,
    opening: &AssignmentOpening,
    attempt_operation: &OperationId,
) -> Result<AssignmentOutcome, StateError>
where
    S: WorkflowStatePort + ?Sized,
    W: WorkGraphPort + ?Sized,
{
    let bead = &opening.assignment.bead;
    let Some(snapshot) = selection.ready.iter().find(|offered| offered.id == *bead) else {
        return Ok(AssignmentOutcome::Refused {
            refusal: AssignRefusal::BeadNotReady { bead: bead.clone() },
        });
    };
    if opening.bead_revision != selection.revision {
        return Ok(AssignmentOutcome::Refused {
            refusal: AssignRefusal::RevisionMismatch {
                authorized: opening.bead_revision.clone(),
                selection: selection.revision.clone(),
            },
        });
    }
    if opening.assignment.bead_content_hash != snapshot.content_hash {
        return Ok(AssignmentOutcome::Refused {
            refusal: AssignRefusal::ContentHashMismatch { bead: bead.clone() },
        });
    }
    if opening.assignment.scope_map != snapshot.scope_map {
        return Ok(AssignmentOutcome::Refused {
            refusal: AssignRefusal::ScopeMapMismatch { bead: bead.clone() },
        });
    }

    let applied = state.open_assignment(opening)?;
    let pending = state
        .pending_applications()?
        .into_iter()
        .find(|candidate| candidate.operation == opening.authorizing.operation);
    let projection = match pending {
        None => None,
        Some(pending) => Some(project_pending(state, work, &pending, attempt_operation)?),
    };
    Ok(AssignmentOutcome::Opened {
        opening: applied,
        projection,
    })
}

/// Outcome of the launch sequence (architecture §3.3–3.5): Envelope
/// persisted, launch attempted, handle associated. Every compensation
/// is an explicit returned outcome, never a hidden retry (I12).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LaunchSequenceOutcome {
    /// The session is live and its handle is durably associated. The
    /// caller must still read `launched.startup_delivery`: a live,
    /// bound session whose startup material definitely or possibly
    /// failed to deliver is stopped/reconciled by the caller's
    /// explicit decision.
    Launched {
        launched: LaunchOutcome,
        bound: StateApplied,
    },
    /// The session is live but the durable association write failed.
    /// The handle is surfaced so the caller can stop the session or
    /// re-bind explicitly — swallowing it would strand a live session.
    LaunchedUnbound {
        launched: LaunchOutcome,
        bind_error: StateError,
    },
    /// The provider may have created a session; no handle is known.
    /// Recovery is `RuntimePort::recover_launch` under the echoed
    /// correlation — never a relaunch.
    Ambiguous {
        subject: LaunchSubject,
        correlation: LaunchCorrelation,
    },
    /// Definite failure: no session exists. The persisted Envelope
    /// remains keyed by the subject for a future launch.
    NotLaunched { error: RuntimeError },
}

/// Launch a subject: persist the canonical Envelope FIRST, then launch
/// with the transient credential secret, then durably associate the
/// returned handle.
///
/// The ordering is the contract (architecture §3.3): nothing may reach
/// a live session that is not already durable, so a persist failure
/// aborts before any provider interaction. Subject/secret identity
/// agreement is the adapter's refusal to make (it must refuse before
/// any provider mutation); this sequence does not duplicate that gate.
pub fn launch_subject<S, R>(
    state: &S,
    runtime: &R,
    spec: &LaunchSpec,
    secret: EphemeralLaunchSecret,
    persist_operation: &OperationId,
    bind_operation: &OperationId,
) -> Result<LaunchSequenceOutcome, StateError>
where
    S: WorkflowStatePort + ?Sized,
    R: RuntimePort + ?Sized,
{
    state.persist_envelope(persist_operation, &spec.subject, &spec.envelope)?;

    let attempt = match runtime.launch(spec, secret) {
        Ok(attempt) => attempt,
        Err(error) => return Ok(LaunchSequenceOutcome::NotLaunched { error }),
    };

    match attempt {
        LaunchAttempt::Ambiguous {
            subject,
            correlation,
        } => Ok(LaunchSequenceOutcome::Ambiguous {
            subject,
            correlation,
        }),
        LaunchAttempt::Launched(launched) => {
            match state.bind_runtime_handle(bind_operation, &spec.subject, &launched.handle) {
                Ok(bound) => Ok(LaunchSequenceOutcome::Launched { launched, bound }),
                Err(bind_error) => Ok(LaunchSequenceOutcome::LaunchedUnbound {
                    launched,
                    bind_error,
                }),
            }
        }
    }
}

/// Fenced worker Report append — the worker-side composition surface.
///
/// These three pass-throughs exist so the CLI and the journey drive
/// ONE production surface for every worker action rather than the
/// port directly: the gate outcomes (including in-band Abort
/// refusals) and the fenced response envelope cross unaltered, and no
/// caller re-implements or reinterprets them. Policy lives in the
/// state seam's gates; nothing is added here by design.
pub fn record_report<S>(
    state: &S,
    action: &FencedAction,
    draft: &SignalDraft,
) -> Result<(ReportOutcome, FencedResponse), StateError>
where
    S: WorkflowStatePort + ?Sized,
{
    state.fenced_report(action, draft)
}

/// Fenced worker Evidence append — see [`record_report`].
pub fn record_evidence<S>(
    state: &S,
    action: &FencedAction,
    evidence: &Evidence,
) -> Result<(EvidenceOutcome, FencedResponse), StateError>
where
    S: WorkflowStatePort + ?Sized,
{
    state.fenced_evidence(action, evidence)
}

/// Fenced Handoff submission — see [`record_report`]. The submission
/// outcome (recorded or refused) is owned idempotently by the call's
/// operation and returned unaltered.
pub fn submit_handoff<S>(
    state: &S,
    action: &FencedAction,
    handoff: &HandoffRecord,
) -> Result<(SubmissionOutcome, FencedResponse), StateError>
where
    S: WorkflowStatePort + ?Sized,
{
    state.fenced_submit_handoff(action, handoff)
}

/// True when observed provider facts satisfy the intended projection.
/// Under the effect-provenance amendment this gate classifies ONLY the
/// observed-after-ambiguous-submission outcome (satisfying facts →
/// possibly applied; anything else → conflict) — matching facts alone
/// never earn a receipt for any provenance. An unrecognized provider
/// close reason never matches.
fn effect_satisfies(projection: &WorkProjection, status: WorkStatus) -> bool {
    match (projection, status) {
        (WorkProjection::MarkInProgress, WorkStatus::InProgress) => true,
        (WorkProjection::Close { reason }, WorkStatus::Closed { observed_reason }) => matches!(
            (reason, observed_reason),
            (
                CloseReason::AcceptedHandoff,
                ObservedCloseReason::AcceptedHandoff
            ) | (
                CloseReason::CancelledObsolete,
                ObservedCloseReason::CancelledObsolete
            )
        ),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ports::*;
    use crate::*;
    use std::cell::{Cell, RefCell};
    use std::rc::Rc;

    /// Cross-fake call-order log: the launch sequence's ordering
    /// contract spans the state and runtime seams.
    type EventLog = Rc<RefCell<Vec<&'static str>>>;

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
            receipt_candidate: None,
        }
    }

    fn snapshot(id: &str, priority: u8) -> BeadSnapshot {
        BeadSnapshot {
            id: BeadId::new(id).expect("valid bead id"),
            content_hash: rev('c').0,
            scope_map: ScopeMap::default(),
            priority: Priority::new(priority).expect("valid priority"),
        }
    }

    fn lead_actor() -> DecisionActor {
        DecisionActor {
            actor: ActorId::new("lead-1").expect("valid actor id"),
            class: AuthorityClass::Orchestrator,
            profile: ProfileName::new("lead").expect("valid profile"),
            profile_hash: rev('a').0,
        }
    }

    fn worker_actor() -> DecisionActor {
        DecisionActor {
            actor: ActorId::new("worker-1").expect("valid actor id"),
            class: AuthorityClass::Worker,
            profile: ProfileName::new("worker").expect("valid profile"),
            profile_hash: rev('b').0,
        }
    }

    fn verification() -> crate::evidence::VerificationSet {
        crate::evidence::VerificationSet::new(
            vec![Argv::new(vec!["cargo".into(), "test".into()]).expect("valid argv")],
            PathSet::new(vec![WorkPath::new("tests/usecase.rs").expect("valid path")])
                .expect("valid path set"),
        )
        .expect("valid verification set")
    }

    fn opening() -> AssignmentOpening {
        let assignment = AssignmentId::new("asg-1").expect("valid assignment");
        let attempt = AttemptId::new("att-1").expect("valid attempt");
        AssignmentOpening {
            assignment: AssignmentRecord {
                id: assignment.clone(),
                bead: bead(),
                bead_content_hash: rev('c').0,
                scope_map: ScopeMap::default(),
                worker: worker_actor(),
                decision_actor: lead_actor(),
                edit_scope: EditScope::new(vec![WorkPath::new("src").expect("valid path")])
                    .expect("valid edit scope"),
                acceptance: crate::evidence::AcceptancePolicy {
                    verification: verification(),
                    form: crate::evidence::PolicyForm::Standard,
                },
                attempt_policy: AttemptPolicy::default(),
                declared_base: CommitId::new(&"d".repeat(40)).expect("valid commit"),
            },
            first_attempt: AttemptRecord {
                id: attempt.clone(),
                assignment: assignment.clone(),
                lease: Lease {
                    token: FencingToken(1),
                    expires_at: Timestamp(100),
                },
            },
            authorizing: AssignDecision {
                operation: op("op-assign"),
                assignment,
                first_attempt: attempt,
                authority: AuthoritySnapshot {
                    actor: lead_actor(),
                    capability: CapabilityId::new("state:assign").expect("valid capability"),
                    scope: ScopeExpr::Universal,
                },
            },
            bead_revision: rev('e'),
            worker_credential: CredentialProvisioning {
                id: CredentialId::new("cred-1").expect("valid credential"),
                digest: rev('f').0,
            },
        }
    }

    /// Records what a use case asked the work seam to do, and answers
    /// with a scripted outcome shared by both mutating verbs (a
    /// projection issues at most one mutation).
    struct ScriptedWork {
        answer: RefCell<Option<Result<MutationOutcome, WorkError>>>,
        closes: RefCell<Vec<(BeadId, CloseReason, OperationId, WorkRevision)>>,
        marks: RefCell<Vec<(BeadId, OperationId, WorkRevision)>>,
        view: RefCell<Option<Result<BeadStatusView, WorkError>>>,
    }

    impl ScriptedWork {
        fn new(answer: Result<MutationOutcome, WorkError>) -> Self {
            Self {
                answer: RefCell::new(Some(answer)),
                closes: RefCell::new(Vec::new()),
                marks: RefCell::new(Vec::new()),
                view: RefCell::new(None),
            }
        }

        /// Script the single read-before-write inspection an
        /// unauthorized-revision projection performs.
        fn with_view(self, view: Result<BeadStatusView, WorkError>) -> Self {
            *self.view.borrow_mut() = Some(view);
            self
        }

        fn take_answer(&self) -> Result<MutationOutcome, WorkError> {
            self.answer
                .borrow_mut()
                .take()
                .expect("a use case issues at most one mutation per projection")
        }
    }

    impl WorkGraphPort for ScriptedWork {
        fn ready(&self) -> Result<(WorkRevision, Vec<BeadSnapshot>), WorkError> {
            unimplemented!("mutation scenarios never list ready work")
        }

        fn inspect(&self, _id: &BeadId) -> Result<BeadStatusView, WorkError> {
            self.view
                .borrow_mut()
                .take()
                .expect("this scenario scripts no inspection")
        }

        fn mark_in_progress(
            &self,
            id: &BeadId,
            operation: &OperationId,
            expected: &WorkRevision,
        ) -> Result<MutationOutcome, WorkError> {
            self.marks
                .borrow_mut()
                .push((id.clone(), operation.clone(), expected.clone()));
            self.take_answer()
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
            self.take_answer()
        }
    }

    /// Read-only work seam scripting exactly one bracketed ready set.
    struct ReadyWork {
        revision: WorkRevision,
        ready: Vec<BeadSnapshot>,
    }

    impl WorkGraphPort for ReadyWork {
        fn ready(&self) -> Result<(WorkRevision, Vec<BeadSnapshot>), WorkError> {
            Ok((self.revision.clone(), self.ready.clone()))
        }

        fn inspect(&self, _id: &BeadId) -> Result<BeadStatusView, WorkError> {
            unimplemented!("selection never inspects")
        }

        fn mark_in_progress(
            &self,
            _id: &BeadId,
            _operation: &OperationId,
            _expected: &WorkRevision,
        ) -> Result<MutationOutcome, WorkError> {
            unimplemented!("selection never mutates")
        }

        fn close(
            &self,
            _id: &BeadId,
            _reason: CloseReason,
            _operation: &OperationId,
            _expected: &WorkRevision,
        ) -> Result<MutationOutcome, WorkError> {
            unimplemented!("selection never mutates")
        }
    }

    /// Answers every advice solicitation with one scripted outcome,
    /// recording the revision it was asked against.
    struct ScriptedAdvice {
        outcome: AdviceOutcome,
        asked: RefCell<Vec<(WorkRevision, Vec<BeadId>)>>,
    }

    impl ScriptedAdvice {
        fn new(outcome: AdviceOutcome) -> Self {
            Self {
                outcome,
                asked: RefCell::new(Vec::new()),
            }
        }
    }

    impl WorkAdvicePort for ScriptedAdvice {
        fn advise(&self, revision: &WorkRevision, ready: &[BeadId]) -> AdviceOutcome {
            self.asked
                .borrow_mut()
                .push((revision.clone(), ready.to_vec()));
            self.outcome.clone()
        }
    }

    /// Scripts exactly one launch answer and records what was asked.
    struct ScriptedRuntime {
        answer: RefCell<Option<Result<LaunchAttempt, RuntimeError>>>,
        launches: RefCell<Vec<(LaunchSubject, LaunchCorrelation)>>,
        events: EventLog,
    }

    impl ScriptedRuntime {
        fn new(answer: Result<LaunchAttempt, RuntimeError>, events: EventLog) -> Self {
            Self {
                answer: RefCell::new(Some(answer)),
                launches: RefCell::new(Vec::new()),
                events,
            }
        }
    }

    impl RuntimePort for ScriptedRuntime {
        fn launch(
            &self,
            spec: &LaunchSpec,
            _secret: EphemeralLaunchSecret,
        ) -> Result<LaunchAttempt, RuntimeError> {
            self.events.borrow_mut().push("launch");
            self.launches
                .borrow_mut()
                .push((spec.subject.clone(), spec.correlation.clone()));
            self.answer
                .borrow_mut()
                .take()
                .expect("the sequence issues at most one launch")
        }

        fn recover_launch(
            &self,
            _: &LaunchSubject,
            _: &LaunchCorrelation,
            _: Timestamp,
        ) -> Result<Option<LaunchOutcome>, RuntimeError> {
            unimplemented!("recovery is a separate explicit act")
        }

        fn observe(
            &self,
            _: &RuntimeHandle,
            _: Timestamp,
        ) -> Result<LivenessObservation, RuntimeError> {
            unimplemented!()
        }

        fn wait(
            &self,
            _: &RuntimeHandle,
            _: LivenessKind,
            _: Timestamp,
        ) -> Result<LivenessObservation, RuntimeError> {
            unimplemented!()
        }

        fn read_view(
            &self,
            _: &RuntimeHandle,
            _: u32,
            _: Timestamp,
        ) -> Result<String, RuntimeError> {
            unimplemented!()
        }

        fn doorbell(
            &self,
            _: &RuntimeHandle,
            _: Timestamp,
        ) -> Result<DeliveryReport, RuntimeError> {
            unimplemented!()
        }

        fn prompt(
            &self,
            _: &RuntimeHandle,
            _: &str,
            _: Timestamp,
        ) -> Result<DeliveryReport, RuntimeError> {
            unimplemented!()
        }

        fn control(
            &self,
            _: &RuntimeHandle,
            _: ControlAction,
            _: Timestamp,
        ) -> Result<EffectReport, RuntimeError> {
            unimplemented!()
        }

        fn stop(
            &self,
            _: &RuntimeHandle,
            _: StopMode,
            _: Timestamp,
        ) -> Result<EffectReport, RuntimeError> {
            unimplemented!()
        }

        fn reassociate(
            &self,
            _: &RuntimeHandle,
            _: Timestamp,
        ) -> Result<RuntimeHandle, RuntimeError> {
            unimplemented!()
        }
    }

    fn worker_subject() -> LaunchSubject {
        LaunchSubject::WorkerAttempt {
            attempt: AttemptId::new("att-1").expect("valid attempt"),
            credential: CredentialId::new("cred-1").expect("valid credential"),
        }
    }

    fn launch_spec() -> LaunchSpec {
        LaunchSpec {
            subject: worker_subject(),
            correlation: LaunchCorrelation::new("abacus-att-1").expect("valid correlation"),
            agent_kind: "claude".to_owned(),
            executable: "claude".to_owned(),
            args: Vec::new(),
            working_directory: HostPath::new("/worktrees/att-1").expect("valid path"),
            environment: std::collections::BTreeMap::new(),
            envelope: EnvelopeSnapshot::new("do the assigned work".to_owned(), rev('c').0)
                .expect("bounded envelope"),
            startup_deadline: Timestamp(1_000),
            delivery_deadline: Timestamp(2_000),
        }
    }

    fn launch_secret() -> EphemeralLaunchSecret {
        EphemeralLaunchSecret::new("a".repeat(32), worker_subject()).expect("valid secret")
    }

    fn launched_outcome() -> LaunchOutcome {
        LaunchOutcome {
            handle: RuntimeHandle::new("arh1|abacus-workers-r1|p2|g1"),
            observation: LivenessObservation {
                observed_at: Timestamp(5),
                kind: LivenessKind::Starting,
            },
            startup_delivery: StartupDelivery::Submitted,
        }
    }

    fn worker_action(operation: &str) -> FencedAction {
        FencedAction {
            call: FencedCall {
                assignment: AssignmentId::new("asg-1").expect("valid assignment"),
                attempt: AttemptId::new("att-1").expect("valid attempt"),
                actor: worker_actor().actor,
                token: FencingToken(1),
                operation: op(operation),
            },
            responds_to: None,
        }
    }

    fn report_draft() -> SignalDraft {
        SignalDraft {
            id: SignalId::new("sig-report-1").expect("valid signal id"),
            sender: AuthoritySnapshot {
                actor: worker_actor(),
                capability: CapabilityId::new("state:report").expect("valid capability"),
                scope: ScopeExpr::Universal,
            },
            subject: SubjectRef::Attempt(AttemptId::new("att-1").expect("valid attempt")),
            body: SignalBody::Report {
                attempt: AttemptId::new("att-1").expect("valid attempt"),
                kind: ReportKind::Progress {
                    phase: SemanticPhase::Verifying,
                    summary: None,
                },
            },
        }
    }

    fn evidence_fixture() -> Evidence {
        let verification = verification();
        Evidence::new(
            verification.commands()[0].clone(),
            verification,
            0,
            VerificationOutcome::Pass,
            CommitId::new(&"d".repeat(40)).expect("valid commit"),
            WorkspaceDigest::new(&"1".repeat(64)).expect("valid digest"),
            WorkspaceDigest::new(&"2".repeat(64)).expect("valid digest"),
            None,
            FileDigestSet::default(),
            None,
        )
        .expect("coherent evidence")
    }

    fn handoff_record() -> HandoffRecord {
        HandoffRecord {
            id: HandoffId::new("hnd-1").expect("valid handoff id"),
            attempt: AttemptId::new("att-1").expect("valid attempt"),
            commit: CommitId::new(&"9".repeat(40)).expect("valid commit"),
            expected_base: CommitId::new(&"d".repeat(40)).expect("valid commit"),
            clean_tree: WorkspaceDigest::new(&"3".repeat(64)).expect("valid digest"),
            changed_paths: PathSet::new(vec![WorkPath::new("src/lib.rs").expect("valid path")])
                .expect("valid path set"),
            evidence_operations: OperationSet::new(vec![op("op-evidence-1")])
                .expect("valid operations"),
            attestation: rev('8').0,
        }
    }

    fn fenced_response(head: u64) -> FencedResponse {
        FencedResponse {
            applied: StateApplied::Applied,
            binding_directives: Vec::new(),
            head: Seq(head),
        }
    }

    /// The selection the `opening()` fixture is honestly bound to.
    fn matching_selection() -> ReadySelection {
        ReadySelection {
            revision: rev('e'),
            ready: vec![snapshot("ABACUS-usecase.1", 1)],
            order: vec![bead()],
            advice: AdviceDisposition::Followed,
        }
    }

    fn acceptance_decision() -> AcceptanceDecision {
        AcceptanceDecision {
            operation: op("op-accept"),
            assignment: AssignmentId::new("asg-1").expect("valid assignment"),
            authority: AuthoritySnapshot {
                actor: lead_actor(),
                capability: CapabilityId::new("state:accept").expect("valid capability"),
                scope: ScopeExpr::Universal,
            },
            handoff: HandoffId::new("hnd-1").expect("valid handoff"),
            reason: DecisionReason::new("verified and accepted").expect("valid reason"),
            resolves: None,
        }
    }

    #[derive(Default)]
    struct RecordingState {
        pending: RefCell<Vec<PendingApplication>>,
        attempts: RefCell<Vec<ApplicationAttempt>>,
        receipts: RefCell<Vec<ApplicationReceipt>>,
        decisions: RefCell<Vec<DecisionRecord>>,
        openings: RefCell<Vec<AssignmentOpening>>,
        /// Scripts an idempotent-replay opening: `AlreadyApplied`, and
        /// no pending projection is (re)minted — modelling a replay
        /// whose original projection already earned its receipt.
        open_replay: Cell<bool>,
        envelopes: RefCell<Vec<(OperationId, LaunchSubject, EnvelopeSnapshot)>>,
        binds: RefCell<Vec<(OperationId, LaunchSubject, RuntimeHandle)>>,
        fail_persist: Cell<bool>,
        fail_bind: Cell<bool>,
        events: EventLog,
        report_calls: RefCell<Vec<(FencedAction, SignalDraft)>>,
        report_answer: RefCell<Option<(ReportOutcome, FencedResponse)>>,
        evidence_calls: RefCell<Vec<(FencedAction, Evidence)>>,
        evidence_answer: RefCell<Option<(EvidenceOutcome, FencedResponse)>>,
        handoff_calls: RefCell<Vec<(FencedAction, HandoffRecord)>>,
        handoff_answer: RefCell<Option<(SubmissionOutcome, FencedResponse)>>,
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

        fn superseded_applications(&self) -> Result<Vec<SupersededApplication>, StateError> {
            unimplemented!("no scenario queries the superseded view")
        }

        fn open_assignment(&self, opening: &AssignmentOpening) -> Result<StateApplied, StateError> {
            self.openings.borrow_mut().push(opening.clone());
            if self.open_replay.get() {
                return Ok(StateApplied::AlreadyApplied);
            }
            // Mirrors the state contract: the opening mints its
            // mark-in-progress projection under the authorizing
            // operation, bound to the opening's bead revision.
            self.pending.borrow_mut().push(PendingApplication {
                operation: opening.authorizing.operation.clone(),
                assignment: opening.assignment.id.clone(),
                bead: opening.assignment.bead.clone(),
                projection: WorkProjection::MarkInProgress,
                committed_at: Seq(1),
                authorized_revision: Some(opening.bead_revision.clone()),
                receipt_candidate: None,
            });
            Ok(StateApplied::Applied)
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
            action: &FencedAction,
            draft: &SignalDraft,
        ) -> Result<(ReportOutcome, FencedResponse), StateError> {
            self.report_calls
                .borrow_mut()
                .push((action.clone(), draft.clone()));
            Ok(self
                .report_answer
                .borrow_mut()
                .take()
                .expect("this scenario scripts no report answer"))
        }
        fn fenced_evidence(
            &self,
            action: &FencedAction,
            evidence: &Evidence,
        ) -> Result<(EvidenceOutcome, FencedResponse), StateError> {
            self.evidence_calls
                .borrow_mut()
                .push((action.clone(), evidence.clone()));
            Ok(self
                .evidence_answer
                .borrow_mut()
                .take()
                .expect("this scenario scripts no evidence answer"))
        }
        fn fenced_submit_handoff(
            &self,
            action: &FencedAction,
            handoff: &HandoffRecord,
        ) -> Result<(SubmissionOutcome, FencedResponse), StateError> {
            self.handoff_calls
                .borrow_mut()
                .push((action.clone(), handoff.clone()));
            Ok(self
                .handoff_answer
                .borrow_mut()
                .take()
                .expect("this scenario scripts no handoff answer"))
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
            operation: &OperationId,
            subject: &LaunchSubject,
            envelope: &EnvelopeSnapshot,
        ) -> Result<StateApplied, StateError> {
            self.events.borrow_mut().push("persist_envelope");
            if self.fail_persist.get() {
                return Err(StateError::Unavailable);
            }
            self.envelopes.borrow_mut().push((
                operation.clone(),
                subject.clone(),
                envelope.clone(),
            ));
            Ok(StateApplied::Applied)
        }
        fn envelope(&self, _: &LaunchSubject) -> Result<EnvelopeSnapshot, StateError> {
            unimplemented!()
        }
        fn bind_runtime_handle(
            &self,
            operation: &OperationId,
            subject: &LaunchSubject,
            handle: &RuntimeHandle,
        ) -> Result<StateApplied, StateError> {
            self.events.borrow_mut().push("bind_runtime_handle");
            if self.fail_bind.get() {
                return Err(StateError::Unavailable);
            }
            self.binds
                .borrow_mut()
                .push((operation.clone(), subject.clone(), handle.clone()));
            Ok(StateApplied::Applied)
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
    fn a_found_present_effect_never_earns_a_receipt_even_when_facts_match() {
        let state = RecordingState::default();
        state.pending.borrow_mut().push(pending_close());
        let work = ScriptedWork::new(Ok(MutationOutcome::FoundBeforeSubmission {
            status: WorkStatus::Closed {
                observed_reason: ObservedCloseReason::AcceptedHandoff,
            },
            revision: rev('7'),
        }));

        // The observed facts match the intended projection exactly —
        // and it does not matter: nothing was submitted by this call,
        // br cannot attest whose effect this is, and only Applied is
        // receipt-eligible (ADR-0001 effect-provenance amendment).
        assert_eq!(
            project_pending(&state, &work, &pending_close(), &op("app-2")).expect("runs"),
            ProjectionOutcome::Unresolved {
                attempt: op("app-2"),
                reason: ProjectionUnresolved::ConflictingEffect {
                    status: WorkStatus::Closed {
                        observed_reason: ObservedCloseReason::AcceptedHandoff,
                    },
                    revision: rev('7'),
                }
            }
        );
        assert!(state.receipts.borrow().is_empty());
        assert_eq!(state.pending.borrow().len(), 1);
    }

    #[test]
    fn an_observed_after_ambiguous_submission_is_possibly_applied_not_confirmed() {
        let pending = PendingApplication {
            projection: WorkProjection::MarkInProgress,
            ..pending_close()
        };
        let state = RecordingState::default();
        state.pending.borrow_mut().push(pending.clone());
        let work = ScriptedWork::new(Ok(MutationOutcome::ObservedAfterAmbiguousSubmission {
            status: WorkStatus::InProgress,
            revision: rev('7'),
        }));

        assert_eq!(
            project_pending(&state, &work, &pending, &op("app-11")).expect("runs"),
            ProjectionOutcome::Unresolved {
                attempt: op("app-11"),
                reason: ProjectionUnresolved::PossiblyApplied {
                    status: WorkStatus::InProgress,
                    revision: rev('7'),
                }
            },
            "our submission may have landed - possibly applied is not confirmation"
        );
        assert_eq!(state.attempts.borrow().len(), 1);
        assert_eq!(
            state.attempts.borrow()[0].outcome,
            ApplicationOutcome::ObservedAfterAmbiguous {
                status: WorkStatus::InProgress,
                revision: rev('7'),
            },
            "the Ledger's attempt history states what this attempt proved"
        );
        assert!(
            state.receipts.borrow().is_empty(),
            "an ambiguous observation must never manufacture a receipt"
        );
        assert_eq!(state.pending.borrow().len(), 1);
    }

    #[test]
    fn a_receipt_candidate_recovers_without_provider_mutation() {
        let pending = PendingApplication {
            receipt_candidate: Some(ReceiptCandidate {
                attempt: op("app-orig"),
                after: rev('9'),
            }),
            ..pending_close()
        };
        let state = RecordingState::default();
        state.pending.borrow_mut().push(pending.clone());
        let work = ScriptedWork::new(Ok(MutationOutcome::Applied {
            before: rev('e'),
            after: rev('9'),
            summary: "must not be issued".to_owned(),
        }));

        let outcome =
            project_pending(&state, &work, &pending, &op("app-fresh")).expect("recovery runs");
        assert_eq!(
            outcome,
            ProjectionOutcome::ReceiptRecovered {
                attempt: op("app-orig"),
                after: rev('9'),
            }
        );

        // Mutation-free, crediting the ORIGINAL applied attempt: no
        // provider call, no fresh attempt record, and the receipt names
        // the candidate's attempt — not the caller-supplied fresh one.
        assert!(work.closes.borrow().is_empty());
        assert!(work.marks.borrow().is_empty());
        assert!(state.attempts.borrow().is_empty());
        assert_eq!(state.receipts.borrow().len(), 1);
        assert_eq!(state.receipts.borrow()[0].attempt, op("app-orig"));
        assert_eq!(state.receipts.borrow()[0].target, op("op-accept"));
        assert!(state.pending.borrow().is_empty());
    }

    #[test]
    fn reconciliation_recovers_candidates_without_consuming_attempt_identities() {
        let state = RecordingState::default();
        state.pending.borrow_mut().push(PendingApplication {
            receipt_candidate: Some(ReceiptCandidate {
                attempt: op("app-orig"),
                after: rev('8'),
            }),
            ..pending_close()
        });
        state.pending.borrow_mut().push(PendingApplication {
            operation: op("op-accept-2"),
            ..pending_close()
        });
        let work = ScriptedWork::new(Ok(MutationOutcome::Applied {
            before: rev('e'),
            after: rev('9'),
            summary: "closed".to_owned(),
        }));

        // ONE attempt identity for two pending items: the candidate
        // consumes none, so the single identity covers the second.
        let pass = reconcile_pending(&state, &work, &[op("app-1")]).expect("reconciles");
        assert_eq!(pass.reconciled.len(), 2);
        assert_eq!(
            pass.reconciled[0],
            (
                op("op-accept"),
                ProjectionOutcome::ReceiptRecovered {
                    attempt: op("app-orig"),
                    after: rev('8'),
                }
            )
        );
        assert_eq!(
            pass.reconciled[1],
            (
                op("op-accept-2"),
                ProjectionOutcome::Projected { after: rev('9') }
            )
        );
        assert!(pass.unattempted.is_empty());
        assert!(state.pending.borrow().is_empty());
    }

    #[test]
    fn an_observed_after_ambiguous_submission_with_foreign_facts_conflicts() {
        let state = RecordingState::default();
        state.pending.borrow_mut().push(pending_close());
        // Composition does not trust the facade's satisfaction check:
        // facts that do not satisfy the intended close are a conflict
        // regardless of submission provenance.
        let work = ScriptedWork::new(Ok(MutationOutcome::ObservedAfterAmbiguousSubmission {
            status: WorkStatus::Closed {
                observed_reason: ObservedCloseReason::CancelledObsolete,
            },
            revision: rev('7'),
        }));

        assert!(matches!(
            project_pending(&state, &work, &pending_close(), &op("app-12")).expect("runs"),
            ProjectionOutcome::Unresolved {
                reason: ProjectionUnresolved::ConflictingEffect { .. },
                ..
            }
        ),);
        assert!(state.receipts.borrow().is_empty());
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
        assert_eq!(results.reconciled.len(), 1);
        assert_eq!(results.reconciled[0].0, op("op-accept"));
        assert_eq!(
            results.reconciled[0].1,
            ProjectionOutcome::Projected { after: rev('9') }
        );
        assert!(results.unattempted.is_empty());
        assert!(state.pending.borrow().is_empty());

        // Nothing is left pending, so a further reconciliation is a
        // no-op rather than a second mutation.
        let again = reconcile_pending(&state, &work, &[op("app-retry-2")]).expect("reconciles");
        assert!(again.reconciled.is_empty());
        assert!(again.unattempted.is_empty());
    }

    #[test]
    fn a_close_authorized_without_a_revision_uses_the_observed_revision() {
        // The production Accept path mints its close projection with
        // authorized_revision: None — the decision does not bind a bead
        // revision. The expected revision must come from a fresh
        // observation, never from a placeholder no real graph can match.
        let mut pending = pending_close();
        pending.authorized_revision = None;
        let state = RecordingState::default();
        state.pending.borrow_mut().push(pending.clone());
        let work = ScriptedWork::new(Ok(MutationOutcome::Applied {
            before: rev('5'),
            after: rev('9'),
            summary: "closed".to_owned(),
        }))
        .with_view(Ok(BeadStatusView {
            snapshot: snapshot("ABACUS-usecase.1", 1),
            status: WorkStatus::InProgress,
            revision: rev('5'),
        }));

        let outcome =
            project_pending(&state, &work, &pending, &op("app-5")).expect("projection runs");
        assert_eq!(outcome, ProjectionOutcome::Projected { after: rev('9') });

        let closes = work.closes.borrow();
        assert_eq!(closes.len(), 1);
        assert_eq!(
            closes[0].3,
            rev('5'),
            "the observed revision is the precondition, not a placeholder"
        );
        assert_eq!(state.receipts.borrow().len(), 1);
    }

    #[test]
    fn a_failed_observation_before_an_unauthorized_revision_close_stays_pending() {
        let mut pending = pending_close();
        pending.authorized_revision = None;
        let state = RecordingState::default();
        state.pending.borrow_mut().push(pending.clone());
        // Inspection fails before any mutation is issued: a definite
        // non-effect, recorded and left pending.
        let work = ScriptedWork::new(Ok(MutationOutcome::Applied {
            before: rev('5'),
            after: rev('9'),
            summary: "must not be issued".to_owned(),
        }))
        .with_view(Err(WorkError::ProviderUnavailable));

        let outcome =
            project_pending(&state, &work, &pending, &op("app-6")).expect("projection runs");
        assert_eq!(
            outcome,
            ProjectionOutcome::Unresolved {
                attempt: op("app-6"),
                reason: ProjectionUnresolved::Failed(WorkError::ProviderUnavailable)
            }
        );
        assert!(
            work.closes.borrow().is_empty(),
            "no mutation may be issued without a real precondition"
        );
        assert_eq!(
            state.attempts.borrow()[0].outcome,
            ApplicationOutcome::Failed {
                error: WorkError::ProviderUnavailable
            }
        );
        assert!(state.receipts.borrow().is_empty());
        assert_eq!(state.pending.borrow().len(), 1);
    }

    #[test]
    fn a_conflicting_already_present_effect_earns_no_receipt() {
        // The bead is closed — but under the WRONG reason. The facade
        // reports observed facts precisely so core can refuse to adopt
        // a foreign mutation as this operation's effect.
        let state = RecordingState::default();
        state.pending.borrow_mut().push(pending_close());
        let work = ScriptedWork::new(Ok(MutationOutcome::FoundBeforeSubmission {
            status: WorkStatus::Closed {
                observed_reason: ObservedCloseReason::CancelledObsolete,
            },
            revision: rev('7'),
        }));

        let outcome = project_pending(&state, &work, &pending_close(), &op("app-7"))
            .expect("projection runs");
        assert_eq!(
            outcome,
            ProjectionOutcome::Unresolved {
                attempt: op("app-7"),
                reason: ProjectionUnresolved::ConflictingEffect {
                    status: WorkStatus::Closed {
                        observed_reason: ObservedCloseReason::CancelledObsolete,
                    },
                    revision: rev('7'),
                }
            }
        );
        // The attempt records the observed facts WITH their provenance;
        // the receipt — which would attest THIS effect — must not exist.
        assert_eq!(state.attempts.borrow().len(), 1);
        assert_eq!(
            state.attempts.borrow()[0].outcome,
            ApplicationOutcome::FoundPresent {
                status: WorkStatus::Closed {
                    observed_reason: ObservedCloseReason::CancelledObsolete,
                },
                revision: rev('7'),
            }
        );
        assert!(state.receipts.borrow().is_empty());
        assert_eq!(
            state.pending.borrow().len(),
            1,
            "a conflicting effect stays pending: resolution is a decision"
        );
    }

    #[test]
    fn an_unrecognized_close_reason_never_satisfies_a_close_projection() {
        let state = RecordingState::default();
        state.pending.borrow_mut().push(pending_close());
        let work = ScriptedWork::new(Ok(MutationOutcome::FoundBeforeSubmission {
            status: WorkStatus::Closed {
                observed_reason: ObservedCloseReason::UnrecognizedProviderReason,
            },
            revision: rev('7'),
        }));

        let outcome = project_pending(&state, &work, &pending_close(), &op("app-8"))
            .expect("projection runs");
        assert!(
            matches!(
                outcome,
                ProjectionOutcome::Unresolved {
                    reason: ProjectionUnresolved::ConflictingEffect { .. },
                    ..
                }
            ),
            "an out-of-band close is not this operation's effect"
        );
        assert!(state.receipts.borrow().is_empty());
    }

    #[test]
    fn a_mark_projection_never_adopts_a_closed_beads_effect() {
        let pending = PendingApplication {
            projection: WorkProjection::MarkInProgress,
            ..pending_close()
        };
        let state = RecordingState::default();
        state.pending.borrow_mut().push(pending.clone());
        let work = ScriptedWork::new(Ok(MutationOutcome::FoundBeforeSubmission {
            status: WorkStatus::Closed {
                observed_reason: ObservedCloseReason::AcceptedHandoff,
            },
            revision: rev('7'),
        }));

        let outcome =
            project_pending(&state, &work, &pending, &op("app-9")).expect("projection runs");
        assert!(
            matches!(
                outcome,
                ProjectionOutcome::Unresolved {
                    reason: ProjectionUnresolved::ConflictingEffect { .. },
                    ..
                }
            ),
            "closed is terminal: it can never satisfy mark-in-progress"
        );
        assert!(state.receipts.borrow().is_empty());
    }

    #[test]
    fn reconciliation_names_the_unattempted_remainder() {
        let state = RecordingState::default();
        state.pending.borrow_mut().push(pending_close());
        state.pending.borrow_mut().push(PendingApplication {
            operation: op("op-accept-2"),
            ..pending_close()
        });
        // One attempt identity for two pending projections: the second
        // must be NAMED as uncovered, never silently dropped.
        let work = ScriptedWork::new(Ok(MutationOutcome::Applied {
            before: rev('e'),
            after: rev('9'),
            summary: "closed".to_owned(),
        }));

        let pass = reconcile_pending(&state, &work, &[op("app-10")]).expect("reconciles");
        assert_eq!(pass.reconciled.len(), 1);
        assert_eq!(pass.reconciled[0].0, op("op-accept"));
        assert_eq!(
            pass.unattempted,
            vec![op("op-accept-2")],
            "partial passes are legitimate; silent truncation is not"
        );
    }

    #[test]
    fn selection_accepts_only_advice_bound_to_the_current_revision() {
        let work = ReadyWork {
            revision: rev('1'),
            ready: vec![snapshot("ABACUS-b.1", 0), snapshot("ABACUS-b.2", 2)],
        };
        // The advisor inverts the fallback order and is bound to the
        // revision the ready set was read at — so its order governs.
        let advice = ScriptedAdvice::new(AdviceOutcome::Advice {
            order: vec![
                BeadId::new("ABACUS-b.2").expect("valid"),
                BeadId::new("ABACUS-b.1").expect("valid"),
            ],
            bound_to: rev('1'),
        });

        let selection = select_ready(&work, &advice).expect("selection runs");
        assert_eq!(selection.revision, rev('1'));
        assert_eq!(selection.ready.len(), 2);
        assert_eq!(
            selection.order,
            vec![
                BeadId::new("ABACUS-b.2").expect("valid"),
                BeadId::new("ABACUS-b.1").expect("valid"),
            ]
        );
        assert_eq!(selection.advice, AdviceDisposition::Followed);

        // Advice was solicited against exactly the bracketed revision
        // and eligible set.
        let asked = advice.asked.borrow();
        assert_eq!(asked.len(), 1);
        assert_eq!(asked[0].0, rev('1'));
        assert_eq!(asked[0].1.len(), 2);
    }

    #[test]
    fn selection_falls_back_deterministically_on_stale_advice() {
        let work = ReadyWork {
            revision: rev('1'),
            ready: vec![snapshot("ABACUS-b.2", 2), snapshot("ABACUS-b.1", 0)],
        };
        // Same permutation, but bound to a revision the graph has
        // moved past: the gate must discard it.
        let advice = ScriptedAdvice::new(AdviceOutcome::Advice {
            order: vec![
                BeadId::new("ABACUS-b.2").expect("valid"),
                BeadId::new("ABACUS-b.1").expect("valid"),
            ],
            bound_to: rev('2'),
        });

        let selection = select_ready(&work, &advice).expect("selection runs");
        assert_eq!(
            selection.order,
            vec![
                BeadId::new("ABACUS-b.1").expect("valid"),
                BeadId::new("ABACUS-b.2").expect("valid"),
            ],
            "stale advice yields the priority-then-id fallback"
        );
        assert_eq!(
            selection.advice,
            AdviceDisposition::Rejected {
                reason: AdviceRejection::StaleBinding
            },
            "the rejection is noted in the selection, never erased"
        );
    }

    #[test]
    fn selection_notes_advisor_degradation_in_its_output() {
        let work = ReadyWork {
            revision: rev('1'),
            ready: vec![snapshot("ABACUS-b.2", 2), snapshot("ABACUS-b.1", 0)],
        };
        let advice = ScriptedAdvice::new(AdviceOutcome::Degraded {
            reason: AdviceDegradation::Unavailable,
        });

        let selection = select_ready(&work, &advice).expect("selection runs");
        assert_eq!(
            selection.order,
            vec![
                BeadId::new("ABACUS-b.1").expect("valid"),
                BeadId::new("ABACUS-b.2").expect("valid"),
            ],
            "an unavailable advisor never blocks: deterministic fallback"
        );
        assert_eq!(
            selection.advice,
            AdviceDisposition::Degraded {
                reason: AdviceDegradation::Unavailable
            }
        );
    }

    #[test]
    fn selection_rejects_advice_that_does_not_cover_the_eligible_set() {
        let work = ReadyWork {
            revision: rev('1'),
            ready: vec![snapshot("ABACUS-b.2", 2), snapshot("ABACUS-b.1", 0)],
        };
        // Bound to the right revision, but naming only one of two
        // eligible beads: not a permutation, so the gate must reject.
        let advice = ScriptedAdvice::new(AdviceOutcome::Advice {
            order: vec![BeadId::new("ABACUS-b.2").expect("valid")],
            bound_to: rev('1'),
        });

        let selection = select_ready(&work, &advice).expect("selection runs");
        assert_eq!(
            selection.order,
            vec![
                BeadId::new("ABACUS-b.1").expect("valid"),
                BeadId::new("ABACUS-b.2").expect("valid"),
            ]
        );
        assert_eq!(
            selection.advice,
            AdviceDisposition::Rejected {
                reason: AdviceRejection::NotCovering
            }
        );
    }

    #[test]
    fn an_opened_assignment_projects_mark_in_progress_under_its_operation() {
        let state = RecordingState::default();
        let work = ScriptedWork::new(Ok(MutationOutcome::Applied {
            before: rev('e'),
            after: rev('9'),
            summary: "in progress".to_owned(),
        }));

        let outcome = assign_ready(
            &state,
            &work,
            &matching_selection(),
            &opening(),
            &op("app-1"),
        )
        .expect("assignment runs");
        assert_eq!(
            outcome,
            AssignmentOutcome::Opened {
                opening: StateApplied::Applied,
                projection: Some(ProjectionOutcome::Projected { after: rev('9') }),
            }
        );

        // The opening bundle was committed before any provider
        // mutation, and the mark-in-progress rode the SAME authorizing
        // operation under the opening's bead revision.
        assert_eq!(state.openings.borrow().len(), 1);
        let marks = work.marks.borrow();
        assert_eq!(marks.len(), 1);
        assert_eq!(marks[0].0, bead());
        assert_eq!(marks[0].1, op("op-assign"));
        assert_eq!(marks[0].2, rev('e'));

        assert_eq!(state.attempts.borrow().len(), 1);
        assert_eq!(state.receipts.borrow().len(), 1);
        assert!(state.pending.borrow().is_empty());
    }

    #[test]
    fn an_opening_for_a_bead_the_graph_never_offered_is_refused() {
        let state = RecordingState::default();
        let work = ScriptedWork::new(Ok(MutationOutcome::Applied {
            before: rev('e'),
            after: rev('9'),
            summary: "must not be issued".to_owned(),
        }));
        // The selection offers a DIFFERENT bead than the opening names.
        let selection = ReadySelection {
            ready: vec![snapshot("ABACUS-other.1", 0)],
            order: vec![BeadId::new("ABACUS-other.1").expect("valid")],
            ..matching_selection()
        };

        let outcome = assign_ready(&state, &work, &selection, &opening(), &op("app-1"))
            .expect("refusal is in-band");
        assert_eq!(
            outcome,
            AssignmentOutcome::Refused {
                refusal: AssignRefusal::BeadNotReady { bead: bead() }
            }
        );
        assert!(
            state.openings.borrow().is_empty(),
            "a refused opening must never reach Scribe"
        );
        assert!(work.marks.borrow().is_empty());
        assert!(state.attempts.borrow().is_empty());
    }

    #[test]
    fn an_opening_authorized_at_a_different_revision_is_refused() {
        let state = RecordingState::default();
        let work = ScriptedWork::new(Ok(MutationOutcome::Applied {
            before: rev('e'),
            after: rev('9'),
            summary: "must not be issued".to_owned(),
        }));
        let selection = ReadySelection {
            revision: rev('1'),
            ..matching_selection()
        };

        let outcome = assign_ready(&state, &work, &selection, &opening(), &op("app-1"))
            .expect("refusal is in-band");
        assert_eq!(
            outcome,
            AssignmentOutcome::Refused {
                refusal: AssignRefusal::RevisionMismatch {
                    authorized: rev('e'),
                    selection: rev('1'),
                }
            }
        );
        assert!(state.openings.borrow().is_empty());
    }

    #[test]
    fn an_opening_with_a_forged_content_hash_is_refused() {
        let state = RecordingState::default();
        let work = ScriptedWork::new(Ok(MutationOutcome::Applied {
            before: rev('e'),
            after: rev('9'),
            summary: "must not be issued".to_owned(),
        }));
        let mut forged = opening();
        forged.assignment.bead_content_hash = rev('f').0;

        let outcome = assign_ready(&state, &work, &matching_selection(), &forged, &op("app-1"))
            .expect("refusal is in-band");
        assert_eq!(
            outcome,
            AssignmentOutcome::Refused {
                refusal: AssignRefusal::ContentHashMismatch { bead: bead() }
            },
            "the opening must snapshot the EXACT content the graph offered"
        );
        assert!(state.openings.borrow().is_empty());
    }

    #[test]
    fn an_opening_with_a_substituted_scope_map_is_refused() {
        let state = RecordingState::default();
        let work = ScriptedWork::new(Ok(MutationOutcome::Applied {
            before: rev('e'),
            after: rev('9'),
            summary: "must not be issued".to_owned(),
        }));
        let mut forged = opening();
        forged.assignment.scope_map = ScopeMap::new(vec![(
            ScopeKey::new("area").expect("valid key"),
            ScopeValue::new("auth").expect("valid value"),
        )])
        .expect("valid scope map");

        let outcome = assign_ready(&state, &work, &matching_selection(), &forged, &op("app-1"))
            .expect("refusal is in-band");
        assert_eq!(
            outcome,
            AssignmentOutcome::Refused {
                refusal: AssignRefusal::ScopeMapMismatch { bead: bead() }
            }
        );
        assert!(state.openings.borrow().is_empty());
    }

    #[test]
    fn an_ambiguous_opening_projection_stays_pending() {
        let state = RecordingState::default();
        let work = ScriptedWork::new(Err(WorkError::AmbiguousOutcome));

        let outcome = assign_ready(
            &state,
            &work,
            &matching_selection(),
            &opening(),
            &op("app-2"),
        )
        .expect("assignment runs");
        assert_eq!(
            outcome,
            AssignmentOutcome::Opened {
                opening: StateApplied::Applied,
                projection: Some(ProjectionOutcome::Unresolved {
                    attempt: op("app-2"),
                    reason: ProjectionUnresolved::Ambiguous
                }),
            }
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
    fn a_launch_persists_the_envelope_before_any_provider_interaction() {
        let state = RecordingState::default();
        let runtime = ScriptedRuntime::new(
            Ok(LaunchAttempt::Launched(launched_outcome())),
            Rc::clone(&state.events),
        );

        let outcome = launch_subject(
            &state,
            &runtime,
            &launch_spec(),
            launch_secret(),
            &op("op-persist"),
            &op("op-bind"),
        )
        .expect("sequence runs");
        assert_eq!(
            outcome,
            LaunchSequenceOutcome::Launched {
                launched: launched_outcome(),
                bound: StateApplied::Applied,
            }
        );

        // The ordering IS the contract: durable Envelope, then launch,
        // then durable association of the returned handle.
        assert_eq!(
            *state.events.borrow(),
            vec!["persist_envelope", "launch", "bind_runtime_handle"]
        );

        let envelopes = state.envelopes.borrow();
        assert_eq!(envelopes.len(), 1);
        assert_eq!(envelopes[0].0, op("op-persist"));
        assert_eq!(envelopes[0].1, worker_subject());
        assert_eq!(envelopes[0].2.content(), "do the assigned work");

        let binds = state.binds.borrow();
        assert_eq!(binds.len(), 1);
        assert_eq!(binds[0].0, op("op-bind"));
        assert_eq!(binds[0].1, worker_subject());
        assert_eq!(binds[0].2, launched_outcome().handle);
    }

    #[test]
    fn a_failed_envelope_persist_reaches_no_provider() {
        let state = RecordingState::default();
        state.fail_persist.set(true);
        let runtime = ScriptedRuntime::new(
            Ok(LaunchAttempt::Launched(launched_outcome())),
            Rc::clone(&state.events),
        );

        let result = launch_subject(
            &state,
            &runtime,
            &launch_spec(),
            launch_secret(),
            &op("op-persist"),
            &op("op-bind"),
        );
        assert_eq!(result, Err(StateError::Unavailable));
        assert!(
            runtime.launches.borrow().is_empty(),
            "nothing may reach a live session that is not already durable"
        );
        assert_eq!(*state.events.borrow(), vec!["persist_envelope"]);
    }

    #[test]
    fn an_ambiguous_launch_binds_nothing_and_echoes_recovery_keys() {
        let state = RecordingState::default();
        let runtime = ScriptedRuntime::new(
            Ok(LaunchAttempt::Ambiguous {
                subject: worker_subject(),
                correlation: LaunchCorrelation::new("abacus-att-1").expect("valid"),
            }),
            Rc::clone(&state.events),
        );

        let outcome = launch_subject(
            &state,
            &runtime,
            &launch_spec(),
            launch_secret(),
            &op("op-persist"),
            &op("op-bind"),
        )
        .expect("sequence runs");
        assert_eq!(
            outcome,
            LaunchSequenceOutcome::Ambiguous {
                subject: worker_subject(),
                correlation: LaunchCorrelation::new("abacus-att-1").expect("valid"),
            }
        );
        assert!(
            state.binds.borrow().is_empty(),
            "no handle is known, so nothing may be associated"
        );
    }

    #[test]
    fn a_definite_launch_failure_is_a_typed_outcome() {
        let state = RecordingState::default();
        let runtime =
            ScriptedRuntime::new(Err(RuntimeError::NotPermitted), Rc::clone(&state.events));

        let outcome = launch_subject(
            &state,
            &runtime,
            &launch_spec(),
            launch_secret(),
            &op("op-persist"),
            &op("op-bind"),
        )
        .expect("sequence runs");
        assert_eq!(
            outcome,
            LaunchSequenceOutcome::NotLaunched {
                error: RuntimeError::NotPermitted
            }
        );
        assert!(state.binds.borrow().is_empty());
        assert_eq!(
            state.envelopes.borrow().len(),
            1,
            "the persisted Envelope remains for a future launch"
        );
    }

    #[test]
    fn a_bind_failure_surfaces_the_live_handle() {
        let state = RecordingState::default();
        state.fail_bind.set(true);
        let runtime = ScriptedRuntime::new(
            Ok(LaunchAttempt::Launched(launched_outcome())),
            Rc::clone(&state.events),
        );

        let outcome = launch_subject(
            &state,
            &runtime,
            &launch_spec(),
            launch_secret(),
            &op("op-persist"),
            &op("op-bind"),
        )
        .expect("sequence runs");
        assert_eq!(
            outcome,
            LaunchSequenceOutcome::LaunchedUnbound {
                launched: launched_outcome(),
                bind_error: StateError::Unavailable,
            },
            "a live session's handle must never be swallowed by a failed write"
        );
    }

    #[test]
    fn a_worker_report_passes_through_with_its_inband_refusal_intact() {
        let state = RecordingState::default();
        // The gate's in-band Abort refusal is a domain outcome, not an
        // error: it must reach the worker exactly as the seam returned
        // it, response envelope included.
        *state.report_answer.borrow_mut() = Some((
            ReportOutcome::Refused {
                reason: DirectiveGateRefusal::AbortInForce,
            },
            fenced_response(7),
        ));

        let (outcome, response) =
            record_report(&state, &worker_action("op-report-1"), &report_draft())
                .expect("pass-through runs");
        assert_eq!(
            outcome,
            ReportOutcome::Refused {
                reason: DirectiveGateRefusal::AbortInForce
            }
        );
        assert_eq!(response, fenced_response(7));

        let calls = state.report_calls.borrow();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].0, worker_action("op-report-1"));
        assert_eq!(calls[0].1, report_draft());
    }

    #[test]
    fn worker_evidence_passes_through_unaltered() {
        let state = RecordingState::default();
        *state.evidence_answer.borrow_mut() = Some((EvidenceOutcome::Recorded, fenced_response(9)));

        let (outcome, response) =
            record_evidence(&state, &worker_action("op-evidence-1"), &evidence_fixture())
                .expect("pass-through runs");
        assert_eq!(outcome, EvidenceOutcome::Recorded);
        assert_eq!(response, fenced_response(9));

        let calls = state.evidence_calls.borrow();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].0, worker_action("op-evidence-1"));
        assert_eq!(calls[0].1, evidence_fixture());
    }

    #[test]
    fn a_handoff_submission_passes_through_unaltered() {
        let state = RecordingState::default();
        *state.handoff_answer.borrow_mut() = Some((
            SubmissionOutcome::Recorded {
                handoff: HandoffId::new("hnd-1").expect("valid"),
            },
            fenced_response(11),
        ));

        let (outcome, response) =
            submit_handoff(&state, &worker_action("op-handoff-1"), &handoff_record())
                .expect("pass-through runs");
        assert_eq!(
            outcome,
            SubmissionOutcome::Recorded {
                handoff: HandoffId::new("hnd-1").expect("valid")
            }
        );
        assert_eq!(response, fenced_response(11));

        let calls = state.handoff_calls.borrow();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].0, worker_action("op-handoff-1"));
        assert_eq!(calls[0].1, handoff_record());
    }

    #[test]
    fn a_replayed_opening_with_no_pending_projection_mutates_nothing() {
        let state = RecordingState::default();
        state.open_replay.set(true);
        let work = ScriptedWork::new(Ok(MutationOutcome::Applied {
            before: rev('e'),
            after: rev('9'),
            summary: "must not be issued".to_owned(),
        }));

        let outcome = assign_ready(
            &state,
            &work,
            &matching_selection(),
            &opening(),
            &op("app-3"),
        )
        .expect("assignment runs");
        assert_eq!(
            outcome,
            AssignmentOutcome::Opened {
                opening: StateApplied::AlreadyApplied,
                projection: None,
            },
            "no pending projection remained, so there is nothing to project"
        );
        assert!(work.marks.borrow().is_empty());
        assert!(work.closes.borrow().is_empty());
        assert!(state.attempts.borrow().is_empty());
    }

    #[test]
    fn the_acceptance_saga_commits_an_accept_decision_and_projects_it() {
        let state = RecordingState::default();
        state.pending.borrow_mut().push(pending_close());
        let work = ScriptedWork::new(Ok(MutationOutcome::Applied {
            before: rev('e'),
            after: rev('9'),
            summary: "closed".to_owned(),
        }));

        let outcome =
            accept_handoff(&state, &work, &acceptance_decision(), &op("app-1")).expect("saga runs");
        assert_eq!(outcome.decision, StateApplied::Applied);
        assert_eq!(
            outcome.projection,
            Some(ProjectionOutcome::Projected { after: rev('9') })
        );

        // The recorded decision is structurally an Accept binding the
        // decided Handoff — no other kind is expressible here.
        let decisions = state.decisions.borrow();
        assert_eq!(decisions.len(), 1);
        assert_eq!(decisions[0].operation, op("op-accept"));
        assert_eq!(
            decisions[0].kind,
            DecisionKind::Accept {
                handoff: HandoffId::new("hnd-1").expect("valid"),
                reason: DecisionReason::new("verified and accepted").expect("valid"),
            }
        );
    }

    #[test]
    fn redriving_by_identity_uses_the_ledgers_own_projection_facts() {
        let state = RecordingState::default();
        state.pending.borrow_mut().push(pending_close());
        let work = ScriptedWork::new(Ok(MutationOutcome::Applied {
            before: rev('e'),
            after: rev('9'),
            summary: "closed".to_owned(),
        }));

        let outcome = redrive_pending(&state, &work, &op("op-accept"), &op("app-redrive"))
            .expect("redrive runs");
        assert_eq!(
            outcome,
            RedriveOutcome::Driven(ProjectionOutcome::Projected { after: rev('9') })
        );

        // The mutation drove the LEDGER's bead under the LEDGER's
        // authorizing operation — the caller supplied no projection
        // facts that could be substituted.
        let closes = work.closes.borrow();
        assert_eq!(closes.len(), 1);
        assert_eq!(closes[0].0, bead());
        assert_eq!(closes[0].2, op("op-accept"));
    }

    #[test]
    fn redriving_an_unknown_or_receipted_identity_is_not_pending() {
        let state = RecordingState::default();
        let work = ScriptedWork::new(Ok(MutationOutcome::Applied {
            before: rev('e'),
            after: rev('9'),
            summary: "must not be issued".to_owned(),
        }));

        let outcome = redrive_pending(&state, &work, &op("op-unknown"), &op("app-redrive"))
            .expect("redrive runs");
        assert_eq!(outcome, RedriveOutcome::NotPending);
        assert!(work.closes.borrow().is_empty());
        assert!(work.marks.borrow().is_empty());
        assert!(
            state.attempts.borrow().is_empty(),
            "nothing pending means nothing recorded"
        );
    }
}
