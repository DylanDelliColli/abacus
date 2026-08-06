//! Provider-neutral ports consumed by core use cases.
//!
//! The seam every adapter implements. Not a plugin framework: a port
//! exists only for behavior a core use case consumes, with a production
//! adapter and a hermetic fake (core contract). No provider vocabulary
//! appears here; changes are C1+ and require cross-review. Clocks and
//! identifier generation are ports too (I13; core invariant 9).
//!
//! Transport-level protocol facts (version negotiation, repository
//! identity, request IDs) belong to the `abacus-state` client/server
//! implementation, not this in-process seam; the workflow-visible
//! rules — Scribe-allocated ordering, fenced envelopes, idempotency,
//! and the typed audit-lineage index — are encoded here in the types.

use std::collections::BTreeMap;

use crate::assignment::{AttemptPolicy, DecisionActor};
use crate::content::{CommitId, ContentHash, WorkspaceDigest};
use crate::edit_scope::{EditScope, WorkPath};
use crate::evidence::{AcceptancePolicy, Evidence, PairRefusal, PathSet};
use crate::id::{
    ActorId, AssignmentId, AttemptId, BeadId, HandoffId, OperationId, ProfileName, SignalId,
};
use crate::lease::{FencingToken, Lease, Timestamp};
use crate::lifecycle::{AssignmentState, AttemptState};
use crate::profile::ProfileActivation;
use crate::scope::ScopeMap;
use crate::signal::{
    AuthoritySnapshot, DirectiveGateRefusal, ReportDraft, Seq, Signal, SignalBody, SignalDraft,
    SubjectRef,
};

// ---------------------------------------------------------------------------
// Work graph
// ---------------------------------------------------------------------------

/// Graph revision observed at read time; reads and decision-gated
/// mutations bracket themselves with it.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct WorkRevision(pub ContentHash);

/// Provider-neutral bounded priority (0 = highest urgency, 4 = lowest).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Priority(u8);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PriorityOutOfRange;

impl Priority {
    pub fn new(value: u8) -> Result<Self, PriorityOutOfRange> {
        if value <= 4 {
            Ok(Self(value))
        } else {
            Err(PriorityOutOfRange)
        }
    }

    pub fn value(self) -> u8 {
        self.0
    }
}

/// Normalized bead observation crossing the work seam (ADR-0002 §1:
/// raw provider labels never cross; the facade normalizes first).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BeadSnapshot {
    pub id: BeadId,
    /// Covers the bead contract including raw declared-key scope
    /// labels; Acceptance rechecks it.
    pub content_hash: ContentHash,
    pub scope_map: ScopeMap,
    pub priority: Priority,
}

/// Closed observed close reasons (I9: no raw provider text crosses).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ObservedCloseReason {
    AcceptedHandoff,
    CancelledObsolete,
    UnrecognizedProviderReason,
}

/// Normalized work status observed on a bead.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum WorkStatus {
    Open,
    InProgress,
    Closed {
        observed_reason: ObservedCloseReason,
    },
}

/// Whether a bead is currently offered for execution, as an axis
/// ORTHOGONAL to the open/in-progress/closed lifecycle.
///
/// Deferral is a scheduling fact, not a lifecycle state: a parked bead
/// is still open work. Collapsing it into `Open` would erase an
/// operator's deliberate decision to park it, and a later projection
/// could then flip it to in-progress — an implicit undefer nobody
/// authorized (ADR-0002 §1 snapshot semantics; ABACUS-wsj).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Eligibility {
    /// Offered for dispatch.
    Eligible,
    /// Deliberately parked: excluded from ready, and never driven into
    /// execution by a projection.
    Parked,
}

/// Read-before-write inspection view: normalized status/revision facts
/// for ambiguity reconciliation and out-of-band correlation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BeadStatusView {
    pub snapshot: BeadSnapshot,
    pub status: WorkStatus,
    /// Preserved rather than collapsed into `status`: see
    /// [`Eligibility`].
    pub eligibility: Eligibility,
    pub revision: WorkRevision,
}

/// Caller-supplied receipt facts used to detect a provider mutation that
/// occurred outside the committed ABACUS operation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExpectedWorkObservation {
    pub status: WorkStatus,
    pub revision: WorkRevision,
    pub operation: OperationId,
}

/// The normalized result of comparing provider facts with a caller's
/// expected receipt. Core correlates the operation against the Ledger; the
/// work module never reads it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkObservation {
    Clean {
        observed: BeadStatusView,
    },
    OutOfBandMutation {
        expected: ExpectedWorkObservation,
        observed: BeadStatusView,
    },
    /// The expected bead no longer exists in the provider. Deletion is
    /// itself an out-of-band mutation, not an ordinary inspection error.
    Missing {
        id: BeadId,
        expected: ExpectedWorkObservation,
    },
}

/// Bounded curated close reasons (ADR-0001 §9.4). Not chosen freely at
/// decision sites: `Accept` projects `AcceptedHandoff` and `Cancel`
/// projects `CancelledObsolete` structurally, so cross-pairing is
/// unrepresentable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CloseReason {
    AcceptedHandoff,
    CancelledObsolete,
}

/// Outcome of a decision-gated work mutation, carrying exactly the
/// provenance the facade possesses (ADR-0001 effect-provenance
/// amendment): what THIS call did. The provider cannot attest ABACUS
/// operation identity on any effect, so neither already-present
/// observation is confirmed application — correlating origin is
/// core's job against the Ledger, and only [`MutationOutcome::Applied`]
/// is receipt-eligible.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MutationOutcome {
    /// The provider attested application of this call's submission.
    Applied {
        before: WorkRevision,
        after: WorkRevision,
        /// Audit-safe normalized summary.
        summary: String,
    },
    /// The read-before-write or terminal gate found these observed
    /// facts before anything was submitted: this call issued nothing.
    FoundBeforeSubmission {
        status: WorkStatus,
        revision: WorkRevision,
    },
    /// This call submitted, the provider's outcome was lost, and the
    /// post-ambiguity re-inspection observed facts satisfying the
    /// target: a typed ambiguous observation, never confirmed
    /// application.
    ObservedAfterAmbiguousSubmission {
        status: WorkStatus,
        revision: WorkRevision,
    },
}

/// Normalized work-seam failures (I9).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkError {
    ProviderUnavailable,
    /// Pinned-version or schema mismatch: fail closed.
    Incompatible,
    Corrupt,
    Busy,
    MalformedOutput,
    NotFound,
    /// The graph moved between bracketed reads; re-read.
    RevisionConflict,
    /// The bead is parked and may not be driven into execution. Only an
    /// explicit authoring un-defer makes it eligible again — a
    /// projection must never do it implicitly (ABACUS-wsj).
    BeadParked,
    /// Mutation outcome unknown: inspect before any retry.
    AmbiguousOutcome,
    /// Scope-label normalization refusals (ADR-0002 §1).
    ScopeLabelMalformed {
        label: String,
    },
    ScopeLabelConflict {
        key: String,
    },
}

/// Normalized reads and decision-gated mutations of the work graph.
/// Only `abacus-work` implements this; the worker role never sees a
/// mutating verb. Every mutation carries the committed authorizing
/// decision's [`OperationId`].
pub trait WorkGraphPort {
    fn ready(&self) -> Result<(WorkRevision, Vec<BeadSnapshot>), WorkError>;

    /// Read-before-write reconciliation primitive.
    fn inspect(&self, id: &BeadId) -> Result<BeadStatusView, WorkError>;

    /// Compare one provider observation with receipt facts supplied by the
    /// caller. This is a read-only correlation primitive: the work module
    /// does not consult the Ledger or infer operation identity. A missing
    /// bead is returned as [`WorkObservation::Missing`]; other inspection
    /// failures remain [`WorkError`]s.
    fn compare_observation(
        &self,
        id: &BeadId,
        expected: &ExpectedWorkObservation,
    ) -> Result<WorkObservation, WorkError> {
        match self.inspect(id) {
            Ok(observed)
                if observed.status == expected.status && observed.revision == expected.revision =>
            {
                Ok(WorkObservation::Clean { observed })
            }
            Ok(observed) => Ok(WorkObservation::OutOfBandMutation {
                expected: expected.clone(),
                observed,
            }),
            Err(WorkError::NotFound) => Ok(WorkObservation::Missing {
                id: id.clone(),
                expected: expected.clone(),
            }),
            Err(error) => Err(error),
        }
    }

    /// Project an Assignment's authorizing decision (architecture
    /// §2.5): the same operation identity that opened the Assignment.
    fn mark_in_progress(
        &self,
        id: &BeadId,
        operation: &OperationId,
        expected: &WorkRevision,
    ) -> Result<MutationOutcome, WorkError>;

    /// Project a committed Acceptance/cancellation decision.
    fn close(
        &self,
        id: &BeadId,
        reason: CloseReason,
        operation: &OperationId,
        expected: &WorkRevision,
    ) -> Result<MutationOutcome, WorkError>;
}

// ---------------------------------------------------------------------------
// Advice
// ---------------------------------------------------------------------------

/// Optional ordering advice (I8): never authoritative, never required.
pub trait WorkAdvicePort {
    fn advise(&self, revision: &WorkRevision, ready: &[BeadId]) -> AdviceOutcome;
}

/// Why advice degraded — a normal noted outcome, never an error.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AdviceDegradation {
    Unavailable,
    Timeout,
    Partial,
    Incompatible,
    Malformed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AdviceOutcome {
    Advice {
        order: Vec<BeadId>,
        bound_to: WorkRevision,
    },
    Degraded {
        reason: AdviceDegradation,
    },
}

/// Deterministic fallback ordering (I8): explicit priority, then
/// stable ID.
pub fn fallback_order(ready: &[BeadSnapshot]) -> Vec<BeadId> {
    let mut beads: Vec<&BeadSnapshot> = ready.iter().collect();
    beads.sort_by(|a, b| {
        a.priority
            .cmp(&b.priority)
            .then_with(|| a.id.as_str().cmp(b.id.as_str()))
    });
    beads.into_iter().map(|b| b.id.clone()).collect()
}

/// Why the single advice gate rejected an answered advice outcome.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AdviceRejection {
    /// Advice bound to a revision other than the bracketed read.
    StaleBinding,
    /// Not an exact permutation of the eligible set.
    NotCovering,
}

/// How an advice solicitation was disposed of by the single advice
/// gate. Degradation and rejection are noted outcomes carried WITH the
/// fallback order (CONTEXT: "the degradation is noted in output") —
/// never errors, and never silently erased.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AdviceDisposition {
    /// The advisor's permutation governs the order.
    Followed,
    /// The advisor itself degraded; the deterministic fallback governs.
    Degraded { reason: AdviceDegradation },
    /// The advisor answered but the gate rejected the answer; the
    /// deterministic fallback governs.
    Rejected { reason: AdviceRejection },
}

/// The single advice gate (core invariant 6), with its disposition:
/// accept only advice bound to the current revision whose order is an
/// exact permutation of the eligible set; otherwise the deterministic
/// fallback, with the reason noted.
pub fn appraise_advice(
    ready: &[BeadSnapshot],
    outcome: &AdviceOutcome,
    current: &WorkRevision,
) -> (Vec<BeadId>, AdviceDisposition) {
    let AdviceOutcome::Advice { order, bound_to } = outcome else {
        let reason = match outcome {
            AdviceOutcome::Degraded { reason } => *reason,
            AdviceOutcome::Advice { .. } => unreachable!("matched above"),
        };
        return (
            fallback_order(ready),
            AdviceDisposition::Degraded { reason },
        );
    };
    if bound_to != current {
        return (
            fallback_order(ready),
            AdviceDisposition::Rejected {
                reason: AdviceRejection::StaleBinding,
            },
        );
    }
    if order.len() != ready.len() {
        return (
            fallback_order(ready),
            AdviceDisposition::Rejected {
                reason: AdviceRejection::NotCovering,
            },
        );
    }
    let mut expected: Vec<&str> = ready.iter().map(|b| b.id.as_str()).collect();
    let mut proposed: Vec<&str> = order.iter().map(|b| b.as_str()).collect();
    expected.sort_unstable();
    proposed.sort_unstable();
    if expected != proposed {
        return (
            fallback_order(ready),
            AdviceDisposition::Rejected {
                reason: AdviceRejection::NotCovering,
            },
        );
    }
    (order.clone(), AdviceDisposition::Followed)
}

/// The order alone, for callers with no disposition to carry. Same
/// gate — this delegates to [`appraise_advice`].
pub fn apply_advice(
    ready: &[BeadSnapshot],
    outcome: &AdviceOutcome,
    current: &WorkRevision,
) -> Vec<BeadId> {
    appraise_advice(ready, outcome, current).0
}

// ---------------------------------------------------------------------------
// Durable workflow state (Scribe)
// ---------------------------------------------------------------------------

/// The exact canonical sanitized Envelope snapshot persisted through
/// Scribe before any live delivery (architecture §3).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnvelopeSnapshot {
    content: String,
    pub content_hash: ContentHash,
}

/// Envelope payloads are explicitly bounded (state protocol rules).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct EnvelopeTooLarge;

impl EnvelopeSnapshot {
    pub const MAX_BYTES: usize = 65536;

    pub fn new(content: String, content_hash: ContentHash) -> Result<Self, EnvelopeTooLarge> {
        if content.len() > Self::MAX_BYTES {
            return Err(EnvelopeTooLarge);
        }
        Ok(Self {
            content,
            content_hash,
        })
    }

    pub fn content(&self) -> &str {
        &self.content
    }
}

/// Assignment content, snapshotting everything Acceptance rechecks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssignmentRecord {
    pub id: AssignmentId,
    pub bead: BeadId,
    pub bead_content_hash: ContentHash,
    pub scope_map: ScopeMap,
    /// Full worker snapshot (actor, class, profile, hash) so fenced
    /// mutations remain historically reconstructible.
    pub worker: DecisionActor,
    pub decision_actor: DecisionActor,
    pub edit_scope: EditScope,
    /// The always-present acceptance policy (verification set plus
    /// form); a worker cannot later select or weaken it, and no state
    /// exists in which an Assignment binds no verification.
    pub acceptance: AcceptancePolicy,
    /// The Assignment's authored attempt policy (optional bounded cap).
    pub attempt_policy: AttemptPolicy,
    pub declared_base: CommitId,
}

/// An Attempt with its Lease (acquisition happens here; renewal is
/// fenced; expiry is time-derived; supersession is the next Attempt's
/// monotonic token).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttemptRecord {
    pub id: AttemptId,
    pub assignment: AssignmentId,
    pub lease: Lease,
}

/// Bounded human-facing reason on every terminal decision (core
/// invariant 7).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecisionReason(String);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DecisionReasonError {
    Empty,
    TooLong,
    InvalidCharacter,
}

impl DecisionReason {
    pub fn new(raw: &str) -> Result<Self, DecisionReasonError> {
        if raw.is_empty() {
            return Err(DecisionReasonError::Empty);
        }
        if raw.len() > 200 {
            return Err(DecisionReasonError::TooLong);
        }
        if raw.bytes().any(|b| b.is_ascii_control()) {
            return Err(DecisionReasonError::InvalidCharacter);
        }
        Ok(Self(raw.to_owned()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Bounded canonical set of operation identities: duplicate-free,
/// sorted, at most 64; may be empty (emptiness is itself a checkable
/// submission fact).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct OperationSet(Vec<OperationId>);

impl OperationSet {
    pub fn new(mut operations: Vec<OperationId>) -> Result<Self, crate::evidence::CollectionError> {
        if operations.len() > 64 {
            return Err(crate::evidence::CollectionError::TooMany);
        }
        operations.sort();
        for pair in operations.windows(2) {
            if pair[0] == pair[1] {
                return Err(crate::evidence::CollectionError::Duplicate(
                    pair[0].as_str().to_owned(),
                ));
            }
        }
        Ok(Self(operations))
    }

    pub fn iter(&self) -> impl Iterator<Item = &OperationId> {
        self.0.iter()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }
}

/// A worker's immutable completion claim (CONTEXT §2), bound to its
/// Attempt, commit, and evidence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HandoffRecord {
    pub id: HandoffId,
    pub attempt: AttemptId,
    pub commit: CommitId,
    pub expected_base: CommitId,
    /// Proof the worktree was clean at submission: the workspace
    /// digest observed at the Handoff commit.
    pub clean_tree: WorkspaceDigest,
    pub changed_paths: PathSet,
    /// Evidence referenced by the fenced operations that recorded it.
    pub evidence_operations: OperationSet,
    /// Attestation binding the evidence set to the handed-off commit.
    pub attestation: ContentHash,
}

/// The typed authorizing input for opening an Assignment: an Assign
/// decision and nothing else (S2 — a generic decision cannot authorize
/// an opening).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssignDecision {
    /// The single operation identity for the opening transaction AND
    /// its mark-in-progress projection (architecture §2.4-2.5).
    pub operation: OperationId,
    pub assignment: AssignmentId,
    pub first_attempt: AttemptId,
    /// Acted-under authority: actor + capability + canonical scope.
    pub authority: AuthoritySnapshot,
}

/// The typed authorizing input for an explicit retry: appends a new
/// fenced Attempt atomically (S2 — only a Retry decision can authorize
/// a successor Attempt; the core-validated Attempt cap applies).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RetryDecision {
    pub operation: OperationId,
    pub assignment: AssignmentId,
    pub authority: AuthoritySnapshot,
    pub reason: DecisionReason,
}

/// Orchestrator decision payloads: illegal terminal shapes are
/// unrepresentable — Accept/Reject bind the Handoff they decide,
/// transfer binds the recipient's full validated actor snapshot (S5),
/// and every kind carries its bounded reason. Close reasons are
/// derived: Accept ⇒ `AcceptedHandoff`, Cancel ⇒ `CancelledObsolete`.
/// Assign and Retry are not here: they exist only as the typed inputs
/// of their transactions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DecisionKind {
    /// Accept/Reject bind only the immutable Handoff; the decided
    /// Attempt is derived transactionally from that Handoff record —
    /// carrying it separately would reintroduce the mismatch.
    Accept {
        handoff: HandoffId,
        reason: DecisionReason,
    },
    Reject {
        handoff: HandoffId,
        reason: DecisionReason,
    },
    Cancel {
        reason: DecisionReason,
    },
    Revoke {
        attempt: AttemptId,
        reason: DecisionReason,
    },
    Reclaim {
        attempt: AttemptId,
        reason: DecisionReason,
    },
    TransferAuthority {
        to: DecisionActor,
        reason: DecisionReason,
    },
}

impl DecisionKind {
    /// The structurally derived work-close projection, when one exists.
    pub fn close_reason(&self) -> Option<CloseReason> {
        match self {
            DecisionKind::Accept { .. } => Some(CloseReason::AcceptedHandoff),
            DecisionKind::Cancel { .. } => Some(CloseReason::CancelledObsolete),
            _ => None,
        }
    }
}

/// An immutable fenced decision with the actor snapshot I17 requires.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecisionRecord {
    pub operation: OperationId,
    pub assignment: AssignmentId,
    pub authority: AuthoritySnapshot,
    /// The subject Attempt, when the kind has one, lives inside
    /// `kind` — there is no free-floating attempt field to contradict
    /// it (S3/F3).
    pub kind: DecisionKind,
    /// A Signal this decision resolves, if any.
    pub resolves: Option<SignalId>,
}

/// The explicit-retry bundle (mirror of [`AssignmentOpening`]): a new
/// fenced Attempt and its authorizing Retry decision, committed in ONE
/// transaction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttemptOpening {
    pub authorizing: RetryDecision,
    pub attempt: AttemptRecord,
}

/// The architecture §2.4 opening bundle, committed in ONE Scribe
/// transaction under the Assign decision's single operation identity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssignmentOpening {
    pub assignment: AssignmentRecord,
    pub first_attempt: AttemptRecord,
    pub authorizing: AssignDecision,
    pub bead_revision: WorkRevision,
}

/// The closed set of activation cases. There is still no general enrolment
/// verb: creation is operator-channel-only, an actor-authorized call may only
/// reactivate an already-registered actor (same ActorId and class), and a
/// first worker is registered as an effect of `AssignmentOpening` — never
/// through this call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ActivationCase {
    /// Initial/orchestrator actor creation. Scribe accepts this ONLY on
    /// the pre-listen operator admin channel; the same message arriving
    /// on the agent-facing protocol is unknown/forbidden.
    OperatorBootstrap,
    /// Reactivation of an already-registered actor: Scribe proves the
    /// target exists with the same ActorId and authority class, the caller
    /// holds the explicit rotation capability, and the activation generation
    /// advances.
    ActorAuthorizedRotation { authority: AuthoritySnapshot },
    /// Operator-channel recovery for an **already registered** orchestrator
    /// whose activation is unavailable (R5.13). Accepted ONLY on the
    /// pre-listen operator channel —
    /// never on the agent protocol, never from a caller-supplied
    /// authority snapshot — and it never creates a new actor: the
    /// target must already exist with the same ActorId and class.
    /// This is why bootstrap can stay strictly one-shot.
    OperatorRecovery,
    /// Operator-channel enrolment of a **new** orchestrator actor into
    /// another validated orchestrator profile (R5.15) — the topology
    /// activation path CONTEXT I16 and ADR-0002 §7 require, since
    /// parallel decision capacity comes from more orchestrator
    /// profiles partitioning scope. Accepted ONLY on the pre-listen
    /// operator channel — never the agent protocol, so this is not a
    /// general enrolment verb — for an actor Scribe does not yet know,
    /// orchestrator class only, subject to the ordinary occupancy and
    /// configuration validation, and it never touches the one-shot
    /// bootstrap sentinel.
    OperatorOrchestratorEnrolment,
}

/// Activation bundled with its closed activation case and committed
/// atomically.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActivationOpening {
    pub activation: ProfileActivation,
    pub case: ActivationCase,
}

/// Fenced worker call identity (ADR-0003 revision 6): the non-secret
/// Attempt locator, current fencing token, and idempotency operation.
/// Scribe derives Assignment and worker authority from durable state;
/// callers cannot assert either one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FencedCall {
    pub attempt: AttemptId,
    pub token: FencingToken,
    pub operation: OperationId,
}

/// A substantive fenced worker action, optionally linked as the
/// response to a committed Directive on the same Attempt. The link is
/// part of the action's idempotent identity. Lease renewal deliberately
/// accepts [`FencedCall`] instead, so a semantically void response link
/// on lease machinery is unrepresentable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FencedAction {
    pub call: FencedCall,
    pub responds_to: Option<SignalId>,
}

/// Every fenced response mechanically surfaces the Attempt's current
/// binding Directives and Ledger head — present even when empty.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FencedResponse {
    pub applied: StateApplied,
    pub binding_directives: Vec<Signal>,
    pub head: Seq,
}

/// The honest idempotency identity attached to an audit event. Most state
/// mutations are operation-owned; a direct Signal append is intentionally
/// owned by its globally unique Signal identity and does not invent a second
/// operation field merely for audit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuditOperation {
    Operation(OperationId),
    Signal(SignalId),
}

/// The strongest initiator fact a state operation structurally proves.
///
/// This is deliberately not a speculative caller-authentication envelope.
/// A future protocol may strengthen individual calls, but it must do so as an
/// explicit seam extension rather than reinterpret already-committed events.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuditInitiator {
    /// Full acted-under identity, including capability and canonical scope.
    Authority(AuthoritySnapshot),
    /// The complete worker binding recovered from durable Assignment state.
    /// It proves actor, profile snapshot, Assignment, and Attempt; it does not
    /// fabricate an exercised capability or scope that the current call lacks.
    WorkerBinding {
        actor: DecisionActor,
        assignment: AssignmentId,
        attempt: AttemptId,
    },
    /// A v1 operation accepted only through the pre-listen operator channel.
    OperatorChannel,
    /// A projection whose authority joins to an operation already committed
    /// in the Ledger. Implementations validate that join before mutation.
    SystemProjection { authorizing: OperationId },
}

/// Closed typed subject families for audit filtering. Workflow subjects reuse
/// the normative four-shape [`SubjectRef`] family; launch associations and
/// operator profile membership are distinct state facts rather than invented
/// workflow subjects.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuditSubject {
    Workflow(SubjectRef),
    ActorProfile {
        actor: ActorId,
        profile: ProfileName,
    },
    Launch(LaunchSubject),
    Projection(OperationId),
}

/// Stable coarse event classes accepted by [`AuditQuery`]. Exact event kinds
/// remain closed and typed; the class exists only to avoid a free-text query
/// language.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AuditClass {
    Assignment,
    Attempt,
    Decision,
    Profile,
    Signal,
    Report,
    Evidence,
    Handoff,
    Lease,
    Envelope,
    Runtime,
    Application,
}

/// Payload-free classification of one decision kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AuditDecisionKind {
    Accept,
    Reject,
    Cancel,
    Revoke,
    Reclaim,
    TransferAuthority,
}

/// Payload-free classification of one activation case.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AuditActivationCase {
    OperatorBootstrap,
    ActorAuthorizedRotation,
    OperatorRecovery,
    OperatorOrchestratorEnrolment,
}

/// Audit-safe Handoff refusal category. Detailed paths and evidence records
/// stay in the operation-owned durable outcome and are joined by operation and
/// sequence; the audit index never copies record bodies.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AuditSubmissionRefusal {
    DirtyWorktree,
    MissingEvidence,
    EvidenceWrongCommit,
    FailingOutcome,
    EditScopeViolation,
    Directive(DirectiveGateRefusal),
    RedGreen,
}

/// Payload-free classification of an application attempt outcome,
/// mirroring the provenance the recorded outcome carries (ADR-0001
/// effect-provenance amendment).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AuditApplicationOutcome {
    Applied,
    FoundPresent,
    ObservedAfterAmbiguous,
    Failed,
    Ambiguous,
}

/// What class of durable mutation committed. Variants carry only typed
/// identities and closed-enum reason/outcome classes, never owning record
/// bodies. Those remain in their canonical Ledger records.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuditKind {
    AssignmentOpened,
    AttemptOpened,
    DecisionRecorded { kind: AuditDecisionKind },
    ProfileActivated { case: AuditActivationCase },
    ProfileDeactivated,
    DirectiveAppended { signal: SignalId },
    RequestAppended { signal: SignalId },
    ReportRecorded { signal: SignalId },
    ReportRefused { reason: DirectiveGateRefusal },
    EvidenceRecorded,
    EvidenceRefused { reason: DirectiveGateRefusal },
    HandoffRecorded { handoff: HandoffId },
    HandoffRefused { reason: AuditSubmissionRefusal },
    LeaseRenewed,
    AttemptAborted,
    EnvelopePersisted,
    RuntimeHandleBound,
    RuntimeHandleUnbound,
    RuntimeObservationRecorded,
    ApplicationAttemptRecorded { outcome: AuditApplicationOutcome },
    ApplicationReceiptRecorded,
}

impl AuditKind {
    pub fn class(&self) -> AuditClass {
        match self {
            Self::AssignmentOpened => AuditClass::Assignment,
            Self::AttemptOpened | Self::AttemptAborted => AuditClass::Attempt,
            Self::DecisionRecorded { .. } => AuditClass::Decision,
            Self::ProfileActivated { .. } | Self::ProfileDeactivated => AuditClass::Profile,
            Self::DirectiveAppended { .. } | Self::RequestAppended { .. } => AuditClass::Signal,
            Self::ReportRecorded { .. } | Self::ReportRefused { .. } => AuditClass::Report,
            Self::EvidenceRecorded | Self::EvidenceRefused { .. } => AuditClass::Evidence,
            Self::HandoffRecorded { .. } | Self::HandoffRefused { .. } => AuditClass::Handoff,
            Self::LeaseRenewed => AuditClass::Lease,
            Self::EnvelopePersisted => AuditClass::Envelope,
            Self::RuntimeHandleBound
            | Self::RuntimeHandleUnbound
            | Self::RuntimeObservationRecorded => AuditClass::Runtime,
            Self::ApplicationAttemptRecorded { .. } | Self::ApplicationReceiptRecorded => {
                AuditClass::Application
            }
        }
    }

    pub fn decision(kind: &DecisionKind) -> Self {
        let kind = match kind {
            DecisionKind::Accept { .. } => AuditDecisionKind::Accept,
            DecisionKind::Reject { .. } => AuditDecisionKind::Reject,
            DecisionKind::Cancel { .. } => AuditDecisionKind::Cancel,
            DecisionKind::Revoke { .. } => AuditDecisionKind::Revoke,
            DecisionKind::Reclaim { .. } => AuditDecisionKind::Reclaim,
            DecisionKind::TransferAuthority { .. } => AuditDecisionKind::TransferAuthority,
        };
        Self::DecisionRecorded { kind }
    }

    pub fn activation(case: &ActivationCase) -> Self {
        let case = match case {
            ActivationCase::OperatorBootstrap => AuditActivationCase::OperatorBootstrap,
            ActivationCase::ActorAuthorizedRotation { .. } => {
                AuditActivationCase::ActorAuthorizedRotation
            }
            ActivationCase::OperatorRecovery => AuditActivationCase::OperatorRecovery,
            ActivationCase::OperatorOrchestratorEnrolment => {
                AuditActivationCase::OperatorOrchestratorEnrolment
            }
        };
        Self::ProfileActivated { case }
    }

    pub fn signal(signal: &Signal) -> Self {
        match &signal.body {
            SignalBody::Directive { .. } => Self::DirectiveAppended {
                signal: signal.id.clone(),
            },
            SignalBody::Request { .. } => Self::RequestAppended {
                signal: signal.id.clone(),
            },
            SignalBody::Report { .. } => Self::ReportRecorded {
                signal: signal.id.clone(),
            },
        }
    }

    pub fn report(outcome: &ReportOutcome) -> Self {
        match outcome {
            ReportOutcome::Recorded { signal } => Self::ReportRecorded {
                signal: signal.id.clone(),
            },
            ReportOutcome::Refused { reason } => Self::ReportRefused { reason: *reason },
        }
    }

    pub fn evidence(outcome: EvidenceOutcome) -> Self {
        match outcome {
            EvidenceOutcome::Recorded => Self::EvidenceRecorded,
            EvidenceOutcome::Refused { reason } => Self::EvidenceRefused { reason },
        }
    }

    pub fn handoff(outcome: &SubmissionOutcome) -> Self {
        match outcome {
            SubmissionOutcome::Recorded { handoff } => Self::HandoffRecorded {
                handoff: handoff.clone(),
            },
            SubmissionOutcome::Refused { reason } => Self::HandoffRefused {
                reason: AuditSubmissionRefusal::from(reason),
            },
        }
    }

    pub fn application(outcome: &ApplicationOutcome) -> Self {
        let outcome = match outcome {
            ApplicationOutcome::Applied { .. } => AuditApplicationOutcome::Applied,
            ApplicationOutcome::FoundPresent { .. } => AuditApplicationOutcome::FoundPresent,
            ApplicationOutcome::ObservedAfterAmbiguous { .. } => {
                AuditApplicationOutcome::ObservedAfterAmbiguous
            }
            ApplicationOutcome::Failed { .. } => AuditApplicationOutcome::Failed,
            ApplicationOutcome::Ambiguous => AuditApplicationOutcome::Ambiguous,
        };
        Self::ApplicationAttemptRecorded { outcome }
    }
}

impl From<&SubmissionRefusalReason> for AuditSubmissionRefusal {
    fn from(reason: &SubmissionRefusalReason) -> Self {
        match reason {
            SubmissionRefusalReason::DirtyWorktree { .. } => Self::DirtyWorktree,
            SubmissionRefusalReason::MissingEvidence => Self::MissingEvidence,
            SubmissionRefusalReason::EvidenceWrongCommit => Self::EvidenceWrongCommit,
            SubmissionRefusalReason::FailingOutcome => Self::FailingOutcome,
            SubmissionRefusalReason::EditScopeViolation { .. } => Self::EditScopeViolation,
            SubmissionRefusalReason::Directive(reason) => Self::Directive(*reason),
            SubmissionRefusalReason::RedGreen(_) => Self::RedGreen,
        }
    }
}

/// One immutable audit index record. `seq` is always the transaction's final
/// ordering position: a multi-position fenced Report has one event at its
/// final call position, none at its intermediate Signal position, and no
/// ordering position can own more than one event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuditEvent {
    pub seq: Seq,
    pub at: Timestamp,
    pub initiator: AuditInitiator,
    pub operation: AuditOperation,
    pub subject: AuditSubject,
    pub kind: AuditKind,
}

/// AND-composed, typed audit filters. Bounds are inclusive and results are
/// always returned in ascending Ledger order. There is intentionally no
/// free-text predicate.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AuditQuery {
    pub subject: Option<AuditSubject>,
    pub class: Option<AuditClass>,
    pub from: Option<Seq>,
    pub through: Option<Seq>,
}

/// One actor-reported, non-authoritative runtime observation. The normalized
/// observation carries its observation time; the linked [`AuditEvent`] also
/// records the distinct Ledger commit time. This record is audit-only and can
/// never establish completion, assignment state, or current liveness alone.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeObservationRecord {
    pub reporter: AuthoritySnapshot,
    pub subject: LaunchSubject,
    pub observation: LivenessObservation,
}

/// Outcome of a fenced worker Report append. A binding Abort is an
/// audited domain refusal, not a protocol failure: the operation owns
/// the refusal and the caller still receives its [`FencedResponse`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReportOutcome {
    Recorded { signal: Box<Signal> },
    Refused { reason: DirectiveGateRefusal },
}

/// Outcome of a fenced worker Evidence append. Evidence has no
/// separately identified state record at this seam, so the recorded
/// variant carries no payload.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EvidenceOutcome {
    Recorded,
    Refused { reason: DirectiveGateRefusal },
}

/// Distinct Submission-refusal reasons (CONTEXT §2/§6): audited,
/// creating no Handoff and leaving the Attempt active.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SubmissionRefusalReason {
    /// The dirt, enumerated — never a bare flag.
    DirtyWorktree {
        paths: PathSet,
    },
    MissingEvidence,
    EvidenceWrongCommit,
    FailingOutcome,
    /// The escaping paths, enumerated.
    EditScopeViolation {
        paths: PathSet,
    },
    Directive(DirectiveGateRefusal),
    RedGreen(PairRefusal),
}

/// Outcome of a fenced Handoff submission. The call's operation
/// identity idempotently owns the WHOLE outcome, recorded or refused:
/// an identical retry returns the same stored outcome, and the same
/// operation with a different Handoff candidate is a conflict (S3).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SubmissionOutcome {
    Recorded { handoff: HandoffId },
    Refused { reason: SubmissionRefusalReason },
}

/// Outcome of one work-status application attempt: recorded immutably
/// whatever happened, mirroring the mutation's provenance so the
/// Ledger's attempt history states what each attempt actually proved
/// (ADR-0001 effect-provenance amendment). Only `Applied` is
/// receipt-eligible — at composition AND at receipt validation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApplicationOutcome {
    /// The provider attested application of this attempt's submission.
    Applied {
        before: WorkRevision,
        after: WorkRevision,
    },
    /// Observed facts found before anything was submitted; whose
    /// effect this is cannot be proven, so never receipted.
    FoundPresent {
        status: WorkStatus,
        revision: WorkRevision,
    },
    /// Satisfying facts observed after this attempt's own ambiguous
    /// submission: possibly applied, never confirmed, never receipted.
    ObservedAfterAmbiguous {
        status: WorkStatus,
        revision: WorkRevision,
    },
    Failed {
        error: WorkError,
    },
    Ambiguous,
}

/// Immutable application-attempt record. `id` is this attempt's own
/// idempotency identity; `decision` is its target.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApplicationAttempt {
    pub id: OperationId,
    /// The projection this attempt targets — an Assignment opening is
    /// not a `DecisionRecord`, so this is a projection target, not a
    /// "decision" (R5.26).
    pub target: OperationId,
    pub outcome: ApplicationOutcome,
}

/// An immutable receipt naming the EXACT successful attempt that
/// justifies clearing a pending projection (R5.26). Scribe validates
/// that the attempt exists, targets this projection, succeeded, and
/// reports the same after-revision.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApplicationReceipt {
    pub target: OperationId,
    pub attempt: OperationId,
    pub after: WorkRevision,
}

/// One atomic, idempotent state operation outcome.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum StateApplied {
    Applied,
    /// The identical operation was already committed (lost response).
    AlreadyApplied,
}

/// Normalized state-seam failures (CONTEXT §6): loud and distinct.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StateError {
    /// Scribe unreachable: halt and preserve the worktree.
    Unavailable,
    /// Fencing token stale or unknown (core invariant 4).
    StaleFencing,
    /// Lease expired: the Attempt is reclaimable, not writable.
    /// Reserved for `now > current expiry` — never for a malformed
    /// renewal request.
    LeaseExpired,
    /// A renewal that does not extend the current deadline: a
    /// malformed request against a LIVE lease, not an expiry.
    NonExtendingLease,
    /// A decision call's authenticated actor is not the record's bound actor.
    ActorMismatch,
    /// Actor's current grant does not cover the subject (I17).
    ScopeUnauthorized,
    /// A singleton-occupancy profile is already occupied (ADR-0002 §7).
    ProfileOccupied,
    /// The activation case is invalid for the target (e.g. bootstrap
    /// of a non-orchestrator, or a case-specific precondition).
    ActivationCaseInvalid,
    /// The one-shot operator bootstrap has already committed; later
    /// actors arrive by rotation or opening, never by bootstrap.
    BootstrapAlreadyComplete,
    /// Deactivation named an actor that does not hold the profile.
    NotTheOccupant,
    /// A transactional bundle's identities disagree (e.g. the
    /// Assignment named by its authorizing decision differs from the
    /// Assignment record, or the first Attempt differs).
    IncoherentBundle,
    /// Rotation named an actor Scribe has never registered.
    UnknownActor,
    /// An ActorId registers/resumes exactly one stable AuthorityClass;
    /// activating under a different class is refused (this structurally
    /// preserves "a worker cannot accept its own handoff").
    ActorClassMismatch,
    /// The worker attempted the explicit abort-compliance terminal call
    /// without a currently binding Abort Directive. The operation is not
    /// claimed; voluntary worker self-cancellation remains unrepresentable.
    AbortNotInForce,
    /// Same operation identity, different content: corrupt input.
    ConflictingOperation,
    UnknownRecord,
    /// Ledger unreadable/corrupt: fail closed, surface to operator.
    Corrupt,
}

/// Evidence with the identities that make it resolvable from a
/// Handoff's claimed set (S3).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EvidenceRecord {
    pub operation: OperationId,
    pub attempt: AttemptId,
    pub evidence: Evidence,
}

/// What a committed decision projects onto the work graph. Only these
/// decisions project: an Assignment opening marks its bead in
/// progress; Accept and Cancel close it. Reject, Revoke, Reclaim, and
/// TransferAuthority change no work status and never enter the
/// application set (R5.25).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkProjection {
    MarkInProgress,
    Close { reason: CloseReason },
}

/// The recoverable crash window's derived recovery basis (ADR-0001
/// effect-provenance amendment): a previously recorded `Applied`
/// attempt that lacks only its receipt. When more than one exists for
/// the target, this is deterministically the EARLIEST Ledger-order
/// `Applied` attempt — the first confirmed application is the recovery
/// basis, and later history never rewrites it. The portable state
/// contract requires that choice identically of every implementation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReceiptCandidate {
    /// The original applied attempt the recovered receipt credits.
    pub attempt: OperationId,
    pub after: WorkRevision,
}

/// A committed projection in the ACTIONABLE pending set — lacking a
/// successful receipt AND not causally superseded — carrying the typed
/// projection plus the identities reconciliation needs without a
/// second read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingApplication {
    /// The authorizing operation identity — also the application
    /// target.
    pub operation: OperationId,
    pub assignment: AssignmentId,
    pub bead: BeadId,
    pub projection: WorkProjection,
    /// Ledger commit order, so reconciliation applies projections
    /// causally rather than by accidental key ordering (R5.26).
    pub committed_at: Seq,
    /// Present only where a revision was genuinely authorized at
    /// commit time (the opening's bead revision). Close projections
    /// carry `None`: reconciliation must inspect fresh rather than
    /// reuse an older revision and call it authorized.
    pub authorized_revision: Option<WorkRevision>,
    /// Present when the recoverable crash window applies: recovery
    /// records this receipt directly, issuing no work mutation and
    /// consuming no fresh attempt identity.
    pub receipt_candidate: Option<ReceiptCandidate>,
}

/// A projection excluded from the actionable pending set by causal
/// supersession: a `MarkInProgress` projection followed in Ledger
/// order by a committed Close projection for the exact same Assignment
/// (ADR-0001 effect-provenance amendment). A derived disposition — not
/// a receipt, attempt, mutation, decision, or waiver — preserving both
/// operations and their history.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SupersededApplication {
    pub application: PendingApplication,
    /// The later committed close projection's operation identity.
    pub superseded_by: OperationId,
}

/// Composed current state of an Assignment as recorded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssignmentView {
    pub record: AssignmentRecord,
    pub state: AssignmentState,
    pub attempts: Vec<(AttemptId, AttemptState)>,
    /// Ledger commit cursor for causal ordering.
    pub head: Seq,
}

/// Worker-reachable Scribe operations. This trait contains no decision,
/// profile, runtime-association, application, or manager-wide query verb, so
/// handing only this trait to worker composition makes the authority boundary
/// a type fact (ADR-0003 revision 6).
pub trait WorkerWorkflowStatePort {
    /// Fenced worker Report append; Scribe derives sender, Assignment, and
    /// subject from the Attempt locator and allocates `Seq`. A binding Abort
    /// is returned in-band with the response envelope.
    fn fenced_report(
        &self,
        action: &FencedAction,
        draft: &ReportDraft,
    ) -> Result<(ReportOutcome, FencedResponse), StateError>;

    /// Fenced worker Evidence append. A binding Abort is returned
    /// in-band with the response envelope.
    fn fenced_evidence(
        &self,
        action: &FencedAction,
        evidence: &Evidence,
    ) -> Result<(EvidenceOutcome, FencedResponse), StateError>;

    /// Fenced Handoff submission: records the immutable Handoff and the
    /// Attempt's `Submitted` transition, or audits a Submission refusal
    /// that creates no Handoff and leaves the Attempt active. The
    /// call's operation idempotently owns whichever outcome occurred.
    fn fenced_submit_handoff(
        &self,
        action: &FencedAction,
        handoff: &HandoffRecord,
    ) -> Result<(SubmissionOutcome, FencedResponse), StateError>;

    /// Explicit worker compliance with a binding Abort Directive. The bare
    /// call carries no response link because the terminal action is itself
    /// the typed response. Exact replay is recognized before all mutable
    /// validation and returns the causally current response envelope.
    fn fenced_abort_attempt(&self, call: &FencedCall) -> Result<FencedResponse, StateError>;

    /// Fenced lease renewal.
    fn renew_lease(
        &self,
        call: &FencedCall,
        until: Timestamp,
    ) -> Result<(Lease, FencedResponse), StateError>;
}

/// Internal aggregate implemented by the in-memory and SQLite state engines.
/// Portable contracts may use it directly; worker composition and dispatch
/// receive only [`WorkerWorkflowStatePort`].
pub trait WorkflowStatePort {
    /// Commit the complete opening bundle in one transaction under the
    /// Assign decision's operation identity.
    fn open_assignment(&self, opening: &AssignmentOpening) -> Result<StateApplied, StateError>;

    /// Append a new fenced Attempt atomically with its authorizing
    /// Retry decision.
    fn append_attempt(&self, opening: &AttemptOpening) -> Result<StateApplied, StateError>;

    /// Record an orchestrator decision (accept/reject/cancel/revoke/
    /// reclaim/transfer).
    fn record_decision(&self, record: &DecisionRecord) -> Result<StateApplied, StateError>;

    /// Audited profile activation (ADR-0002 §7; see [`ActivationCase`]
    /// for the closed set). Initial bootstrap arrives on Scribe's
    /// pre-listen operator channel; every later activation/reactivation
    /// is authorized by an already-enrolled decision actor whose
    /// authority is recorded.
    fn activate_profile(&self, opening: &ActivationOpening) -> Result<StateApplied, StateError>;

    /// Explicit audited deactivation.
    fn deactivate_profile(
        &self,
        operation: &OperationId,
        actor: &ActorId,
        profile: &ProfileName,
    ) -> Result<StateApplied, StateError>;

    /// Orchestrator-side Signal append. Scribe allocates `Seq` and
    /// returns the committed Signal — identically on idempotent retry.
    fn append_signal(&self, draft: &SignalDraft) -> Result<(Signal, StateApplied), StateError>;

    fn fenced_report(
        &self,
        action: &FencedAction,
        draft: &ReportDraft,
    ) -> Result<(ReportOutcome, FencedResponse), StateError>;

    fn fenced_evidence(
        &self,
        action: &FencedAction,
        evidence: &Evidence,
    ) -> Result<(EvidenceOutcome, FencedResponse), StateError>;

    fn fenced_submit_handoff(
        &self,
        action: &FencedAction,
        handoff: &HandoffRecord,
    ) -> Result<(SubmissionOutcome, FencedResponse), StateError>;

    fn fenced_abort_attempt(&self, call: &FencedCall) -> Result<FencedResponse, StateError>;

    fn renew_lease(
        &self,
        call: &FencedCall,
        until: Timestamp,
    ) -> Result<(Lease, FencedResponse), StateError>;

    /// Persist the canonical sanitized Envelope snapshot before any
    /// live delivery (architecture §3.3), keyed by the same closed
    /// launch subject used for launch and handle association: worker
    /// Assignment Envelopes are unchanged semantically, and a spawned
    /// orchestrator/watchdog gets its activation/profile Envelope
    /// (R5.19 addendum).
    fn persist_envelope(
        &self,
        operation: &OperationId,
        subject: &LaunchSubject,
        envelope: &EnvelopeSnapshot,
    ) -> Result<StateApplied, StateError>;

    /// Read the persisted Envelope for a launch subject.
    fn envelope(&self, subject: &LaunchSubject) -> Result<EnvelopeSnapshot, StateError>;

    /// Associate an opaque runtime handle with a launch subject
    /// (architecture
    /// §3.5). Re-association after generation change is a new explicit
    /// bind; reconciling an uncertain association is unbind+bind under
    /// their own operations.
    fn bind_runtime_handle(
        &self,
        operation: &OperationId,
        subject: &LaunchSubject,
        handle: &RuntimeHandle,
    ) -> Result<StateApplied, StateError>;

    fn unbind_runtime_handle(
        &self,
        operation: &OperationId,
        subject: &LaunchSubject,
    ) -> Result<StateApplied, StateError>;

    fn runtime_handle(&self, subject: &LaunchSubject) -> Result<Option<RuntimeHandle>, StateError>;

    /// Record one explicitly reported runtime observation as immutable,
    /// non-authoritative audit data.
    fn record_runtime_observation(
        &self,
        operation: &OperationId,
        record: &RuntimeObservationRecord,
    ) -> Result<StateApplied, StateError>;

    /// Read the runtime-observation body joined by its operation identity.
    fn runtime_observation(
        &self,
        operation: &OperationId,
    ) -> Result<RuntimeObservationRecord, StateError>;

    /// Record one immutable application attempt (any outcome).
    fn record_application_attempt(
        &self,
        attempt: &ApplicationAttempt,
    ) -> Result<StateApplied, StateError>;

    /// Record the confirmed-success receipt; pending remains
    /// "projections lacking a successful receipt".
    fn record_application_receipt(
        &self,
        receipt: &ApplicationReceipt,
    ) -> Result<StateApplied, StateError>;

    fn assignment(&self, id: &AssignmentId) -> Result<AssignmentView, StateError>;

    /// Evidence for an Attempt with resolvable identities, so
    /// Acceptance can resolve a Handoff's claimed evidence set (S3).
    fn evidence_for(&self, attempt: &AttemptId) -> Result<Vec<EvidenceRecord>, StateError>;

    /// Signals about an Attempt in causal order.
    fn signals_for(&self, attempt: &AttemptId) -> Result<Vec<Signal>, StateError>;

    /// One immutable Handoff by identity (acceptance derives the
    /// decided Attempt from this record).
    fn handoff(&self, id: &HandoffId) -> Result<HandoffRecord, StateError>;

    /// One committed decision by operation identity.
    fn decision(&self, operation: &OperationId) -> Result<DecisionRecord, StateError>;

    /// Active occupants of a profile (route resolves profile → active
    /// actor through this).
    fn active_occupants(&self, profile: &ProfileName) -> Result<Vec<ActorId>, StateError>;

    /// The derived ACTIONABLE reconciliation set: committed projections
    /// lacking a successful receipt AND not causally superseded
    /// (ADR-0001 effect-provenance amendment), in Ledger commit order,
    /// each carrying its `receipt_candidate` when the recoverable
    /// crash window applies.
    fn pending_applications(&self) -> Result<Vec<PendingApplication>, StateError>;

    /// The derived superseded view: projections excluded from the
    /// actionable set by causal supersession, each with its
    /// `superseded_by` operation. Query-exposed history — reconciling
    /// or receipting these is not possible and not needed.
    fn superseded_applications(&self) -> Result<Vec<SupersededApplication>, StateError>;

    /// Derived unresolved-Signal set, per recipient or global.
    fn unresolved_signals(&self, recipient: Option<&ActorId>) -> Result<Vec<Signal>, StateError>;

    /// Complete typed audit lineage under AND-composed filters, in Ledger
    /// order. Replay and outer validation errors never add events.
    fn audit_events(&self, query: &AuditQuery) -> Result<Vec<AuditEvent>, StateError>;
}

/// Authenticated decision/composition view. Worker mutations are deliberately
/// absent: a composer holding this trait cannot accidentally reuse the worker
/// RPC surface as a decision path (ADR-0003 revision 6).
pub trait DecisionWorkflowStatePort {
    fn open_assignment(&self, opening: &AssignmentOpening) -> Result<StateApplied, StateError>;
    fn append_attempt(&self, opening: &AttemptOpening) -> Result<StateApplied, StateError>;
    fn record_decision(&self, record: &DecisionRecord) -> Result<StateApplied, StateError>;
    fn activate_profile(&self, opening: &ActivationOpening) -> Result<StateApplied, StateError>;
    fn deactivate_profile(
        &self,
        operation: &OperationId,
        actor: &ActorId,
        profile: &ProfileName,
    ) -> Result<StateApplied, StateError>;
    fn append_signal(&self, draft: &SignalDraft) -> Result<(Signal, StateApplied), StateError>;
    fn persist_envelope(
        &self,
        operation: &OperationId,
        subject: &LaunchSubject,
        envelope: &EnvelopeSnapshot,
    ) -> Result<StateApplied, StateError>;
    fn envelope(&self, subject: &LaunchSubject) -> Result<EnvelopeSnapshot, StateError>;
    fn bind_runtime_handle(
        &self,
        operation: &OperationId,
        subject: &LaunchSubject,
        handle: &RuntimeHandle,
    ) -> Result<StateApplied, StateError>;
    fn unbind_runtime_handle(
        &self,
        operation: &OperationId,
        subject: &LaunchSubject,
    ) -> Result<StateApplied, StateError>;
    fn runtime_handle(&self, subject: &LaunchSubject) -> Result<Option<RuntimeHandle>, StateError>;
    fn record_runtime_observation(
        &self,
        operation: &OperationId,
        record: &RuntimeObservationRecord,
    ) -> Result<StateApplied, StateError>;
    fn runtime_observation(
        &self,
        operation: &OperationId,
    ) -> Result<RuntimeObservationRecord, StateError>;
    fn record_application_attempt(
        &self,
        attempt: &ApplicationAttempt,
    ) -> Result<StateApplied, StateError>;
    fn record_application_receipt(
        &self,
        receipt: &ApplicationReceipt,
    ) -> Result<StateApplied, StateError>;
    fn assignment(&self, id: &AssignmentId) -> Result<AssignmentView, StateError>;
    fn evidence_for(&self, attempt: &AttemptId) -> Result<Vec<EvidenceRecord>, StateError>;
    fn signals_for(&self, attempt: &AttemptId) -> Result<Vec<Signal>, StateError>;
    fn handoff(&self, id: &HandoffId) -> Result<HandoffRecord, StateError>;
    fn decision(&self, operation: &OperationId) -> Result<DecisionRecord, StateError>;
    fn active_occupants(&self, profile: &ProfileName) -> Result<Vec<ActorId>, StateError>;
    fn pending_applications(&self) -> Result<Vec<PendingApplication>, StateError>;
    fn superseded_applications(&self) -> Result<Vec<SupersededApplication>, StateError>;
    fn unresolved_signals(&self, recipient: Option<&ActorId>) -> Result<Vec<Signal>, StateError>;
    fn audit_events(&self, query: &AuditQuery) -> Result<Vec<AuditEvent>, StateError>;
}

impl<T> DecisionWorkflowStatePort for T
where
    T: WorkflowStatePort + ?Sized,
{
    fn open_assignment(&self, opening: &AssignmentOpening) -> Result<StateApplied, StateError> {
        WorkflowStatePort::open_assignment(self, opening)
    }

    fn append_attempt(&self, opening: &AttemptOpening) -> Result<StateApplied, StateError> {
        WorkflowStatePort::append_attempt(self, opening)
    }

    fn record_decision(&self, record: &DecisionRecord) -> Result<StateApplied, StateError> {
        WorkflowStatePort::record_decision(self, record)
    }

    fn activate_profile(&self, opening: &ActivationOpening) -> Result<StateApplied, StateError> {
        WorkflowStatePort::activate_profile(self, opening)
    }

    fn deactivate_profile(
        &self,
        operation: &OperationId,
        actor: &ActorId,
        profile: &ProfileName,
    ) -> Result<StateApplied, StateError> {
        WorkflowStatePort::deactivate_profile(self, operation, actor, profile)
    }

    fn append_signal(&self, draft: &SignalDraft) -> Result<(Signal, StateApplied), StateError> {
        WorkflowStatePort::append_signal(self, draft)
    }

    fn persist_envelope(
        &self,
        operation: &OperationId,
        subject: &LaunchSubject,
        envelope: &EnvelopeSnapshot,
    ) -> Result<StateApplied, StateError> {
        WorkflowStatePort::persist_envelope(self, operation, subject, envelope)
    }

    fn envelope(&self, subject: &LaunchSubject) -> Result<EnvelopeSnapshot, StateError> {
        WorkflowStatePort::envelope(self, subject)
    }

    fn bind_runtime_handle(
        &self,
        operation: &OperationId,
        subject: &LaunchSubject,
        handle: &RuntimeHandle,
    ) -> Result<StateApplied, StateError> {
        WorkflowStatePort::bind_runtime_handle(self, operation, subject, handle)
    }

    fn unbind_runtime_handle(
        &self,
        operation: &OperationId,
        subject: &LaunchSubject,
    ) -> Result<StateApplied, StateError> {
        WorkflowStatePort::unbind_runtime_handle(self, operation, subject)
    }

    fn runtime_handle(&self, subject: &LaunchSubject) -> Result<Option<RuntimeHandle>, StateError> {
        WorkflowStatePort::runtime_handle(self, subject)
    }

    fn record_runtime_observation(
        &self,
        operation: &OperationId,
        record: &RuntimeObservationRecord,
    ) -> Result<StateApplied, StateError> {
        WorkflowStatePort::record_runtime_observation(self, operation, record)
    }

    fn runtime_observation(
        &self,
        operation: &OperationId,
    ) -> Result<RuntimeObservationRecord, StateError> {
        WorkflowStatePort::runtime_observation(self, operation)
    }

    fn record_application_attempt(
        &self,
        attempt: &ApplicationAttempt,
    ) -> Result<StateApplied, StateError> {
        WorkflowStatePort::record_application_attempt(self, attempt)
    }

    fn record_application_receipt(
        &self,
        receipt: &ApplicationReceipt,
    ) -> Result<StateApplied, StateError> {
        WorkflowStatePort::record_application_receipt(self, receipt)
    }

    fn assignment(&self, id: &AssignmentId) -> Result<AssignmentView, StateError> {
        WorkflowStatePort::assignment(self, id)
    }

    fn evidence_for(&self, attempt: &AttemptId) -> Result<Vec<EvidenceRecord>, StateError> {
        WorkflowStatePort::evidence_for(self, attempt)
    }

    fn signals_for(&self, attempt: &AttemptId) -> Result<Vec<Signal>, StateError> {
        WorkflowStatePort::signals_for(self, attempt)
    }

    fn handoff(&self, id: &HandoffId) -> Result<HandoffRecord, StateError> {
        WorkflowStatePort::handoff(self, id)
    }

    fn decision(&self, operation: &OperationId) -> Result<DecisionRecord, StateError> {
        WorkflowStatePort::decision(self, operation)
    }

    fn active_occupants(&self, profile: &ProfileName) -> Result<Vec<ActorId>, StateError> {
        WorkflowStatePort::active_occupants(self, profile)
    }

    fn pending_applications(&self) -> Result<Vec<PendingApplication>, StateError> {
        WorkflowStatePort::pending_applications(self)
    }

    fn superseded_applications(&self) -> Result<Vec<SupersededApplication>, StateError> {
        WorkflowStatePort::superseded_applications(self)
    }

    fn unresolved_signals(&self, recipient: Option<&ActorId>) -> Result<Vec<Signal>, StateError> {
        WorkflowStatePort::unresolved_signals(self, recipient)
    }

    fn audit_events(&self, query: &AuditQuery) -> Result<Vec<AuditEvent>, StateError> {
        WorkflowStatePort::audit_events(self, query)
    }
}

impl<T> WorkerWorkflowStatePort for T
where
    T: WorkflowStatePort + ?Sized,
{
    fn fenced_report(
        &self,
        action: &FencedAction,
        draft: &ReportDraft,
    ) -> Result<(ReportOutcome, FencedResponse), StateError> {
        WorkflowStatePort::fenced_report(self, action, draft)
    }

    fn fenced_evidence(
        &self,
        action: &FencedAction,
        evidence: &Evidence,
    ) -> Result<(EvidenceOutcome, FencedResponse), StateError> {
        WorkflowStatePort::fenced_evidence(self, action, evidence)
    }

    fn fenced_submit_handoff(
        &self,
        action: &FencedAction,
        handoff: &HandoffRecord,
    ) -> Result<(SubmissionOutcome, FencedResponse), StateError> {
        WorkflowStatePort::fenced_submit_handoff(self, action, handoff)
    }

    fn fenced_abort_attempt(&self, call: &FencedCall) -> Result<FencedResponse, StateError> {
        WorkflowStatePort::fenced_abort_attempt(self, call)
    }

    fn renew_lease(
        &self,
        call: &FencedCall,
        until: Timestamp,
    ) -> Result<(Lease, FencedResponse), StateError> {
        WorkflowStatePort::renew_lease(self, call, until)
    }
}

// ---------------------------------------------------------------------------
// Runtime
// ---------------------------------------------------------------------------

/// Opaque, generation-fenced runtime handle (core invariant 8).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct RuntimeHandle(String);

impl RuntimeHandle {
    pub fn new(token: impl Into<String>) -> Self {
        Self(token.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Absolute host path (worktrees are host paths, not `WorkPath`s).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostPath(String);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum HostPathError {
    NotAbsolute,
    InvalidCharacter,
}

impl HostPath {
    pub fn new(raw: &str) -> Result<Self, HostPathError> {
        if !raw.starts_with('/') || raw.len() < 2 {
            return Err(HostPathError::NotAbsolute);
        }
        if raw.bytes().any(|b| b.is_ascii_control()) {
            return Err(HostPathError::InvalidCharacter);
        }
        Ok(Self(raw.to_owned()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Explicit launch specification: nothing ambient; the Envelope is the
/// exact snapshot already persisted through Scribe.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaunchSpec {
    /// What is being launched (worker Attempt or actor activation).
    /// The adapter refuses material bound to a different subject
    /// before any provider mutation. Trusted internal field —
    /// `LaunchSpec` never crosses the approval boundary.
    pub subject: LaunchSubject,
    /// Recovery key, known before launch (R5.11).
    pub correlation: LaunchCorrelation,
    pub agent_kind: String,
    pub executable: String,
    pub args: Vec<String>,
    pub working_directory: HostPath,
    pub environment: BTreeMap<String, String>,
    pub envelope: EnvelopeSnapshot,
    pub startup_deadline: Timestamp,
    pub delivery_deadline: Timestamp,
}

/// Non-authoritative liveness observation (CONTEXT §5).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LivenessObservation {
    pub observed_at: Timestamp,
    pub kind: LivenessKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LivenessKind {
    Starting,
    Running,
    Idle,
    Blocked,
    Exited,
    NotFound,
    Unavailable,
    Unknown,
    /// Stale until explicit re-association; normal after a successful
    /// provider live-handoff, not only crash recovery.
    StaleGeneration,
}

/// Closed outcome of startup Envelope delivery. A definite failure keeps the handle so the caller can
/// stop and reconcile the created session: an outer `Err` means no
/// usable handle exists at all.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StartupDelivery {
    /// One provider API submission accepted (text + delayed Enter
    /// scheduled) — submission, never proof of application or read.
    Submitted,
    /// Definitely not delivered, with the normalized reason; the
    /// session exists and must be stopped/reconciled.
    NotDelivered(RuntimeError),
    /// Unknown: stop and reconcile the possibly-live session; never
    /// manufacture successful delivery.
    Ambiguous,
}

/// The closed set of things ABACUS can launch and durably associate
/// (R5.19). **This is runtime association identity, not a workflow
/// subject:** ADR-0002's `SubjectRef` family (Bead, Assignment,
/// Attempt, scope) is unchanged and gains no fifth variant. Workers run an Assignment Attempt; orchestrators and
/// watchdogs are spawned profiles running an actor activation
/// (CONTEXT I12/I16).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum LaunchSubject {
    WorkerAttempt {
        attempt: AttemptId,
    },
    /// A spawned orchestrator/watchdog profile: the actor, its profile,
    /// and the activation generation (the activation's operation identity).
    ActorActivation {
        actor: ActorId,
        profile: ProfileName,
        generation: OperationId,
    },
}

/// Bounded recovery key, established BEFORE launch and carried in the
/// trusted internal `LaunchSpec`, so an ambiguous launch is
/// recoverable even when the launch response itself is lost (R5.11).
/// It maps deterministically to the provider namespace.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct LaunchCorrelation(String);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LaunchCorrelationError {
    Empty,
    TooLong,
    InvalidCharacter,
}

impl LaunchCorrelation {
    /// 1..=64 lowercase alphanumerics and hyphens.
    pub fn new(raw: &str) -> Result<Self, LaunchCorrelationError> {
        if raw.is_empty() {
            return Err(LaunchCorrelationError::Empty);
        }
        if raw.len() > 64 {
            return Err(LaunchCorrelationError::TooLong);
        }
        if !raw
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        {
            return Err(LaunchCorrelationError::InvalidCharacter);
        }
        Ok(Self(raw.to_owned()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Outcome of a launch attempt. `Ambiguous` is the honest value for
/// "the provider may have created a session but no generation handle
/// is known" — distinct from `RuntimeError::Timeout`, which is a
/// definite pre-submission failure. Recovery is
/// [`RuntimePort::recover_launch`], never a retry of launch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LaunchAttempt {
    Launched(LaunchOutcome),
    /// Echoes the correlation the caller already holds (it was in the
    /// `LaunchSpec`), so recovery never depends on reading this value.
    Ambiguous {
        subject: LaunchSubject,
        correlation: LaunchCorrelation,
    },
}

/// Normalized launch facts returned with the handle.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaunchOutcome {
    pub handle: RuntimeHandle,
    pub observation: LivenessObservation,
    /// Covers Envelope delivery — submission is not application or read;
    /// see [`StartupDelivery`].
    pub startup_delivery: StartupDelivery,
}

/// Best-effort delivery report (I6): informational only; `Ambiguous`
/// is never permission to retry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DeliveryReport {
    Submitted,
    Ambiguous,
}

/// Outcome of a mutating control/stop operation (S6, HPG.3 doctrine):
/// a timeout after submission is an ambiguous effect, distinct from
/// definite failure, and never triggers a blind retry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EffectReport {
    Applied,
    Ambiguous,
}

/// Closed bounded input controls: prompt submission may never fall
/// back to raw keystrokes (provider-lock property), so the only
/// supported input beyond prompts is cancelling a blocked dialog.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ControlAction {
    CancelBlockedDialog,
}

/// Normalized runtime-seam failures.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RuntimeError {
    ProviderUnavailable,
    VersionMismatch,
    /// Host approval absent: fail closed, no fallback path exists.
    NotPermitted,
    NotFound,
    HandleStale,
    Rejected,
    /// Definite pre-submission timeout: the effect did not happen.
    Timeout,
    MalformedOutput,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum StopMode {
    Graceful,
    Forced,
}

/// Agent/session lifecycle mechanics. Every call is bounded by an
/// explicit deadline; every mutating verb fences stale generations.
pub trait RuntimePort {
    /// Launch with the persisted Envelope and injected, non-secret
    /// launch configuration inside `spec`.
    fn launch(&self, spec: &LaunchSpec) -> Result<LaunchAttempt, RuntimeError>;

    /// Look up / re-associate a possibly-created session after an
    /// ambiguous launch. Never re-launches: it resolves to the existing
    /// session's handle or reports that none exists.
    ///
    /// Implementations MUST validate the `(subject, correlation)` pair
    /// together — a correlation alone must never rebind a session to a
    /// different workflow identity (R5.17). The recovered
    /// `startup_delivery` MUST be `Ambiguous` unless the adapter holds
    /// explicit durable or provider-supplied proof that the startup
    /// submission occurred; the provider offers no operation-identity
    /// receipt for it after a lost response, so a recovered `Submitted`
    /// would be manufactured. `Ambiguous` still returns a handle, which
    /// is what lets the caller stop the session and revoke.
    fn recover_launch(
        &self,
        subject: &LaunchSubject,
        correlation: &LaunchCorrelation,
        deadline: Timestamp,
    ) -> Result<Option<LaunchOutcome>, RuntimeError>;

    fn observe(
        &self,
        handle: &RuntimeHandle,
        deadline: Timestamp,
    ) -> Result<LivenessObservation, RuntimeError>;

    fn wait(
        &self,
        handle: &RuntimeHandle,
        desired: LivenessKind,
        deadline: Timestamp,
    ) -> Result<LivenessObservation, RuntimeError>;

    /// Bounded text/detection view for diagnosis; never authority.
    fn read_view(
        &self,
        handle: &RuntimeHandle,
        max_bytes: u32,
        deadline: Timestamp,
    ) -> Result<String, RuntimeError>;

    /// Content-free doorbell; the durable Signal is already committed.
    fn doorbell(
        &self,
        handle: &RuntimeHandle,
        deadline: Timestamp,
    ) -> Result<DeliveryReport, RuntimeError>;

    /// Bounded transient live prompt (conversation, never authority).
    fn prompt(
        &self,
        handle: &RuntimeHandle,
        text: &str,
        deadline: Timestamp,
    ) -> Result<DeliveryReport, RuntimeError>;

    /// Bounded closed input control; ambiguity is reported, never
    /// retried blind.
    fn control(
        &self,
        handle: &RuntimeHandle,
        action: ControlAction,
        deadline: Timestamp,
    ) -> Result<EffectReport, RuntimeError>;

    fn stop(
        &self,
        handle: &RuntimeHandle,
        mode: StopMode,
        deadline: Timestamp,
    ) -> Result<EffectReport, RuntimeError>;

    /// Explicit re-association after restart/live-handoff.
    fn reassociate(
        &self,
        stale: &RuntimeHandle,
        deadline: Timestamp,
    ) -> Result<RuntimeHandle, RuntimeError>;
}

// ---------------------------------------------------------------------------
// Commit verification
// ---------------------------------------------------------------------------

/// Ancestry relation (a diff proves nothing about ancestry).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BaseRelation {
    BasedOn,
    NotBasedOn,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum VerifyError {
    UnknownCommit,
    Unavailable,
}

/// Canonical possibly-empty dirt set: duplicate-free, sorted, bounded.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DirtSet(Vec<WorkPath>);

impl DirtSet {
    pub fn new(mut paths: Vec<WorkPath>) -> Result<Self, crate::evidence::CollectionError> {
        if paths.len() > 256 {
            return Err(crate::evidence::CollectionError::TooMany);
        }
        paths.sort();
        for pair in paths.windows(2) {
            if pair[0] == pair[1] {
                return Err(crate::evidence::CollectionError::Duplicate(
                    pair[0].as_str().to_owned(),
                ));
            }
        }
        Ok(Self(paths))
    }

    pub fn iter(&self) -> impl Iterator<Item = &WorkPath> {
        self.0.iter()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

/// A bounded snapshot of one explicit worktree: the clean-tree proof
/// the submit use case produces and acceptance verifies.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorktreeView {
    pub head: CommitId,
    pub workspace_digest: WorkspaceDigest,
    pub dirt: DirtSet,
}

/// Commit facts for evidence validation. `Unavailable` is never
/// collapsed into absence.
pub trait CommitVerifierPort {
    fn exists(&self, commit: &CommitId) -> Result<(), VerifyError>;

    fn relation(&self, base: &CommitId, commit: &CommitId) -> Result<BaseRelation, VerifyError>;

    fn changed_paths(
        &self,
        base: &CommitId,
        commit: &CommitId,
    ) -> Result<Vec<WorkPath>, VerifyError>;

    fn file_digest(
        &self,
        commit: &CommitId,
        path: &WorkPath,
    ) -> Result<Option<ContentHash>, VerifyError>;

    /// Bounded clean/dirt snapshot of an explicit injected worktree.
    fn worktree_view(&self, worktree: &HostPath) -> Result<WorktreeView, VerifyError>;
}

// ---------------------------------------------------------------------------
// Clock and identifiers
// ---------------------------------------------------------------------------

/// Time as an input (I13).
pub trait ClockPort {
    fn now(&self) -> Timestamp;
}

/// Identifier generation as an input (core invariant 9).
pub trait IdGeneratorPort {
    fn generate(&self) -> String;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::SignalSender;
    use crate::authority::AuthorityClass;
    use crate::evidence::{Argv, FileDigestSet, VerificationOutcome, VerificationSet};
    use crate::id::CapabilityId;
    use crate::profile::{
        CapabilityDescriptor, CheckClass, Grant, OccupancyClass, ProfileSpec, ValidatedProfileSet,
        validate_profiles,
    };
    use crate::scope::{ScopeExpr, ScopeKey, ScopeValue};
    use crate::signal::{
        BoundedText, DirectiveKind, ReportKind, ResponseAction, ResponseKind, SignalBody,
        SubjectRef, binding_directives, handoff_gate, worker_append_gate,
    };
    use std::cell::RefCell;

    fn rev(fill: char) -> WorkRevision {
        WorkRevision(ContentHash::new(&fill.to_string().repeat(64)).unwrap())
    }

    fn op(raw: &str) -> OperationId {
        OperationId::new(raw).unwrap()
    }

    fn snapshot(id: &str, fill: char, priority: u8) -> BeadSnapshot {
        BeadSnapshot {
            id: BeadId::new(id).unwrap(),
            content_hash: ContentHash::new(&fill.to_string().repeat(64)).unwrap(),
            scope_map: ScopeMap::new(vec![(
                ScopeKey::new("area").unwrap(),
                ScopeValue::new("core").unwrap(),
            )])
            .unwrap(),
            priority: Priority::new(priority).unwrap(),
        }
    }

    struct FakeWork {
        revision: WorkRevision,
        beads: Vec<BeadSnapshot>,
        effect_present_for: Vec<BeadId>,
    }

    impl WorkGraphPort for FakeWork {
        fn ready(&self) -> Result<(WorkRevision, Vec<BeadSnapshot>), WorkError> {
            Ok((self.revision.clone(), self.beads.clone()))
        }

        fn inspect(&self, id: &BeadId) -> Result<BeadStatusView, WorkError> {
            let snapshot = self
                .beads
                .iter()
                .find(|b| &b.id == id)
                .cloned()
                .ok_or(WorkError::NotFound)?;
            let status = if self.effect_present_for.contains(id) {
                WorkStatus::Closed {
                    observed_reason: ObservedCloseReason::AcceptedHandoff,
                }
            } else {
                WorkStatus::Open
            };
            Ok(BeadStatusView {
                snapshot,
                status,
                eligibility: Eligibility::Eligible,
                revision: self.revision.clone(),
            })
        }

        fn mark_in_progress(
            &self,
            id: &BeadId,
            operation: &OperationId,
            expected: &WorkRevision,
        ) -> Result<MutationOutcome, WorkError> {
            self.close(id, CloseReason::AcceptedHandoff, operation, expected)
        }

        fn close(
            &self,
            id: &BeadId,
            _reason: CloseReason,
            _operation: &OperationId,
            expected: &WorkRevision,
        ) -> Result<MutationOutcome, WorkError> {
            let view = self.inspect(id)?;
            if let WorkStatus::Closed { .. } = view.status {
                return Ok(MutationOutcome::FoundBeforeSubmission {
                    status: view.status,
                    revision: view.revision,
                });
            }
            if *expected != self.revision {
                return Err(WorkError::RevisionConflict);
            }
            Ok(MutationOutcome::Applied {
                before: self.revision.clone(),
                after: rev('f'),
                summary: "closed".into(),
            })
        }
    }

    #[test]
    fn work_mutations_expose_normalized_reconciliation_facts() {
        let closed_id = BeadId::new("ABACUS-x").unwrap();
        let port = FakeWork {
            revision: rev('a'),
            beads: vec![snapshot("ABACUS-x", 'b', 2)],
            effect_present_for: vec![closed_id.clone()],
        };
        assert_eq!(
            port.close(
                &closed_id,
                CloseReason::AcceptedHandoff,
                &op("op-1"),
                &rev('9')
            ),
            Ok(MutationOutcome::FoundBeforeSubmission {
                status: WorkStatus::Closed {
                    observed_reason: ObservedCloseReason::AcceptedHandoff
                },
                revision: rev('a'),
            })
        );
    }

    #[test]
    fn fallback_orders_by_priority_then_stable_id() {
        let ready = vec![
            snapshot("ABACUS-c", 'b', 2),
            snapshot("ABACUS-a", 'c', 2),
            snapshot("ABACUS-z", 'd', 0),
        ];
        assert_eq!(
            fallback_order(&ready)
                .iter()
                .map(|b| b.as_str())
                .collect::<Vec<_>>(),
            vec!["ABACUS-z", "ABACUS-a", "ABACUS-c"]
        );
        assert!(Priority::new(5).is_err());
    }

    #[test]
    fn advice_gate_and_degradation_reasons() {
        let ready = vec![snapshot("ABACUS-b", 'b', 1), snapshot("ABACUS-a", 'c', 2)];
        let current = rev('a');
        let fallback = fallback_order(&ready);
        assert_eq!(fallback.first().unwrap().as_str(), "ABACUS-b");

        let good = AdviceOutcome::Advice {
            order: vec![
                BeadId::new("ABACUS-a").unwrap(),
                BeadId::new("ABACUS-b").unwrap(),
            ],
            bound_to: current.clone(),
        };
        assert_eq!(
            apply_advice(&ready, &good, &current)
                .iter()
                .map(|b| b.as_str())
                .collect::<Vec<_>>(),
            vec!["ABACUS-a", "ABACUS-b"]
        );

        for bad in [
            AdviceOutcome::Advice {
                order: vec![
                    BeadId::new("ABACUS-zzz").unwrap(),
                    BeadId::new("ABACUS-a").unwrap(),
                ],
                bound_to: current.clone(),
            },
            AdviceOutcome::Advice {
                order: vec![
                    BeadId::new("ABACUS-a").unwrap(),
                    BeadId::new("ABACUS-a").unwrap(),
                ],
                bound_to: current.clone(),
            },
            AdviceOutcome::Advice {
                order: vec![BeadId::new("ABACUS-a").unwrap()],
                bound_to: current.clone(),
            },
            AdviceOutcome::Advice {
                order: vec![
                    BeadId::new("ABACUS-a").unwrap(),
                    BeadId::new("ABACUS-b").unwrap(),
                ],
                bound_to: rev('9'),
            },
        ] {
            assert_eq!(apply_advice(&ready, &bad, &current), fallback);
        }

        for reason in [
            AdviceDegradation::Unavailable,
            AdviceDegradation::Timeout,
            AdviceDegradation::Partial,
            AdviceDegradation::Incompatible,
            AdviceDegradation::Malformed,
        ] {
            let outcome = AdviceOutcome::Degraded { reason };
            assert_eq!(apply_advice(&ready, &outcome, &current), fallback);
        }
    }

    fn actor() -> DecisionActor {
        DecisionActor {
            actor: ActorId::new("lead-1").unwrap(),
            class: AuthorityClass::Orchestrator,
            profile: ProfileName::new("lead").unwrap(),
            profile_hash: ContentHash::new(&"a".repeat(64)).unwrap(),
        }
    }

    fn reason(text: &str) -> DecisionReason {
        DecisionReason::new(text).unwrap()
    }

    fn authority(capability: &str) -> AuthoritySnapshot {
        AuthoritySnapshot {
            actor: actor(),
            capability: CapabilityId::new(capability).unwrap(),
            scope: ScopeExpr::Universal,
        }
    }

    fn worker_snapshot() -> DecisionActor {
        DecisionActor {
            actor: ActorId::new("worker-1").unwrap(),
            class: AuthorityClass::Worker,
            profile: ProfileName::new("worker").unwrap(),
            profile_hash: ContentHash::new(&"a".repeat(64)).unwrap(),
        }
    }

    fn opening() -> AssignmentOpening {
        let assignment_id = AssignmentId::new("asg-1").unwrap();
        let attempt_id = AttemptId::new("att-1").unwrap();
        AssignmentOpening {
            assignment: AssignmentRecord {
                id: assignment_id.clone(),
                bead: BeadId::new("ABACUS-x").unwrap(),
                bead_content_hash: ContentHash::new(&"b".repeat(64)).unwrap(),
                scope_map: ScopeMap::default(),
                worker: worker_snapshot(),
                decision_actor: actor(),
                edit_scope: EditScope::new(vec![WorkPath::new("src").unwrap()]).unwrap(),
                acceptance: AcceptancePolicy {
                    verification: VerificationSet::new(
                        vec![Argv::new(vec!["cargo".into(), "test".into()]).unwrap()],
                        PathSet::new(vec![WorkPath::new("tests/a.rs").unwrap()]).unwrap(),
                    )
                    .unwrap(),
                    form: crate::evidence::PolicyForm::Standard,
                },
                attempt_policy: AttemptPolicy::default(),
                declared_base: CommitId::new(&"c".repeat(40)).unwrap(),
            },
            first_attempt: AttemptRecord {
                id: attempt_id.clone(),
                assignment: assignment_id.clone(),
                lease: Lease {
                    token: FencingToken(1),
                    expires_at: Timestamp(100),
                },
            },
            authorizing: AssignDecision {
                operation: op("op-assign"),
                assignment: assignment_id,
                first_attempt: attempt_id,
                authority: authority("state:assign"),
            },
            bead_revision: rev('a'),
        }
    }

    /// In-memory state fake proving the transactional seam shape.
    struct FakeState {
        committed: RefCell<BTreeMap<String, String>>,
        current_token: RefCell<FencingToken>,
        bound_worker_snapshot: DecisionActor,
        /// Lease expiry plus the caller-supplied "now" the fake
        /// evaluates against, so LeaseExpired is reachable.
        lease_expires_at: RefCell<Timestamp>,
        now: RefCell<Timestamp>,
        next_seq: RefCell<u64>,
        stored_signals: RefCell<Vec<Signal>>,
        evidence_records: RefCell<Vec<EvidenceRecord>>,
        response_actions: RefCell<Vec<ResponseAction>>,
        reports: RefCell<BTreeMap<String, ReportOutcome>>,
        evidence_outcomes: RefCell<BTreeMap<String, EvidenceOutcome>>,
        submissions: RefCell<BTreeMap<String, (String, SubmissionOutcome)>>,
        receipts: RefCell<Vec<String>>,
        decisions: RefCell<Vec<DecisionRecord>>,
        /// Attempt locator → owning Assignment. Worker authority is
        /// resolved from this durable relation rather than asserted by
        /// the caller.
        attempt_owners: RefCell<BTreeMap<String, String>>,
        /// Launch-subject key → operation that authorized the launch.
        launch_authorizers: RefCell<BTreeMap<String, OperationId>>,
        /// `actor:profile` → current activation generation.
        active_activation_generations: RefCell<BTreeMap<String, OperationId>>,
        /// Subject key → persisted Envelope (immutable once written).
        envelopes: RefCell<BTreeMap<String, EnvelopeSnapshot>>,
        /// Association key → currently bound runtime handle.
        handles: RefCell<BTreeMap<String, RuntimeHandle>>,
        /// Verb-scoped idempotency ledger: `verb:operation` →
        /// (request, stored result).
        operations: RefCell<BTreeMap<String, (String, StateApplied)>>,
        /// Typed durable record key (`signal:<id>` / `handoff:<id>`) →
        /// the call operation that created it, so a different operation
        /// cannot claim an existing record. Keys are kind-scoped
        /// because SignalId and HandoffId are distinct opaque types
        /// with no shared namespace (R5.29).
        record_owners: RefCell<BTreeMap<String, String>>,
        /// Committed projections awaiting application: operation → pending.
        projections: RefCell<BTreeMap<String, PendingApplication>>,
        /// Typed Assignment records, so projections derive their bead
        /// from real state rather than parsed Debug output.
        assignments: RefCell<BTreeMap<String, AssignmentRecord>>,
        /// Application attempts recorded per target operation.
        application_attempts: RefCell<BTreeMap<String, Vec<(OperationId, ApplicationOutcome)>>>,
        /// Active (profile → actors) membership for EVERY occupancy
        /// class; singleton is an extra cardinality rule on top.
        active_members: RefCell<BTreeMap<String, std::collections::BTreeSet<String>>>,
        handoffs: RefCell<Vec<HandoffRecord>>,
        actor_classes: RefCell<BTreeMap<String, AuthorityClass>>,
        attempt_states: RefCell<BTreeMap<String, AttemptState>>,
        audit_events: RefCell<BTreeMap<u64, AuditEvent>>,
        runtime_observations: RefCell<BTreeMap<String, RuntimeObservationRecord>>,
    }

    impl FakeState {
        /// Full non-secret launch identity used by Envelope and runtime
        /// associations.
        fn association_key(subject: &LaunchSubject) -> String {
            match subject {
                LaunchSubject::WorkerAttempt { attempt } => {
                    format!("attempt:{}", attempt.as_str())
                }
                LaunchSubject::ActorActivation {
                    actor,
                    profile,
                    generation,
                } => format!(
                    "activation:{}:{}:{}",
                    actor.as_str(),
                    profile.as_str(),
                    generation.as_str()
                ),
            }
        }

        fn activation_key(actor: &ActorId, profile: &ProfileName) -> String {
            format!("{}:{}", actor.as_str(), profile.as_str())
        }

        /// **Verb-scoped** idempotency ledger for association mutators:
        /// each records its operation + full request + prior result in
        /// the same transaction. Deliberately NOT a global cross-verb
        /// namespace — the domain intentionally reuses a decision
        /// operation as the receipt/projection target, and forcing
        /// global uniqueness would break that saga.
        fn replay(
            &self,
            verb: &str,
            operation: &OperationId,
            request: &str,
        ) -> Result<Option<StateApplied>, StateError> {
            let scoped = format!("{verb}:{}", operation.as_str());
            match self.operations.borrow().get(&scoped) {
                None => Ok(None),
                Some((stored_request, _result)) => {
                    if stored_request == request {
                        // Identical operation already committed: no
                        // side effect is reapplied.
                        Ok(Some(StateApplied::AlreadyApplied))
                    } else {
                        Err(StateError::ConflictingOperation)
                    }
                }
            }
        }

        fn remember(
            &self,
            verb: &str,
            operation: &OperationId,
            request: &str,
            result: StateApplied,
        ) {
            self.operations.borrow_mut().insert(
                format!("{verb}:{}", operation.as_str()),
                (request.to_owned(), result),
            );
        }

        fn apply(&self, key: String, content: String) -> Result<StateApplied, StateError> {
            let mut committed = self.committed.borrow_mut();
            match committed.get(&key) {
                Some(existing) if *existing == content => Ok(StateApplied::AlreadyApplied),
                Some(_) => Err(StateError::ConflictingOperation),
                None => {
                    committed.insert(key, content);
                    Ok(StateApplied::Applied)
                }
            }
        }

        fn fence(&self, call: &FencedCall) -> Result<(), StateError> {
            if !self
                .attempt_owners
                .borrow()
                .contains_key(call.attempt.as_str())
            {
                return Err(StateError::UnknownRecord);
            }
            if call.token != *self.current_token.borrow() {
                return Err(StateError::StaleFencing);
            }
            if self
                .attempt_states
                .borrow()
                .get(call.attempt.as_str())
                .is_some_and(|state| state.is_ended())
            {
                return Err(StateError::StaleFencing);
            }
            if *self.now.borrow() > *self.lease_expires_at.borrow() {
                return Err(StateError::LeaseExpired);
            }
            Ok(())
        }

        fn active_fence(&self, call: &FencedCall) -> Result<(), StateError> {
            self.fence(call)?;
            if self
                .attempt_states
                .borrow()
                .get(call.attempt.as_str())
                .is_none_or(|state| *state != AttemptState::Active)
            {
                Err(StateError::IncoherentBundle)
            } else {
                Ok(())
            }
        }

        fn commit_seq(&self) -> Seq {
            self.next_ledger_seq()
        }

        fn resolve_subject(&self, subject: &LaunchSubject) -> Result<(), StateError> {
            match subject {
                LaunchSubject::WorkerAttempt { attempt } => self
                    .launch_authorizers
                    .borrow()
                    .contains_key(&format!("attempt:{}", attempt.as_str()))
                    .then_some(())
                    .ok_or(StateError::UnknownRecord),
                LaunchSubject::ActorActivation {
                    actor,
                    profile,
                    generation,
                } => match self
                    .active_activation_generations
                    .borrow()
                    .get(&Self::activation_key(actor, profile))
                {
                    None => Err(StateError::UnknownRecord),
                    Some(current) if current != generation => Err(StateError::StaleFencing),
                    Some(_) => Ok(()),
                },
            }
        }

        /// Caller-supplied fenced identity: Attempt locator and token.
        /// Authority is deliberately absent and resolved server-side.
        fn call_identity(call: &FencedCall) -> String {
            format!("att={}|tok={}", call.attempt.as_str(), call.token.0)
        }

        /// FULL substantive-action identity: the response link is an
        /// input fact, so changing it under the same operation is a
        /// conflicting duplicate rather than a replay.
        fn action_identity(action: &FencedAction) -> String {
            format!(
                "{}|responds_to={}",
                Self::call_identity(&action.call),
                action.responds_to.as_ref().map_or("-", SignalId::as_str)
            )
        }

        fn next_ledger_seq(&self) -> Seq {
            let mut next = self.next_seq.borrow_mut();
            *next += 1;
            Seq(*next)
        }

        fn current_head(&self) -> Seq {
            Seq(*self.next_seq.borrow())
        }

        fn append_audit(
            &self,
            seq: Seq,
            initiator: AuditInitiator,
            operation: AuditOperation,
            subject: AuditSubject,
            kind: AuditKind,
        ) {
            let prior = self.audit_events.borrow_mut().insert(
                seq.0,
                AuditEvent {
                    seq,
                    at: *self.now.borrow(),
                    initiator,
                    operation,
                    subject,
                    kind,
                },
            );
            assert!(prior.is_none(), "one audit event per Ledger position");
        }

        fn worker_initiator(&self, call: &FencedCall) -> AuditInitiator {
            let assignment = self
                .attempt_owners
                .borrow()
                .get(call.attempt.as_str())
                .and_then(|raw| AssignmentId::new(raw).ok())
                .unwrap_or_else(|| AssignmentId::new("asg-1").expect("valid fallback"));
            let actor = self
                .assignments
                .borrow()
                .get(assignment.as_str())
                .map(|assignment| assignment.worker.clone())
                .unwrap_or_else(|| self.bound_worker_snapshot.clone());
            AuditInitiator::WorkerBinding {
                actor,
                assignment,
                attempt: call.attempt.clone(),
            }
        }

        fn launch_authorizing(&self, subject: &LaunchSubject) -> Result<OperationId, StateError> {
            let operation = self
                .launch_authorizers
                .borrow()
                .get(&Self::association_key(subject))
                .cloned()
                .ok_or(StateError::Corrupt)?;
            let committed = self.committed.borrow();
            if ["open", "attempt", "act"]
                .iter()
                .any(|verb| committed.contains_key(&format!("{verb}:{}", operation.as_str())))
            {
                Ok(operation)
            } else {
                Err(StateError::Corrupt)
            }
        }

        /// A link is accepted only when its target is already a
        /// committed Directive for this exact Attempt. Directive-kind
        /// policy deliberately remains in `directive_status`.
        fn validate_response_target(&self, action: &FencedAction) -> Result<(), StateError> {
            let Some(target) = &action.responds_to else {
                return Ok(());
            };
            let signals = self.stored_signals.borrow();
            let signal = signals
                .iter()
                .find(|signal| &signal.id == target)
                .ok_or(StateError::UnknownRecord)?;
            let owning_assignment = self
                .attempt_owners
                .borrow()
                .get(action.call.attempt.as_str())
                .cloned()
                .ok_or(StateError::UnknownRecord)?;
            match (&signal.subject, &signal.body) {
                (
                    SubjectRef::Attempt(subject),
                    SignalBody::Directive {
                        assignment,
                        attempt,
                        ..
                    },
                ) if assignment.as_str() == owning_assignment.as_str()
                    && subject == &action.call.attempt
                    && attempt == &action.call.attempt =>
                {
                    Ok(())
                }
                _ => Err(StateError::IncoherentBundle),
            }
        }

        fn response_action(action: &FencedAction, seq: Seq) -> ResponseAction {
            ResponseAction {
                seq,
                kind: ResponseKind::WorkerAction {
                    attempt: action.call.attempt.clone(),
                    responds_to: action.responds_to.clone(),
                },
            }
        }

        /// Commit one fenced call ordering position. Only permitted
        /// substantive actions enter the responding-action log; an
        /// audited domain refusal still advances the causal head but
        /// cannot discharge a Directive.
        fn commit_fenced_call(&self, action: Option<&FencedAction>, substantive: bool) -> Seq {
            let seq = self.next_ledger_seq();
            if substantive && let Some(action) = action {
                self.response_actions
                    .borrow_mut()
                    .push(Self::response_action(action, seq));
            }
            seq
        }

        fn respond(&self, attempt: &AttemptId, applied: StateApplied, head: Seq) -> FencedResponse {
            let signals = self.stored_signals.borrow();
            let actions = self.response_actions.borrow();
            FencedResponse {
                applied,
                binding_directives: binding_directives(attempt, &signals, &actions)
                    .into_iter()
                    .cloned()
                    .collect(),
                head,
            }
        }

        fn replay_fenced_response(
            &self,
            verb: &str,
            operation: &OperationId,
            request: &str,
            attempt: &AttemptId,
        ) -> Result<Option<FencedResponse>, StateError> {
            if self.replay(verb, operation, request)?.is_none() {
                return Ok(None);
            }
            // A replay allocates no new ordering position, but its
            // response still surfaces the causally current binding set
            // and Ledger head.
            Ok(Some(self.respond(
                attempt,
                StateApplied::AlreadyApplied,
                self.current_head(),
            )))
        }

        fn commit_signal(&self, draft: &SignalDraft) -> Result<(Signal, StateApplied), StateError> {
            if let Some(existing) = self
                .stored_signals
                .borrow()
                .iter()
                .find(|s| s.id == draft.id)
            {
                let SignalSender::Authority(sender) = &existing.sender else {
                    return Err(StateError::IncoherentBundle);
                };
                let redraft = SignalDraft {
                    id: existing.id.clone(),
                    sender: sender.clone(),
                    subject: existing.subject.clone(),
                    body: existing.body.clone(),
                };
                if redraft == *draft {
                    return Ok((existing.clone(), StateApplied::AlreadyApplied));
                }
                return Err(StateError::ConflictingOperation);
            }
            let committed = draft.clone().commit(self.next_ledger_seq());
            if let SignalBody::Directive { attempt, .. } = &committed.body {
                self.response_actions.borrow_mut().push(ResponseAction {
                    seq: committed.seq,
                    kind: ResponseKind::DirectiveCommitted {
                        attempt: attempt.clone(),
                        directive: committed.id.clone(),
                    },
                });
            }
            self.stored_signals.borrow_mut().push(committed.clone());
            Ok((committed, StateApplied::Applied))
        }
    }

    impl WorkflowStatePort for FakeState {
        fn open_assignment(&self, opening: &AssignmentOpening) -> Result<StateApplied, StateError> {
            // Bundle coherence BEFORE anything else: adjacent fields
            // must name the same Assignment and Attempt (R5.23).
            if opening.assignment.id != opening.authorizing.assignment
                || opening.first_attempt.assignment != opening.assignment.id
                || opening.first_attempt.id != opening.authorizing.first_attempt
            {
                return Err(StateError::IncoherentBundle);
            }
            let key = format!("open:{}", opening.authorizing.operation.as_str());
            let content = format!("{opening:?}");
            let worker = &opening.assignment.worker;
            // ---- validate; mutate nothing ----
            if let Some(existing) = self.actor_classes.borrow().get(worker.actor.as_str())
                && *existing != worker.class
            {
                return Err(StateError::ActorClassMismatch);
            }
            if let Some(existing) = self.committed.borrow().get(&key) {
                return if *existing == content {
                    Ok(StateApplied::AlreadyApplied)
                } else {
                    Err(StateError::ConflictingOperation)
                };
            }
            // ---- commit first-worker registration WITH the opening ----
            self.actor_classes
                .borrow_mut()
                .insert(worker.actor.as_str().to_owned(), worker.class);
            self.attempt_owners.borrow_mut().insert(
                opening.first_attempt.id.as_str().to_owned(),
                opening.assignment.id.as_str().to_owned(),
            );
            self.launch_authorizers.borrow_mut().insert(
                format!("attempt:{}", opening.first_attempt.id.as_str()),
                opening.authorizing.operation.clone(),
            );
            // First-worker registration IS an activation case, so it also
            // records active (profile, actor) membership; otherwise the
            // worker would be unqueryable and undeactivatable (R5.10
            // addendum).
            self.active_members
                .borrow_mut()
                .entry(format!("occupied:{}", worker.profile.as_str()))
                .or_default()
                .insert(worker.actor.as_str().to_owned());
            self.assignments.borrow_mut().insert(
                opening.assignment.id.as_str().to_owned(),
                opening.assignment.clone(),
            );
            self.attempt_states.borrow_mut().insert(
                opening.first_attempt.id.as_str().to_owned(),
                AttemptState::Active,
            );
            let seq = self.commit_seq();
            self.projections.borrow_mut().insert(
                opening.authorizing.operation.as_str().to_owned(),
                PendingApplication {
                    operation: opening.authorizing.operation.clone(),
                    assignment: opening.assignment.id.clone(),
                    bead: opening.assignment.bead.clone(),
                    projection: WorkProjection::MarkInProgress,
                    committed_at: seq,
                    authorized_revision: Some(opening.bead_revision.clone()),
                    receipt_candidate: None,
                },
            );
            self.append_audit(
                seq,
                AuditInitiator::Authority(opening.authorizing.authority.clone()),
                AuditOperation::Operation(opening.authorizing.operation.clone()),
                AuditSubject::Workflow(SubjectRef::Assignment(opening.assignment.id.clone())),
                AuditKind::AssignmentOpened,
            );
            self.committed.borrow_mut().insert(key, content);
            Ok(StateApplied::Applied)
        }

        fn append_attempt(&self, opening: &AttemptOpening) -> Result<StateApplied, StateError> {
            if opening.attempt.assignment != opening.authorizing.assignment {
                return Err(StateError::IncoherentBundle);
            }
            let key = format!("attempt:{}", opening.authorizing.operation.as_str());
            let content = format!("{opening:?}");
            // ---- validate; mutate nothing ----
            if let Some(existing) = self.committed.borrow().get(&key) {
                return if *existing == content {
                    Ok(StateApplied::AlreadyApplied)
                } else {
                    Err(StateError::ConflictingOperation)
                };
            }
            // ---- commit successor Attempt + its durable owner relation ----
            let assignment_id = opening.attempt.assignment.as_str().to_owned();
            if !self.assignments.borrow().contains_key(&assignment_id) {
                return Err(StateError::UnknownRecord);
            }
            self.attempt_owners
                .borrow_mut()
                .insert(opening.attempt.id.as_str().to_owned(), assignment_id);
            self.launch_authorizers.borrow_mut().insert(
                format!("attempt:{}", opening.attempt.id.as_str()),
                opening.authorizing.operation.clone(),
            );
            self.attempt_states
                .borrow_mut()
                .insert(opening.attempt.id.as_str().to_owned(), AttemptState::Active);
            let seq = self.next_ledger_seq();
            self.append_audit(
                seq,
                AuditInitiator::Authority(opening.authorizing.authority.clone()),
                AuditOperation::Operation(opening.authorizing.operation.clone()),
                AuditSubject::Workflow(SubjectRef::Attempt(opening.attempt.id.clone())),
                AuditKind::AttemptOpened,
            );
            self.committed.borrow_mut().insert(key, content);
            Ok(StateApplied::Applied)
        }

        fn record_decision(&self, record: &DecisionRecord) -> Result<StateApplied, StateError> {
            // Resolve the decided Handoff BEFORE recording anything: an
            // unknown or mismatched Handoff must refuse without committing a
            // decision or changing lifecycle state (R5.23).
            if let DecisionKind::Accept { handoff, .. } | DecisionKind::Reject { handoff, .. } =
                &record.kind
            {
                let handoffs = self.handoffs.borrow();
                let Some(found) = handoffs.iter().find(|h| &h.id == handoff) else {
                    return Err(StateError::UnknownRecord);
                };
                let owning = self
                    .attempt_owners
                    .borrow()
                    .get(found.attempt.as_str())
                    .cloned();
                if owning.as_deref() != Some(record.assignment.as_str()) {
                    return Err(StateError::IncoherentBundle);
                }
            }
            // Every decision target must exist and belong to this
            // Assignment before anything commits (R5.26).
            {
                let assignments = self.assignments.borrow();
                if !assignments.contains_key(record.assignment.as_str()) {
                    return Err(StateError::UnknownRecord);
                }
                let attempt_target = match &record.kind {
                    DecisionKind::Revoke { attempt, .. }
                    | DecisionKind::Reclaim { attempt, .. } => Some(attempt.clone()),
                    _ => None,
                };
                if let Some(attempt) = attempt_target {
                    let owning = self.attempt_owners.borrow().get(attempt.as_str()).cloned();
                    if owning.as_deref() != Some(record.assignment.as_str()) {
                        return Err(StateError::IncoherentBundle);
                    }
                }
            }
            let applied = self.apply(
                format!("dec:{}", record.operation.as_str()),
                format!("{record:?}"),
            )?;
            if applied == StateApplied::Applied {
                let seq = self.next_ledger_seq();
                let mut ended: Vec<String> = Vec::new();
                match &record.kind {
                    DecisionKind::Revoke { attempt, .. }
                    | DecisionKind::Reclaim { attempt, .. } => {
                        ended.push(attempt.as_str().to_owned());
                    }
                    DecisionKind::Accept { handoff, .. } | DecisionKind::Reject { handoff, .. } => {
                        if let Some(h) = self.handoffs.borrow().iter().find(|h| &h.id == handoff) {
                            ended.push(h.attempt.as_str().to_owned());
                        }
                    }
                    DecisionKind::Cancel { .. } => {
                        let assignment = record.assignment.as_str().to_owned();
                        ended.extend(
                            self.attempt_owners
                                .borrow()
                                .iter()
                                .filter(|(_, owner)| owner.as_str() == assignment.as_str())
                                .map(|(attempt, _)| attempt.clone())
                                .filter(|attempt| {
                                    self.attempt_states
                                        .borrow()
                                        .get(attempt)
                                        .is_some_and(|state| !state.is_ended())
                                }),
                        );
                    }
                    DecisionKind::TransferAuthority { .. } => {}
                }
                {
                    let mut states = self.attempt_states.borrow_mut();
                    for attempt in &ended {
                        let state = match &record.kind {
                            DecisionKind::Accept { .. } => AttemptState::Accepted,
                            DecisionKind::Reject { .. } => AttemptState::Rejected,
                            DecisionKind::Reclaim { .. } => AttemptState::Expired,
                            DecisionKind::Cancel { .. } | DecisionKind::Revoke { .. } => {
                                AttemptState::Revoked
                            }
                            DecisionKind::TransferAuthority { .. } => continue,
                        };
                        states.insert(attempt.clone(), state);
                    }
                }
                self.response_actions.borrow_mut().push(ResponseAction {
                    seq,
                    kind: ResponseKind::FencedDecision {
                        responds_to: record.resolves.clone(),
                    },
                });
                for attempt in &ended {
                    self.response_actions.borrow_mut().push(ResponseAction {
                        seq,
                        kind: ResponseKind::TerminalAttemptAction {
                            attempt: AttemptId::new(attempt)
                                .expect("stored AttemptId was validated"),
                            abort_consistent: true,
                        },
                    });
                }
                // Only Accept and Cancel project a close (R5.25).
                if let Some(reason) = record.kind.close_reason() {
                    // The bead comes from the typed Assignment record —
                    // never from parsed Debug output (R5.26).
                    let bead = self
                        .assignments
                        .borrow()
                        .get(record.assignment.as_str())
                        .map(|a| a.bead.clone())
                        .expect("assignment validated above");
                    self.projections.borrow_mut().insert(
                        record.operation.as_str().to_owned(),
                        PendingApplication {
                            operation: record.operation.clone(),
                            assignment: record.assignment.clone(),
                            bead,
                            projection: WorkProjection::Close { reason },
                            committed_at: seq,
                            // No revision is authorized at close time
                            // here; reconciliation inspects fresh.
                            authorized_revision: None,
                            receipt_candidate: None,
                        },
                    );
                }
                self.decisions.borrow_mut().push(record.clone());
                let subject = match &record.kind {
                    DecisionKind::Accept { .. } | DecisionKind::Reject { .. } => {
                        AuditSubject::Workflow(SubjectRef::Attempt(
                            AttemptId::new(
                                ended.first().expect("handoff decision ended its Attempt"),
                            )
                            .expect("stored AttemptId was validated"),
                        ))
                    }
                    DecisionKind::Revoke { attempt, .. }
                    | DecisionKind::Reclaim { attempt, .. } => {
                        AuditSubject::Workflow(SubjectRef::Attempt(attempt.clone()))
                    }
                    DecisionKind::Cancel { .. } | DecisionKind::TransferAuthority { .. } => {
                        AuditSubject::Workflow(SubjectRef::Assignment(record.assignment.clone()))
                    }
                };
                self.append_audit(
                    seq,
                    AuditInitiator::Authority(record.authority.clone()),
                    AuditOperation::Operation(record.operation.clone()),
                    subject,
                    AuditKind::decision(&record.kind),
                );
            }
            Ok(applied)
        }

        fn activate_profile(
            &self,
            opening: &ActivationOpening,
        ) -> Result<StateApplied, StateError> {
            let activation = &opening.activation;
            let key = format!("act:{}", activation.operation.as_str());
            let content = format!("{opening:?}");
            let occupied_key = format!("occupied:{}", activation.profile.as_str());

            // ---- validate everything; mutate nothing ----
            match &opening.case {
                ActivationCase::OperatorBootstrap => {
                    if activation.class() != AuthorityClass::Orchestrator {
                        return Err(StateError::ActivationCaseInvalid);
                    }
                }
                ActivationCase::ActorAuthorizedRotation { .. } => {
                    match self.actor_classes.borrow().get(activation.actor.as_str()) {
                        None => return Err(StateError::UnknownActor),
                        Some(existing) if *existing != activation.class() => {
                            return Err(StateError::ActorClassMismatch);
                        }
                        _ => {}
                    }
                }
                ActivationCase::OperatorOrchestratorEnrolment => {
                    if activation.class() != AuthorityClass::Orchestrator {
                        return Err(StateError::ActivationCaseInvalid);
                    }
                    // The unknown-actor requirement is checked after
                    // idempotency, so an identical replay still
                    // succeeds (see below).
                }
                ActivationCase::OperatorRecovery => {
                    // Recovers an existing orchestrator only; never
                    // creates an actor, never reopens bootstrap.
                    if activation.class() != AuthorityClass::Orchestrator {
                        return Err(StateError::ActivationCaseInvalid);
                    }
                    match self.actor_classes.borrow().get(activation.actor.as_str()) {
                        None => return Err(StateError::UnknownActor),
                        Some(existing) if *existing != activation.class() => {
                            return Err(StateError::ActorClassMismatch);
                        }
                        _ => {}
                    }
                }
            }
            if let Some(existing) = self.actor_classes.borrow().get(activation.actor.as_str())
                && *existing != activation.class()
            {
                return Err(StateError::ActorClassMismatch);
            }
            if let Some(existing) = self.committed.borrow().get(&key) {
                return if *existing == content {
                    Ok(StateApplied::AlreadyApplied)
                } else {
                    Err(StateError::ConflictingOperation)
                };
            }
            // Enrolment is for actors Scribe does not know; an existing
            // actor rotates or recovers instead. Identical replay was
            // already resolved above, so this only rejects genuinely
            // new operations naming a known actor (R5.15).
            if matches!(opening.case, ActivationCase::OperatorOrchestratorEnrolment)
                && self
                    .actor_classes
                    .borrow()
                    .contains_key(activation.actor.as_str())
            {
                return Err(StateError::ActivationCaseInvalid);
            }
            // One-shot bootstrap: identical replay was already handled
            // above; any NEW bootstrap operation after one committed is
            // refused, and deactivation never reopens it (R5.9).
            if matches!(opening.case, ActivationCase::OperatorBootstrap)
                && self.committed.borrow().contains_key("bootstrap:done")
            {
                return Err(StateError::BootstrapAlreadyComplete);
            }
            if activation.occupancy() == OccupancyClass::Singleton
                && let Some(members) = self.active_members.borrow().get(&occupied_key)
                && !members.is_empty()
                && !members.contains(activation.actor.as_str())
            {
                return Err(StateError::ProfileOccupied);
            }

            // ---- commit class + occupancy + operation together ----
            self.actor_classes
                .borrow_mut()
                .insert(activation.actor.as_str().to_owned(), activation.class());
            self.active_activation_generations.borrow_mut().insert(
                Self::activation_key(&activation.actor, &activation.profile),
                activation.operation.clone(),
            );
            self.launch_authorizers.borrow_mut().insert(
                Self::association_key(&LaunchSubject::ActorActivation {
                    actor: activation.actor.clone(),
                    profile: activation.profile.clone(),
                    generation: activation.operation.clone(),
                }),
                activation.operation.clone(),
            );
            // Membership is recorded for every occupancy class.
            self.active_members
                .borrow_mut()
                .entry(occupied_key)
                .or_default()
                .insert(activation.actor.as_str().to_owned());
            if matches!(opening.case, ActivationCase::OperatorBootstrap) {
                self.committed
                    .borrow_mut()
                    .insert("bootstrap:done".into(), "1".into());
            }
            let seq = self.next_ledger_seq();
            let initiator = match &opening.case {
                ActivationCase::ActorAuthorizedRotation { authority } => {
                    AuditInitiator::Authority(authority.clone())
                }
                ActivationCase::OperatorBootstrap
                | ActivationCase::OperatorRecovery
                | ActivationCase::OperatorOrchestratorEnrolment => AuditInitiator::OperatorChannel,
            };
            self.append_audit(
                seq,
                initiator,
                AuditOperation::Operation(activation.operation.clone()),
                AuditSubject::ActorProfile {
                    actor: activation.actor.clone(),
                    profile: activation.profile.clone(),
                },
                AuditKind::activation(&opening.case),
            );
            self.committed.borrow_mut().insert(key, content);
            Ok(StateApplied::Applied)
        }

        fn deactivate_profile(
            &self,
            operation: &OperationId,
            actor: &ActorId,
            profile: &ProfileName,
        ) -> Result<StateApplied, StateError> {
            let key = format!("deact:{}", operation.as_str());
            // Full (actor, profile) content participates — a same-op
            // deactivation of a different profile is a conflict, not a
            // silent second removal.
            let content = format!("{}|{}", actor.as_str(), profile.as_str());

            // ---- validate; mutate nothing ----
            if let Some(existing) = self.committed.borrow().get(&key) {
                return if *existing == content {
                    Ok(StateApplied::AlreadyApplied)
                } else {
                    Err(StateError::ConflictingOperation)
                };
            }
            // Only an ACTIVE member may release/revoke, in every
            // occupancy class (R5.10).
            let occupied_key = format!("occupied:{}", profile.as_str());
            let is_member = self
                .active_members
                .borrow()
                .get(&occupied_key)
                .is_some_and(|members| members.contains(actor.as_str()));
            if !is_member {
                return Err(StateError::NotTheOccupant);
            }

            // ---- commit revocation + membership release together ----
            // Removes ONLY this actor; co-occupants are untouched.
            self.active_members
                .borrow_mut()
                .entry(occupied_key)
                .or_default()
                .remove(actor.as_str());
            self.active_activation_generations
                .borrow_mut()
                .remove(&Self::activation_key(actor, profile));
            let seq = self.next_ledger_seq();
            self.append_audit(
                seq,
                AuditInitiator::OperatorChannel,
                AuditOperation::Operation(operation.clone()),
                AuditSubject::ActorProfile {
                    actor: actor.clone(),
                    profile: profile.clone(),
                },
                AuditKind::ProfileDeactivated,
            );
            self.committed.borrow_mut().insert(key, content);
            Ok(StateApplied::Applied)
        }

        fn append_signal(&self, draft: &SignalDraft) -> Result<(Signal, StateApplied), StateError> {
            if matches!(draft.body, SignalBody::Report { .. }) {
                return Err(StateError::IncoherentBundle);
            }
            if self
                .stored_signals
                .borrow()
                .iter()
                .any(|signal| signal.id == draft.id)
            {
                return self.commit_signal(draft);
            }
            crate::signal::validate_subject(&draft.body, &draft.subject)
                .map_err(|_| StateError::IncoherentBundle)?;
            if let SignalBody::Directive { attempt, .. } = &draft.body
                && self
                    .attempt_states
                    .borrow()
                    .get(attempt.as_str())
                    .is_none_or(|state| *state != AttemptState::Active)
            {
                return Err(StateError::IncoherentBundle);
            }
            let (signal, applied) = self.commit_signal(draft)?;
            if applied == StateApplied::Applied {
                self.append_audit(
                    signal.seq,
                    AuditInitiator::Authority(draft.sender.clone()),
                    AuditOperation::Signal(signal.id.clone()),
                    AuditSubject::Workflow(draft.subject.clone()),
                    AuditKind::signal(&signal),
                );
            }
            Ok((signal, applied))
        }

        fn fenced_report(
            &self,
            action: &FencedAction,
            draft: &ReportDraft,
        ) -> Result<(ReportOutcome, FencedResponse), StateError> {
            let call = &action.call;
            let request = format!("{}|{draft:?}", Self::action_identity(action));
            if let Some(response) = self.replay_fenced_response(
                "fenced_report",
                &call.operation,
                &request,
                &call.attempt,
            )? {
                let outcome = self
                    .reports
                    .borrow()
                    .get(call.operation.as_str())
                    .cloned()
                    .ok_or(StateError::Corrupt)?;
                return Ok((outcome, response));
            }
            // A durable record id belongs to exactly one operation.
            if let Some(owner) = self
                .record_owners
                .borrow()
                .get(&format!("signal:{}", draft.id.as_str()))
                && owner != call.operation.as_str()
            {
                return Err(StateError::ConflictingOperation);
            }
            self.active_fence(call)?;
            self.validate_response_target(action)?;
            let gate = {
                let signals = self.stored_signals.borrow();
                let actions = self.response_actions.borrow();
                let binding = binding_directives(&call.attempt, &signals, &actions);
                worker_append_gate(&binding)
            };
            if let Err(reason) = gate {
                let outcome = ReportOutcome::Refused { reason };
                self.reports
                    .borrow_mut()
                    .insert(call.operation.as_str().to_owned(), outcome.clone());
                self.remember(
                    "fenced_report",
                    &call.operation,
                    &request,
                    StateApplied::Applied,
                );
                let head = self.commit_fenced_call(Some(action), false);
                self.append_audit(
                    head,
                    self.worker_initiator(call),
                    AuditOperation::Operation(call.operation.clone()),
                    AuditSubject::Workflow(SubjectRef::Attempt(call.attempt.clone())),
                    AuditKind::report(&outcome),
                );
                return Ok((
                    outcome,
                    self.respond(&call.attempt, StateApplied::Applied, head),
                ));
            }

            let assignment = self
                .attempt_owners
                .borrow()
                .get(call.attempt.as_str())
                .and_then(|raw| AssignmentId::new(raw).ok())
                .ok_or(StateError::UnknownRecord)?;
            let actor = self
                .assignments
                .borrow()
                .get(assignment.as_str())
                .map(|record| record.worker.clone())
                .ok_or(StateError::UnknownRecord)?;
            let signal = Signal {
                id: draft.id.clone(),
                sender: SignalSender::WorkerBinding {
                    actor,
                    assignment,
                    attempt: call.attempt.clone(),
                },
                subject: SubjectRef::Attempt(call.attempt.clone()),
                body: SignalBody::Report {
                    attempt: call.attempt.clone(),
                    kind: draft.kind.clone(),
                },
                seq: self.next_ledger_seq(),
            };
            self.stored_signals.borrow_mut().push(signal.clone());
            let applied = StateApplied::Applied;
            self.record_owners.borrow_mut().insert(
                format!("signal:{}", draft.id.as_str()),
                call.operation.as_str().to_owned(),
            );
            let outcome = ReportOutcome::Recorded {
                signal: Box::new(signal),
            };
            self.reports
                .borrow_mut()
                .insert(call.operation.as_str().to_owned(), outcome.clone());
            let head = self.commit_fenced_call(Some(action), true);
            self.remember("fenced_report", &call.operation, &request, applied);
            self.append_audit(
                head,
                self.worker_initiator(call),
                AuditOperation::Operation(call.operation.clone()),
                AuditSubject::Workflow(SubjectRef::Attempt(call.attempt.clone())),
                AuditKind::report(&outcome),
            );
            Ok((outcome, self.respond(&call.attempt, applied, head)))
        }

        fn fenced_evidence(
            &self,
            action: &FencedAction,
            evidence: &Evidence,
        ) -> Result<(EvidenceOutcome, FencedResponse), StateError> {
            let call = &action.call;
            let request = format!("{}|{evidence:?}", Self::action_identity(action));
            if let Some(response) = self.replay_fenced_response(
                "fenced_evidence",
                &call.operation,
                &request,
                &call.attempt,
            )? {
                let outcome = self
                    .evidence_outcomes
                    .borrow()
                    .get(call.operation.as_str())
                    .copied()
                    .ok_or(StateError::Corrupt)?;
                return Ok((outcome, response));
            }
            self.active_fence(call)?;
            self.validate_response_target(action)?;

            let outcome = {
                let signals = self.stored_signals.borrow();
                let actions = self.response_actions.borrow();
                let binding = binding_directives(&call.attempt, &signals, &actions);
                match worker_append_gate(&binding) {
                    Ok(()) => EvidenceOutcome::Recorded,
                    Err(reason) => EvidenceOutcome::Refused { reason },
                }
            };
            let substantive = matches!(outcome, EvidenceOutcome::Recorded);
            if substantive {
                self.evidence_records.borrow_mut().push(EvidenceRecord {
                    operation: call.operation.clone(),
                    attempt: call.attempt.clone(),
                    evidence: evidence.clone(),
                });
            }
            self.evidence_outcomes
                .borrow_mut()
                .insert(call.operation.as_str().to_owned(), outcome);
            let head = self.commit_fenced_call(Some(action), substantive);
            self.remember(
                "fenced_evidence",
                &call.operation,
                &request,
                StateApplied::Applied,
            );
            self.append_audit(
                head,
                self.worker_initiator(call),
                AuditOperation::Operation(call.operation.clone()),
                AuditSubject::Workflow(SubjectRef::Attempt(call.attempt.clone())),
                AuditKind::evidence(outcome),
            );
            Ok((
                outcome,
                self.respond(&call.attempt, StateApplied::Applied, head),
            ))
        }

        fn fenced_submit_handoff(
            &self,
            action: &FencedAction,
            handoff: &HandoffRecord,
        ) -> Result<(SubmissionOutcome, FencedResponse), StateError> {
            let call = &action.call;
            let replay_request = format!("{}|{handoff:?}", Self::action_identity(action));
            if let Some(response) = self.replay_fenced_response(
                "fenced_handoff",
                &call.operation,
                &replay_request,
                &call.attempt,
            )? {
                let (_, stored) = self
                    .submissions
                    .borrow()
                    .get(call.operation.as_str())
                    .cloned()
                    .ok_or(StateError::Corrupt)?;
                return Ok((stored, response));
            }
            if let Some(owner) = self
                .record_owners
                .borrow()
                .get(&format!("handoff:{}", handoff.id.as_str()))
                && owner != call.operation.as_str()
            {
                return Err(StateError::ConflictingOperation);
            }
            self.active_fence(call)?;
            self.validate_response_target(action)?;
            if handoff.attempt != call.attempt {
                return Err(StateError::IncoherentBundle);
            }
            let key = call.operation.as_str().to_owned();
            let content = format!("{handoff:?}");
            if let Some((stored_content, stored_outcome)) = self.submissions.borrow().get(&key) {
                if *stored_content == content {
                    let response = self.respond(
                        &call.attempt,
                        StateApplied::AlreadyApplied,
                        self.current_head(),
                    );
                    return Ok((stored_outcome.clone(), response));
                }
                return Err(StateError::ConflictingOperation);
            }
            let outcome = if handoff.evidence_operations.is_empty() {
                SubmissionOutcome::Refused {
                    reason: SubmissionRefusalReason::MissingEvidence,
                }
            } else {
                // Evaluate the Directive gate against the action as it
                // would commit at the next position. A valid link may
                // therefore discharge an amend/answer Directive in the
                // same transaction whose post-commit state is returned.
                let candidate = Self::response_action(action, Seq(self.current_head().0 + 1));
                let signals = self.stored_signals.borrow();
                let mut actions = self.response_actions.borrow().clone();
                actions.push(candidate);
                let binding = binding_directives(&call.attempt, &signals, &actions);
                match handoff_gate(&binding) {
                    Ok(()) => SubmissionOutcome::Recorded {
                        handoff: handoff.id.clone(),
                    },
                    Err(reason) => SubmissionOutcome::Refused {
                        reason: SubmissionRefusalReason::Directive(reason),
                    },
                }
            };
            let substantive = matches!(outcome, SubmissionOutcome::Recorded { .. });
            if substantive {
                self.handoffs.borrow_mut().push(handoff.clone());
                self.record_owners.borrow_mut().insert(
                    format!("handoff:{}", handoff.id.as_str()),
                    call.operation.as_str().to_owned(),
                );
                self.attempt_states
                    .borrow_mut()
                    .insert(call.attempt.as_str().to_owned(), AttemptState::Submitted);
            }
            self.submissions
                .borrow_mut()
                .insert(key, (content, outcome.clone()));
            self.remember(
                "fenced_handoff",
                &call.operation,
                &replay_request,
                StateApplied::Applied,
            );
            let head = self.commit_fenced_call(Some(action), substantive);
            self.append_audit(
                head,
                self.worker_initiator(call),
                AuditOperation::Operation(call.operation.clone()),
                AuditSubject::Workflow(SubjectRef::Attempt(call.attempt.clone())),
                AuditKind::handoff(&outcome),
            );
            Ok((
                outcome,
                self.respond(&call.attempt, StateApplied::Applied, head),
            ))
        }

        fn fenced_abort_attempt(&self, call: &FencedCall) -> Result<FencedResponse, StateError> {
            let request = Self::call_identity(call);
            if let Some(response) = self.replay_fenced_response(
                "fenced_abort_attempt",
                &call.operation,
                &request,
                &call.attempt,
            )? {
                return Ok(response);
            }
            self.active_fence(call)?;
            let binding_has_abort = {
                let signals = self.stored_signals.borrow();
                let actions = self.response_actions.borrow();
                let binding = binding_directives(&call.attempt, &signals, &actions);
                worker_append_gate(&binding) == Err(DirectiveGateRefusal::AbortInForce)
            };
            if !binding_has_abort {
                return Err(StateError::AbortNotInForce);
            }

            let head = self.next_ledger_seq();
            let next = crate::lifecycle::attempt_transition(
                AttemptState::Active,
                crate::lifecycle::AttemptAction::Abort,
                false,
            )
            .expect("active Abort transition is a core invariant");
            self.attempt_states
                .borrow_mut()
                .insert(call.attempt.as_str().to_owned(), next);
            self.response_actions.borrow_mut().push(ResponseAction {
                seq: head,
                kind: ResponseKind::TerminalAttemptAction {
                    attempt: call.attempt.clone(),
                    abort_consistent: true,
                },
            });
            self.append_audit(
                head,
                self.worker_initiator(call),
                AuditOperation::Operation(call.operation.clone()),
                AuditSubject::Workflow(SubjectRef::Attempt(call.attempt.clone())),
                AuditKind::AttemptAborted,
            );
            self.remember(
                "fenced_abort_attempt",
                &call.operation,
                &request,
                StateApplied::Applied,
            );
            Ok(self.respond(&call.attempt, StateApplied::Applied, head))
        }

        fn renew_lease(
            &self,
            call: &FencedCall,
            until: Timestamp,
        ) -> Result<(Lease, FencedResponse), StateError> {
            let request = format!("{}|{until:?}", Self::call_identity(call));
            // Committed replay resolves BEFORE the mutable fence, so a
            // lost-response retry after expiry or token supersession
            // still returns its committed outcome (R5.27).
            if let Some(response) = self.replay_fenced_response(
                "renew_lease",
                &call.operation,
                &request,
                &call.attempt,
            )? {
                return Ok((
                    Lease {
                        token: call.token,
                        expires_at: until,
                    },
                    response,
                ));
            }
            self.fence(call)?;
            // A renewal must actually extend the lease.
            if until <= *self.lease_expires_at.borrow() {
                return Err(StateError::NonExtendingLease);
            }
            *self.lease_expires_at.borrow_mut() = until;
            self.remember(
                "renew_lease",
                &call.operation,
                &request,
                StateApplied::Applied,
            );
            let head = self.commit_fenced_call(None, false);
            self.append_audit(
                head,
                self.worker_initiator(call),
                AuditOperation::Operation(call.operation.clone()),
                AuditSubject::Workflow(SubjectRef::Attempt(call.attempt.clone())),
                AuditKind::LeaseRenewed,
            );
            Ok((
                Lease {
                    token: call.token,
                    expires_at: until,
                },
                self.respond(&call.attempt, StateApplied::Applied, head),
            ))
        }

        fn persist_envelope(
            &self,
            operation: &OperationId,
            subject: &LaunchSubject,
            envelope: &EnvelopeSnapshot,
        ) -> Result<StateApplied, StateError> {
            self.resolve_subject(subject)?;
            let key = Self::association_key(subject);
            let request = format!("{key}|{envelope:?}");
            if let Some(stored) = self.replay("persist_envelope", operation, &request)? {
                return Ok(stored);
            }
            if let Some(existing) = self.envelopes.borrow().get(&key)
                && existing != envelope
            {
                return Err(StateError::ConflictingOperation);
            }
            let authorizing = self.launch_authorizing(subject)?;
            self.envelopes.borrow_mut().insert(key, envelope.clone());
            self.remember(
                "persist_envelope",
                operation,
                &request,
                StateApplied::Applied,
            );
            let seq = self.next_ledger_seq();
            self.append_audit(
                seq,
                AuditInitiator::SystemProjection { authorizing },
                AuditOperation::Operation(operation.clone()),
                AuditSubject::Launch(subject.clone()),
                AuditKind::EnvelopePersisted,
            );
            Ok(StateApplied::Applied)
        }

        fn envelope(&self, subject: &LaunchSubject) -> Result<EnvelopeSnapshot, StateError> {
            self.resolve_subject(subject)?;
            self.envelopes
                .borrow()
                .get(&Self::association_key(subject))
                .cloned()
                .ok_or(StateError::UnknownRecord)
        }

        fn bind_runtime_handle(
            &self,
            operation: &OperationId,
            subject: &LaunchSubject,
            handle: &RuntimeHandle,
        ) -> Result<StateApplied, StateError> {
            self.resolve_subject(subject)?;
            let key = Self::association_key(subject);
            let request = format!("{key}|{}", handle.as_str());
            // A stale replay returns its STORED result without
            // resurrecting a handle that was later unbound/rebound.
            if let Some(stored) = self.replay("bind", operation, &request)? {
                return Ok(stored);
            }
            if let Some(existing) = self.handles.borrow().get(&key)
                && existing != handle
            {
                return Err(StateError::ConflictingOperation);
            }
            let authorizing = self.launch_authorizing(subject)?;
            self.handles.borrow_mut().insert(key, handle.clone());
            self.remember("bind", operation, &request, StateApplied::Applied);
            let seq = self.next_ledger_seq();
            self.append_audit(
                seq,
                AuditInitiator::SystemProjection { authorizing },
                AuditOperation::Operation(operation.clone()),
                AuditSubject::Launch(subject.clone()),
                AuditKind::RuntimeHandleBound,
            );
            Ok(StateApplied::Applied)
        }

        fn unbind_runtime_handle(
            &self,
            operation: &OperationId,
            subject: &LaunchSubject,
        ) -> Result<StateApplied, StateError> {
            self.resolve_subject(subject)?;
            let key = Self::association_key(subject);
            let request = key.clone();
            if let Some(stored) = self.replay("unbind", operation, &request)? {
                return Ok(stored);
            }
            let authorizing = self.launch_authorizing(subject)?;
            self.handles.borrow_mut().remove(&key);
            self.remember("unbind", operation, &request, StateApplied::Applied);
            let seq = self.next_ledger_seq();
            self.append_audit(
                seq,
                AuditInitiator::SystemProjection { authorizing },
                AuditOperation::Operation(operation.clone()),
                AuditSubject::Launch(subject.clone()),
                AuditKind::RuntimeHandleUnbound,
            );
            Ok(StateApplied::Applied)
        }

        fn runtime_handle(
            &self,
            subject: &LaunchSubject,
        ) -> Result<Option<RuntimeHandle>, StateError> {
            self.resolve_subject(subject)?;
            Ok(self
                .handles
                .borrow()
                .get(&Self::association_key(subject))
                .cloned())
        }

        fn record_runtime_observation(
            &self,
            operation: &OperationId,
            record: &RuntimeObservationRecord,
        ) -> Result<StateApplied, StateError> {
            self.resolve_subject(&record.subject)?;
            let request = format!("{record:?}");
            if let Some(stored) = self.replay("runtime_observation", operation, &request)? {
                return Ok(stored);
            }
            let seq = self.next_ledger_seq();
            self.runtime_observations
                .borrow_mut()
                .insert(operation.as_str().to_owned(), record.clone());
            self.append_audit(
                seq,
                AuditInitiator::Authority(record.reporter.clone()),
                AuditOperation::Operation(operation.clone()),
                AuditSubject::Launch(record.subject.clone()),
                AuditKind::RuntimeObservationRecorded,
            );
            self.remember(
                "runtime_observation",
                operation,
                &request,
                StateApplied::Applied,
            );
            Ok(StateApplied::Applied)
        }

        fn runtime_observation(
            &self,
            operation: &OperationId,
        ) -> Result<RuntimeObservationRecord, StateError> {
            self.runtime_observations
                .borrow()
                .get(operation.as_str())
                .cloned()
                .ok_or(StateError::UnknownRecord)
        }

        fn record_application_attempt(
            &self,
            attempt: &ApplicationAttempt,
        ) -> Result<StateApplied, StateError> {
            // The target must be a committed projection (R5.25).
            if !self
                .projections
                .borrow()
                .contains_key(attempt.target.as_str())
            {
                return Err(StateError::UnknownRecord);
            }
            let applied = self.apply(
                format!("app-attempt:{}", attempt.id.as_str()),
                format!("{attempt:?}"),
            )?;
            if applied == StateApplied::Applied {
                self.application_attempts
                    .borrow_mut()
                    .entry(attempt.target.as_str().to_owned())
                    .or_default()
                    .push((attempt.id.clone(), attempt.outcome.clone()));
                let seq = self.next_ledger_seq();
                self.append_audit(
                    seq,
                    AuditInitiator::SystemProjection {
                        authorizing: attempt.target.clone(),
                    },
                    AuditOperation::Operation(attempt.id.clone()),
                    AuditSubject::Projection(attempt.target.clone()),
                    AuditKind::application(&attempt.outcome),
                );
            }
            Ok(applied)
        }

        fn record_application_receipt(
            &self,
            receipt: &ApplicationReceipt,
        ) -> Result<StateApplied, StateError> {
            // The projection target must exist…
            if !self
                .projections
                .borrow()
                .contains_key(receipt.target.as_str())
            {
                return Err(StateError::UnknownRecord);
            }
            // …and the EXACT named attempt must exist for that target,
            // have succeeded, and report this after-revision (R5.26).
            let attempts = self.application_attempts.borrow();
            let Some(recorded) = attempts.get(receipt.target.as_str()) else {
                return Err(StateError::IncoherentBundle);
            };
            let Some((_, outcome)) = recorded.iter().find(|(id, _)| id == &receipt.attempt) else {
                return Err(StateError::IncoherentBundle);
            };
            // Only a provider-attested application is receipt-eligible
            // (ADR-0001 effect-provenance amendment): a receipt can no
            // more be manufactured from an observation here than at
            // composition.
            let revision_matches = match outcome {
                ApplicationOutcome::Applied { after, .. } => after == &receipt.after,
                ApplicationOutcome::FoundPresent { .. }
                | ApplicationOutcome::ObservedAfterAmbiguous { .. }
                | ApplicationOutcome::Failed { .. }
                | ApplicationOutcome::Ambiguous => false,
            };
            if !revision_matches {
                return Err(StateError::IncoherentBundle);
            }
            drop(attempts);
            let applied = self.apply(
                format!("receipt:{}", receipt.target.as_str()),
                format!("{receipt:?}"),
            )?;
            if applied == StateApplied::Applied {
                self.receipts
                    .borrow_mut()
                    .push(receipt.target.as_str().to_owned());
                let seq = self.next_ledger_seq();
                self.append_audit(
                    seq,
                    AuditInitiator::SystemProjection {
                        authorizing: receipt.target.clone(),
                    },
                    AuditOperation::Operation(receipt.target.clone()),
                    AuditSubject::Projection(receipt.target.clone()),
                    AuditKind::ApplicationReceiptRecorded,
                );
            }
            Ok(applied)
        }

        fn assignment(&self, _id: &AssignmentId) -> Result<AssignmentView, StateError> {
            Err(StateError::UnknownRecord)
        }

        fn evidence_for(&self, attempt: &AttemptId) -> Result<Vec<EvidenceRecord>, StateError> {
            Ok(self
                .evidence_records
                .borrow()
                .iter()
                .filter(|record| &record.attempt == attempt)
                .cloned()
                .collect())
        }

        fn signals_for(&self, _attempt: &AttemptId) -> Result<Vec<Signal>, StateError> {
            Ok(self.stored_signals.borrow().clone())
        }

        fn handoff(&self, id: &HandoffId) -> Result<HandoffRecord, StateError> {
            self.handoffs
                .borrow()
                .iter()
                .find(|h| &h.id == id)
                .cloned()
                .ok_or(StateError::UnknownRecord)
        }

        fn decision(&self, operation: &OperationId) -> Result<DecisionRecord, StateError> {
            self.decisions
                .borrow()
                .iter()
                .find(|d| &d.operation == operation)
                .cloned()
                .ok_or(StateError::UnknownRecord)
        }

        fn active_occupants(&self, profile: &ProfileName) -> Result<Vec<ActorId>, StateError> {
            Ok(self
                .active_members
                .borrow()
                .get(&format!("occupied:{}", profile.as_str()))
                .map(|members| {
                    members
                        .iter()
                        .map(|a| ActorId::new(a).unwrap())
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default())
        }

        fn pending_applications(&self) -> Result<Vec<PendingApplication>, StateError> {
            let receipts = self.receipts.borrow();
            Ok(self
                .projections
                .borrow()
                .values()
                .filter(|p| !receipts.iter().any(|r| r == p.operation.as_str()))
                .cloned()
                .collect::<Vec<_>>())
            .map(|mut pending: Vec<PendingApplication>| {
                // Causal order, never accidental key order (R5.26).
                pending.sort_by_key(|p| p.committed_at);
                pending
            })
        }

        fn superseded_applications(&self) -> Result<Vec<SupersededApplication>, StateError> {
            unimplemented!("core port tests never query the superseded view")
        }

        fn unresolved_signals(
            &self,
            _recipient: Option<&ActorId>,
        ) -> Result<Vec<Signal>, StateError> {
            Ok(Vec::new())
        }

        fn audit_events(&self, query: &AuditQuery) -> Result<Vec<AuditEvent>, StateError> {
            Ok(self
                .audit_events
                .borrow()
                .values()
                .filter(|event| {
                    query
                        .subject
                        .as_ref()
                        .is_none_or(|subject| &event.subject == subject)
                        && query.class.is_none_or(|class| event.kind.class() == class)
                        && query.from.is_none_or(|from| event.seq >= from)
                        && query.through.is_none_or(|through| event.seq <= through)
                })
                .cloned()
                .collect())
        }
    }

    fn fake_state() -> FakeState {
        FakeState {
            committed: RefCell::new(BTreeMap::new()),
            current_token: RefCell::new(FencingToken(3)),
            bound_worker_snapshot: worker_snapshot(),
            lease_expires_at: RefCell::new(Timestamp(100)),
            now: RefCell::new(Timestamp(50)),
            next_seq: RefCell::new(0),
            stored_signals: RefCell::new(Vec::new()),
            evidence_records: RefCell::new(Vec::new()),
            response_actions: RefCell::new(Vec::new()),
            reports: RefCell::new(BTreeMap::new()),
            evidence_outcomes: RefCell::new(BTreeMap::new()),
            submissions: RefCell::new(BTreeMap::new()),
            receipts: RefCell::new(Vec::new()),
            decisions: RefCell::new(Vec::new()),
            attempt_owners: RefCell::new(BTreeMap::from([(
                "att-1".to_owned(),
                "asg-1".to_owned(),
            )])),
            launch_authorizers: RefCell::new(BTreeMap::new()),
            active_activation_generations: RefCell::new(BTreeMap::new()),
            envelopes: RefCell::new(BTreeMap::new()),
            handles: RefCell::new(BTreeMap::new()),
            operations: RefCell::new(BTreeMap::new()),
            record_owners: RefCell::new(BTreeMap::new()),
            projections: RefCell::new(BTreeMap::new()),
            assignments: RefCell::new(BTreeMap::from([("asg-1".to_owned(), opening().assignment)])),
            application_attempts: RefCell::new(BTreeMap::new()),
            active_members: RefCell::new(BTreeMap::new()),
            handoffs: RefCell::new(Vec::new()),
            actor_classes: RefCell::new(BTreeMap::new()),
            attempt_states: RefCell::new(BTreeMap::from([(
                "att-1".to_owned(),
                AttemptState::Active,
            )])),
            audit_events: RefCell::new(BTreeMap::new()),
            runtime_observations: RefCell::new(BTreeMap::new()),
        }
    }

    fn good_call(operation: &str) -> FencedCall {
        FencedCall {
            attempt: AttemptId::new("att-1").unwrap(),
            token: FencingToken(3),
            operation: op(operation),
        }
    }

    fn good_action(operation: &str) -> FencedAction {
        FencedAction {
            call: good_call(operation),
            responds_to: None,
        }
    }

    fn worker_action_count(state: &FakeState) -> usize {
        state
            .response_actions
            .borrow()
            .iter()
            .filter(|action| matches!(&action.kind, ResponseKind::WorkerAction { .. }))
            .count()
    }

    fn report_draft(id: &str) -> ReportDraft {
        ReportDraft {
            id: SignalId::new(id).unwrap(),
            kind: ReportKind::Progress {
                phase: crate::signal::SemanticPhase::Verifying,
                summary: None,
            },
        }
    }

    fn directive_draft(id: &str, attempt: &str, kind: DirectiveKind) -> SignalDraft {
        let attempt = AttemptId::new(attempt).unwrap();
        SignalDraft {
            id: SignalId::new(id).unwrap(),
            sender: authority("state:directive"),
            subject: SubjectRef::Attempt(attempt.clone()),
            body: SignalBody::Directive {
                assignment: AssignmentId::new("asg-1").unwrap(),
                attempt,
                kind,
            },
        }
    }

    fn passing_evidence() -> Evidence {
        Evidence::new(
            Argv::new(vec!["cargo".into(), "test".into()]).unwrap(),
            VerificationSet::new(
                vec![Argv::new(vec!["cargo".into(), "test".into()]).unwrap()],
                PathSet::new(vec![WorkPath::new("tests/a.rs").unwrap()]).unwrap(),
            )
            .unwrap(),
            0,
            VerificationOutcome::Pass,
            CommitId::new(&"c".repeat(40)).unwrap(),
            crate::content::WorkspaceDigest::new(&"1".repeat(64)).unwrap(),
            crate::content::WorkspaceDigest::new(&"1".repeat(64)).unwrap(),
            None,
            FileDigestSet::default(),
            None,
        )
        .unwrap()
    }

    fn handoff(id: &str, evidence: Vec<OperationId>) -> HandoffRecord {
        let evidence = OperationSet::new(evidence).unwrap();
        HandoffRecord {
            id: HandoffId::new(id).unwrap(),
            attempt: AttemptId::new("att-1").unwrap(),
            commit: CommitId::new(&"d".repeat(40)).unwrap(),
            expected_base: CommitId::new(&"c".repeat(40)).unwrap(),
            clean_tree: crate::content::WorkspaceDigest::new(&"9".repeat(64)).unwrap(),
            changed_paths: PathSet::new(vec![WorkPath::new("src/lib.rs").unwrap()]).unwrap(),
            evidence_operations: evidence,
            attestation: ContentHash::new(&"e".repeat(64)).unwrap(),
        }
    }

    #[test]
    fn opening_bundle_uses_one_operation_identity() {
        let state = fake_state();
        let port: &dyn WorkflowStatePort = &state;
        assert_eq!(port.open_assignment(&opening()), Ok(StateApplied::Applied));
        assert_eq!(
            port.open_assignment(&opening()),
            Ok(StateApplied::AlreadyApplied)
        );
        let mut altered = opening();
        altered.bead_revision = rev('9');
        assert_eq!(
            port.open_assignment(&altered),
            Err(StateError::ConflictingOperation)
        );
    }

    #[test]
    fn retry_bundle_records_its_attempt_locator_atomically() {
        let state = fake_state();
        let port: &dyn WorkflowStatePort = &state;
        // Ownership resolves from this Assignment's existing bindings.
        assert_eq!(port.open_assignment(&opening()), Ok(StateApplied::Applied));
        let retry_opening = AttemptOpening {
            authorizing: RetryDecision {
                operation: op("op-retry"),
                assignment: AssignmentId::new("asg-1").unwrap(),
                authority: authority("state:assign"),
                reason: reason("previous attempt expired"),
            },
            attempt: AttemptRecord {
                id: AttemptId::new("att-2").unwrap(),
                assignment: AssignmentId::new("asg-1").unwrap(),
                lease: Lease {
                    token: FencingToken(2),
                    expires_at: Timestamp(200),
                },
            },
        };
        assert_eq!(
            port.append_attempt(&retry_opening),
            Ok(StateApplied::Applied)
        );
        assert_eq!(
            port.append_attempt(&retry_opening),
            Ok(StateApplied::AlreadyApplied)
        );
        let mut altered = retry_opening;
        altered.attempt.lease.expires_at = Timestamp(201);
        assert_eq!(
            port.append_attempt(&altered),
            Err(StateError::ConflictingOperation)
        );
    }

    #[test]
    fn scribe_allocates_signal_order_and_absorbs_retries() {
        let state = fake_state();
        let port: &dyn WorkflowStatePort = &state;
        let draft = directive_draft(
            "sig-1",
            "att-1",
            DirectiveKind::Amend {
                instruction: BoundedText::new("update the implementation").unwrap(),
            },
        );
        let (first, applied) = port.append_signal(&draft).unwrap();
        assert_eq!(applied, StateApplied::Applied);
        assert_eq!(first.seq, Seq(1));
        let (again, retry) = port.append_signal(&draft).unwrap();
        assert_eq!(retry, StateApplied::AlreadyApplied);
        assert_eq!(again, first);
        let mut altered = draft.clone();
        altered.body = SignalBody::Directive {
            assignment: AssignmentId::new("asg-1").unwrap(),
            attempt: AttemptId::new("att-1").unwrap(),
            kind: DirectiveKind::Pause {
                reason: BoundedText::new("blocked on dependency").unwrap(),
            },
        };
        assert_eq!(
            port.append_signal(&altered),
            Err(StateError::ConflictingOperation)
        );
    }

    #[test]
    fn fenced_calls_resolve_actor_and_check_token() {
        let state = fake_state();
        let port: &dyn WorkflowStatePort = &state;
        assert_eq!(port.open_assignment(&opening()), Ok(StateApplied::Applied));
        let evidence = crate::evidence::Evidence::new(
            Argv::new(vec!["cargo".into(), "test".into()]).unwrap(),
            VerificationSet::new(
                vec![Argv::new(vec!["cargo".into(), "test".into()]).unwrap()],
                PathSet::new(vec![WorkPath::new("tests/a.rs").unwrap()]).unwrap(),
            )
            .unwrap(),
            0,
            VerificationOutcome::Pass,
            CommitId::new(&"c".repeat(40)).unwrap(),
            crate::content::WorkspaceDigest::new(&"1".repeat(64)).unwrap(),
            crate::content::WorkspaceDigest::new(&"1".repeat(64)).unwrap(),
            None,
            FileDigestSet::default(),
            None,
        )
        .unwrap();
        let stale = FencedAction {
            call: FencedCall {
                token: FencingToken(2),
                ..good_call("op-evi")
            },
            responds_to: None,
        };
        assert_eq!(
            port.fenced_evidence(&stale, &evidence),
            Err(StateError::StaleFencing)
        );
        let (outcome, response) = port
            .fenced_evidence(&good_action("op-evi"), &evidence)
            .unwrap();
        assert_eq!(outcome, EvidenceOutcome::Recorded);
        assert_eq!(response.applied, StateApplied::Applied);
    }

    #[test]
    fn submission_operation_owns_recorded_and_refused_outcomes() {
        let state = fake_state();
        let port: &dyn WorkflowStatePort = &state;

        let refused = handoff("h-1", vec![]);
        let (outcome, response) = port
            .fenced_submit_handoff(&good_action("op-h1"), &refused)
            .unwrap();
        assert_eq!(
            outcome,
            SubmissionOutcome::Refused {
                reason: SubmissionRefusalReason::MissingEvidence
            }
        );
        assert_eq!(response.applied, StateApplied::Applied);
        let (retry_outcome, retry_response) = port
            .fenced_submit_handoff(&good_action("op-h1"), &refused)
            .unwrap();
        assert_eq!(retry_outcome, outcome);
        assert_eq!(retry_response.applied, StateApplied::AlreadyApplied);
        let different = handoff("h-2", vec![]);
        assert_eq!(
            port.fenced_submit_handoff(&good_action("op-h1"), &different),
            Err(StateError::ConflictingOperation)
        );

        let recorded = handoff("h-3", vec![op("op-evi")]);
        let (outcome, _) = port
            .fenced_submit_handoff(&good_action("op-h2"), &recorded)
            .unwrap();
        assert_eq!(
            outcome,
            SubmissionOutcome::Recorded {
                handoff: HandoffId::new("h-3").unwrap()
            }
        );
        let (retry_outcome, retry_response) = port
            .fenced_submit_handoff(&good_action("op-h2"), &recorded)
            .unwrap();
        assert_eq!(retry_outcome, outcome);
        assert_eq!(retry_response.applied, StateApplied::AlreadyApplied);
        let different = handoff("h-4", vec![op("op-evi")]);
        assert_eq!(
            port.fenced_submit_handoff(&good_action("op-h2"), &different),
            Err(StateError::ConflictingOperation)
        );
    }

    #[test]
    fn application_attempts_have_their_own_identity() {
        let state = fake_state();
        let port: &dyn WorkflowStatePort = &state;
        let decision = DecisionRecord {
            operation: op("op-accept"),
            assignment: AssignmentId::new("asg-1").unwrap(),
            authority: authority("state:accept"),
            kind: DecisionKind::Accept {
                handoff: HandoffId::new("h-1").unwrap(),
                reason: reason("verified handoff"),
            },
            resolves: None,
        };
        assert_eq!(
            decision.kind.close_reason(),
            Some(CloseReason::AcceptedHandoff)
        );
        // Accept must decide a REAL Handoff belonging to this
        // Assignment, so record one first (R5.23).
        assert_eq!(port.open_assignment(&opening()), Ok(StateApplied::Applied));
        let recorded = handoff("h-1", vec![op("op-evi")]);
        let (outcome, _) = port
            .fenced_submit_handoff(&good_action("op-h-accept"), &recorded)
            .unwrap();
        assert_eq!(
            outcome,
            SubmissionOutcome::Recorded {
                handoff: HandoffId::new("h-1").unwrap()
            }
        );
        assert_eq!(port.record_decision(&decision), Ok(StateApplied::Applied));
        // TWO projections are pending: the opening's mark-in-progress
        // and the Acceptance close (R5.25).
        let pending = port.pending_applications().unwrap();
        assert_eq!(pending.len(), 2);
        assert!(
            pending
                .iter()
                .any(|p| p.projection == WorkProjection::MarkInProgress)
        );
        assert!(pending.iter().any(|p| matches!(
            p.projection,
            WorkProjection::Close {
                reason: CloseReason::AcceptedHandoff
            }
        )));

        let first = ApplicationAttempt {
            id: op("app-1"),
            target: op("op-accept"),
            outcome: ApplicationOutcome::Failed {
                error: WorkError::Busy,
            },
        };
        assert_eq!(
            port.record_application_attempt(&first),
            Ok(StateApplied::Applied)
        );
        assert_eq!(
            port.record_application_attempt(&first),
            Ok(StateApplied::AlreadyApplied)
        );
        let conflicting = ApplicationAttempt {
            id: op("app-1"),
            target: op("op-accept"),
            outcome: ApplicationOutcome::Ambiguous,
        };
        assert_eq!(
            port.record_application_attempt(&conflicting),
            Err(StateError::ConflictingOperation)
        );
        let second = ApplicationAttempt {
            id: op("app-2"),
            target: op("op-accept"),
            outcome: ApplicationOutcome::Ambiguous,
        };
        assert_eq!(
            port.record_application_attempt(&second),
            Ok(StateApplied::Applied)
        );
        // Failed/ambiguous attempts never clear pending, and a receipt
        // cannot be manufactured from them (R5.25).
        assert_eq!(port.pending_applications().unwrap().len(), 2);
        assert_eq!(
            port.record_application_receipt(&ApplicationReceipt {
                target: op("op-accept"),
                attempt: op("app-3"),
                after: rev('f'),
            }),
            Err(StateError::IncoherentBundle)
        );
        // Receipt before any projection target refuses.
        assert_eq!(
            port.record_application_receipt(&ApplicationReceipt {
                target: op("op-absent"),
                attempt: op("app-3"),
                after: rev('f'),
            }),
            Err(StateError::UnknownRecord)
        );
        // An attempt naming no projection refuses.
        assert_eq!(
            port.record_application_attempt(&ApplicationAttempt {
                id: op("app-x"),
                target: op("op-absent"),
                outcome: ApplicationOutcome::Applied {
                    before: rev('a'),
                    after: rev('f')
                },
            }),
            Err(StateError::UnknownRecord)
        );
        // A linked SUCCESSFUL attempt lets the receipt clear exactly
        // that projection; the opening's remains pending.
        assert_eq!(
            port.record_application_attempt(&ApplicationAttempt {
                id: op("app-3"),
                target: op("op-accept"),
                outcome: ApplicationOutcome::Applied {
                    before: rev('a'),
                    after: rev('f')
                },
            }),
            Ok(StateApplied::Applied)
        );
        assert_eq!(
            port.record_application_receipt(&ApplicationReceipt {
                target: op("op-accept"),
                attempt: op("app-3"),
                after: rev('f'),
            }),
            Ok(StateApplied::Applied)
        );
        let remaining = port.pending_applications().unwrap();
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].projection, WorkProjection::MarkInProgress);
    }

    fn small_validated_set() -> ValidatedProfileSet {
        let registry = vec![
            CapabilityDescriptor {
                id: CapabilityId::new("work:select").unwrap(),
                class: CheckClass::Exclusive,
                bundle: None,
                work_scoped: true,
            },
            CapabilityDescriptor {
                id: CapabilityId::new("state:report").unwrap(),
                class: CheckClass::Fenced,
                bundle: None,
                work_scoped: true,
            },
        ];
        let keys = vec![crate::scope::ScopeKey::new("area").unwrap()];
        let profiles = vec![
            ProfileSpec {
                name: ProfileName::new("lead").unwrap(),
                class: AuthorityClass::Orchestrator,
                grants: vec![Grant {
                    capability: CapabilityId::new("work:select").unwrap(),
                    scope: ScopeExpr::parse("area=frontend", &keys).unwrap(),
                }],
            },
            // A second orchestrator partitioning the remaining scope —
            // the multi-orchestrator topology ADR-0002 §7 requires.
            ProfileSpec {
                name: ProfileName::new("second-lead").unwrap(),
                class: AuthorityClass::Orchestrator,
                grants: vec![Grant {
                    capability: CapabilityId::new("work:select").unwrap(),
                    scope: ScopeExpr::parse("area!=frontend", &keys).unwrap(),
                }],
            },
            ProfileSpec {
                name: ProfileName::new("worker").unwrap(),
                class: AuthorityClass::Worker,
                grants: vec![Grant {
                    capability: CapabilityId::new("state:report").unwrap(),
                    scope: ScopeExpr::Universal,
                }],
            },
        ];
        validate_profiles(&profiles, &registry).unwrap()
    }

    fn activation_with(
        op_id: &str,
        actor_id: &str,
        profile: &str,
        case: ActivationCase,
    ) -> ActivationOpening {
        let mut opening = activation(op_id, actor_id, profile);
        opening.case = case;
        opening
    }

    fn activation(op_id: &str, actor_id: &str, profile: &str) -> ActivationOpening {
        ActivationOpening {
            activation: ProfileActivation::from_validated(
                &small_validated_set(),
                op(op_id),
                ActorId::new(actor_id).unwrap(),
                ProfileName::new(profile).unwrap(),
                ContentHash::new(&"a".repeat(64)).unwrap(),
            )
            .unwrap(),
            case: ActivationCase::OperatorBootstrap,
        }
    }

    #[test]
    fn occupancy_class_governs_activation() {
        let state = fake_state();
        let port: &dyn WorkflowStatePort = &state;
        // Workers are registered by the opening, then may rotate;
        // bootstrap is orchestrator-only (R5.7).
        assert_eq!(
            port.activate_profile(&activation_with(
                "a-0",
                "worker-1",
                "worker",
                ActivationCase::OperatorBootstrap
            )),
            Err(StateError::ActivationCaseInvalid)
        );
        assert_eq!(
            port.activate_profile(&activation_with(
                "a-x",
                "worker-9",
                "worker",
                ActivationCase::ActorAuthorizedRotation {
                    authority: authority("state:assign")
                }
            )),
            Err(StateError::UnknownActor)
        );
        assert_eq!(port.open_assignment(&opening()), Ok(StateApplied::Applied));
        assert_eq!(
            port.activate_profile(&activation_with(
                "a-1",
                "worker-1",
                "worker",
                ActivationCase::ActorAuthorizedRotation {
                    authority: authority("state:assign")
                }
            )),
            Ok(StateApplied::Applied)
        );
        assert_eq!(
            port.activate_profile(&activation_with(
                "a-2",
                "worker-1",
                "worker",
                ActivationCase::ActorAuthorizedRotation {
                    authority: authority("state:assign")
                }
            )),
            Ok(StateApplied::Applied)
        );
        assert_eq!(
            port.activate_profile(&activation("a-3", "lead-1", "lead")),
            Ok(StateApplied::Applied)
        );
        // Bootstrap is one-shot: a second bootstrap operation is
        // refused outright, before occupancy is even consulted (R5.9).
        assert_eq!(
            port.activate_profile(&activation("a-4", "lead-2", "lead")),
            Err(StateError::BootstrapAlreadyComplete)
        );
        assert_eq!(
            port.deactivate_profile(
                &op("d-1"),
                &ActorId::new("lead-1").unwrap(),
                &ProfileName::new("lead").unwrap()
            ),
            Ok(StateApplied::Applied)
        );
        // Deactivation frees occupancy but does NOT reopen bootstrap.
        assert_eq!(
            port.activate_profile(&activation("a-5", "lead-2", "lead")),
            Err(StateError::BootstrapAlreadyComplete)
        );
    }

    /// R5.15: multi-orchestrator topology — a second orchestrator is
    /// enrolled through the operator channel, with occupancy still
    /// enforced and the one-shot bootstrap sentinel untouched.
    #[test]
    fn operator_channel_enrols_additional_orchestrators() {
        let state = fake_state();
        let port: &dyn WorkflowStatePort = &state;
        assert_eq!(
            port.activate_profile(&activation("boot", "lead-1", "lead")),
            Ok(StateApplied::Applied)
        );
        // A NEW orchestrator actor enrols into another profile.
        let second = activation_with(
            "enrol-1",
            "lead-2",
            "second-lead",
            ActivationCase::OperatorOrchestratorEnrolment,
        );
        assert_eq!(port.activate_profile(&second), Ok(StateApplied::Applied));
        assert_eq!(
            port.activate_profile(&second),
            Ok(StateApplied::AlreadyApplied)
        );
        let mut conflicting = second.clone();
        conflicting.activation.profile_hash = ContentHash::new(&"b".repeat(64)).unwrap();
        assert_eq!(
            port.activate_profile(&conflicting),
            Err(StateError::ConflictingOperation)
        );
        // Enrolment is only for unknown actors; a known one rotates.
        assert_eq!(
            port.activate_profile(&activation_with(
                "enrol-2",
                "lead-1",
                "lead",
                ActivationCase::OperatorOrchestratorEnrolment
            )),
            Err(StateError::ActivationCaseInvalid)
        );
        // Occupancy is still enforced for a singleton profile.
        assert_eq!(
            port.activate_profile(&activation_with(
                "enrol-3",
                "lead-3",
                "lead",
                ActivationCase::OperatorOrchestratorEnrolment
            )),
            Err(StateError::ProfileOccupied)
        );
        // The bootstrap sentinel is untouched by enrolment.
        assert_eq!(
            port.activate_profile(&activation("boot-2", "lead-4", "lead")),
            Err(StateError::BootstrapAlreadyComplete)
        );
    }

    /// R5.13: an initial orchestrator recovers through the operator
    /// channel — without reopening one-shot
    /// bootstrap and without creating a new actor.
    #[test]
    fn operator_recovery_rotates_an_existing_orchestrator_only() {
        let state = fake_state();
        let port: &dyn WorkflowStatePort = &state;
        assert_eq!(
            port.activate_profile(&activation("boot", "lead-1", "lead")),
            Ok(StateApplied::Applied)
        );
        // Recovery of an unregistered actor never creates one.
        assert_eq!(
            port.activate_profile(&activation_with(
                "rec-0",
                "ghost",
                "lead",
                ActivationCase::OperatorRecovery
            )),
            Err(StateError::UnknownActor)
        );
        // Recovery of the registered orchestrator succeeds, is
        // idempotent on replay, and conflicts on changed content.
        let recovery = activation_with("rec-1", "lead-1", "lead", ActivationCase::OperatorRecovery);
        assert_eq!(port.activate_profile(&recovery), Ok(StateApplied::Applied));
        assert_eq!(
            port.activate_profile(&recovery),
            Ok(StateApplied::AlreadyApplied)
        );
        let mut conflicting = recovery;
        conflicting.activation.profile_hash = ContentHash::new(&"b".repeat(64)).unwrap();
        assert_eq!(
            port.activate_profile(&conflicting),
            Err(StateError::ConflictingOperation)
        );
        // Bootstrap remains closed regardless.
        assert_eq!(
            port.activate_profile(&activation("boot-2", "lead-2", "lead")),
            Err(StateError::BootstrapAlreadyComplete)
        );
    }

    #[test]
    fn activation_idempotency_covers_full_content() {
        let state = fake_state();
        let port: &dyn WorkflowStatePort = &state;
        let base = activation("a-10", "lead-9", "lead");
        assert_eq!(port.activate_profile(&base), Ok(StateApplied::Applied));
        assert_eq!(
            port.activate_profile(&base),
            Ok(StateApplied::AlreadyApplied)
        );
        // Same operation, different profile snapshot → conflict.
        let mut profile_hash = base.clone();
        profile_hash.activation.profile_hash = ContentHash::new(&"7".repeat(64)).unwrap();
        assert_eq!(
            port.activate_profile(&profile_hash),
            Err(StateError::ConflictingOperation)
        );
        // Same operation, different authority case → conflict.
        let mut case = base;
        case.case = ActivationCase::ActorAuthorizedRotation {
            authority: authority("state:assign"),
        };
        assert_eq!(
            port.activate_profile(&case),
            Err(StateError::ConflictingOperation)
        );
        // A same-operation conflict leaves occupancy/classes unchanged.
        assert_eq!(
            port.active_occupants(&ProfileName::new("lead").unwrap())
                .unwrap(),
            vec![ActorId::new("lead-9").unwrap()]
        );
    }

    #[test]
    fn refused_activations_and_openings_register_nothing() {
        let state = fake_state();
        let port: &dyn WorkflowStatePort = &state;
        assert_eq!(
            port.activate_profile(&activation("a-1", "lead-1", "lead")),
            Ok(StateApplied::Applied)
        );
        // A second bootstrap is refused (one-shot), and must register
        // nothing: the rotation probe below proves it stayed unknown.
        assert_eq!(
            port.activate_profile(&activation("a-2", "lead-2", "lead")),
            Err(StateError::BootstrapAlreadyComplete)
        );
        assert_eq!(
            port.activate_profile(&activation_with(
                "a-3",
                "lead-2",
                "lead",
                ActivationCase::ActorAuthorizedRotation {
                    authority: authority("state:assign")
                }
            )),
            Err(StateError::UnknownActor)
        );
        // A conflicting opening must not register its worker.
        assert_eq!(port.open_assignment(&opening()), Ok(StateApplied::Applied));
        let mut other_worker = opening();
        other_worker.assignment.worker = DecisionActor {
            actor: ActorId::new("worker-77").unwrap(),
            class: AuthorityClass::Worker,
            profile: ProfileName::new("worker").unwrap(),
            profile_hash: ContentHash::new(&"a".repeat(64)).unwrap(),
        };
        assert_eq!(
            port.open_assignment(&other_worker),
            Err(StateError::ConflictingOperation)
        );
        assert_eq!(
            port.activate_profile(&activation_with(
                "a-4",
                "worker-77",
                "worker",
                ActivationCase::ActorAuthorizedRotation {
                    authority: authority("state:assign")
                }
            )),
            Err(StateError::UnknownActor)
        );
        // Only the occupant may deactivate (R5.10).
        assert_eq!(
            port.deactivate_profile(
                &op("op-evict"),
                &ActorId::new("intruder").unwrap(),
                &ProfileName::new("lead").unwrap()
            ),
            Err(StateError::NotTheOccupant)
        );
        assert_eq!(
            port.active_occupants(&ProfileName::new("lead").unwrap())
                .unwrap(),
            vec![ActorId::new("lead-1").unwrap()]
        );

        // An existing actor cannot silently change class.
        let mut reclassed = opening_for("asg-1", "att-1", "op-assign-2");
        reclassed.assignment.worker.class = AuthorityClass::Orchestrator;
        assert_eq!(
            port.open_assignment(&reclassed),
            Err(StateError::ActorClassMismatch)
        );

        // Deactivation inverse: lead-1 is still the active occupant
        // here; a conflicting same-operation deactivation naming a
        // DIFFERENT profile is refused and leaves it untouched.
        assert_eq!(
            port.deactivate_profile(
                &op("op-deact-2"),
                &ActorId::new("lead-1").unwrap(),
                &ProfileName::new("lead").unwrap()
            ),
            Ok(StateApplied::Applied)
        );
        assert_eq!(
            port.activate_profile(&activation_with(
                "a-7",
                "lead-1",
                "lead",
                ActivationCase::ActorAuthorizedRotation {
                    authority: authority("state:assign")
                }
            )),
            Ok(StateApplied::Applied)
        );
        assert_eq!(
            port.deactivate_profile(
                &op("op-deact-3"),
                &ActorId::new("lead-1").unwrap(),
                &ProfileName::new("worker").unwrap()
            ),
            Err(StateError::NotTheOccupant)
        );
        assert_eq!(
            port.active_occupants(&ProfileName::new("lead").unwrap())
                .unwrap(),
            vec![ActorId::new("lead-1").unwrap()]
        );
    }

    #[test]
    fn transfer_binds_a_full_recipient_snapshot() {
        let kind = DecisionKind::TransferAuthority {
            to: actor(),
            reason: reason("rebalance"),
        };
        match kind {
            DecisionKind::TransferAuthority { to, .. } => {
                assert_eq!(to.class, AuthorityClass::Orchestrator);
                assert_eq!(to.profile.as_str(), "lead");
            }
            _ => unreachable!(),
        }
    }

    #[test]
    fn decision_reasons_are_bounded() {
        assert!(DecisionReason::new("verified handoff").is_ok());
        assert_eq!(DecisionReason::new(""), Err(DecisionReasonError::Empty));
        assert_eq!(
            DecisionReason::new(&"x".repeat(201)),
            Err(DecisionReasonError::TooLong)
        );
        assert_eq!(
            DecisionReason::new("a\nb"),
            Err(DecisionReasonError::InvalidCharacter)
        );
    }

    struct FakeRuntime {
        live: RuntimeHandle,
    }

    impl FakeRuntime {
        fn fenced(&self, handle: &RuntimeHandle) -> Result<(), RuntimeError> {
            if handle != &self.live {
                return Err(RuntimeError::HandleStale);
            }
            Ok(())
        }
    }

    impl RuntimePort for FakeRuntime {
        fn launch(&self, _spec: &LaunchSpec) -> Result<LaunchAttempt, RuntimeError> {
            Ok(LaunchAttempt::Launched(LaunchOutcome {
                handle: self.live.clone(),
                observation: LivenessObservation {
                    observed_at: Timestamp(1),
                    kind: LivenessKind::Starting,
                },
                startup_delivery: StartupDelivery::Submitted,
            }))
        }

        fn recover_launch(
            &self,
            subject: &LaunchSubject,
            correlation: &LaunchCorrelation,
            _deadline: Timestamp,
        ) -> Result<Option<LaunchOutcome>, RuntimeError> {
            // Resolves from the pre-known key — no launch response is
            // needed — but the Attempt must match too (R5.17).
            // Recovery works for BOTH subject kinds, keyed by the
            // pre-known (subject, correlation) pair.
            let known = subject == &worker_subject("att-1")
                || subject == &actor_subject("lead-1", "lead", "act-1");
            if correlation == &LaunchCorrelation::new("corr-1").unwrap() && known {
                return Ok(Some(LaunchOutcome {
                    handle: self.live.clone(),
                    observation: LivenessObservation {
                        observed_at: Timestamp(2),
                        kind: LivenessKind::Running,
                    },
                    // Honest: no provider receipt proves the startup
                    // submission after a lost response.
                    startup_delivery: StartupDelivery::Ambiguous,
                }));
            }
            Ok(None)
        }

        fn observe(
            &self,
            handle: &RuntimeHandle,
            _deadline: Timestamp,
        ) -> Result<LivenessObservation, RuntimeError> {
            let kind = if handle == &self.live {
                LivenessKind::Running
            } else {
                LivenessKind::StaleGeneration
            };
            Ok(LivenessObservation {
                observed_at: Timestamp(1),
                kind,
            })
        }

        fn wait(
            &self,
            handle: &RuntimeHandle,
            _desired: LivenessKind,
            deadline: Timestamp,
        ) -> Result<LivenessObservation, RuntimeError> {
            self.observe(handle, deadline)
        }

        fn read_view(
            &self,
            handle: &RuntimeHandle,
            _max: u32,
            _deadline: Timestamp,
        ) -> Result<String, RuntimeError> {
            self.fenced(handle)?;
            Ok("view".into())
        }

        fn doorbell(
            &self,
            handle: &RuntimeHandle,
            _deadline: Timestamp,
        ) -> Result<DeliveryReport, RuntimeError> {
            self.fenced(handle)?;
            Ok(DeliveryReport::Submitted)
        }

        fn prompt(
            &self,
            handle: &RuntimeHandle,
            _text: &str,
            _deadline: Timestamp,
        ) -> Result<DeliveryReport, RuntimeError> {
            self.fenced(handle)?;
            Ok(DeliveryReport::Ambiguous)
        }

        fn control(
            &self,
            handle: &RuntimeHandle,
            _action: ControlAction,
            _deadline: Timestamp,
        ) -> Result<EffectReport, RuntimeError> {
            self.fenced(handle)?;
            Ok(EffectReport::Ambiguous)
        }

        fn stop(
            &self,
            handle: &RuntimeHandle,
            mode: StopMode,
            _deadline: Timestamp,
        ) -> Result<EffectReport, RuntimeError> {
            self.fenced(handle)?;
            Ok(match mode {
                StopMode::Graceful => EffectReport::Ambiguous,
                StopMode::Forced => EffectReport::Applied,
            })
        }

        fn reassociate(
            &self,
            _stale: &RuntimeHandle,
            _deadline: Timestamp,
        ) -> Result<RuntimeHandle, RuntimeError> {
            Ok(self.live.clone())
        }
    }

    /// A coherent second Assignment: every identity updated together.
    fn opening_for(assignment: &str, attempt: &str, operation: &str) -> AssignmentOpening {
        let mut o = opening();
        let asg = AssignmentId::new(assignment).unwrap();
        let att = AttemptId::new(attempt).unwrap();
        o.assignment.id = asg.clone();
        o.first_attempt.id = att.clone();
        o.first_attempt.assignment = asg.clone();
        o.authorizing.operation = op(operation);
        o.authorizing.assignment = asg;
        o.authorizing.first_attempt = att;
        o
    }

    fn worker_subject(attempt: &str) -> LaunchSubject {
        LaunchSubject::WorkerAttempt {
            attempt: AttemptId::new(attempt).unwrap(),
        }
    }

    fn actor_subject(actor: &str, profile: &str, generation: &str) -> LaunchSubject {
        LaunchSubject::ActorActivation {
            actor: ActorId::new(actor).unwrap(),
            profile: ProfileName::new(profile).unwrap(),
            generation: op(generation),
        }
    }

    fn spec() -> LaunchSpec {
        LaunchSpec {
            subject: worker_subject("att-1"),
            agent_kind: "claude".into(),
            executable: "/usr/local/bin/agent".into(),
            args: vec!["--project".into()],
            correlation: LaunchCorrelation::new("corr-1").unwrap(),
            working_directory: HostPath::new("/home/user/worktrees/abacus-x").unwrap(),
            environment: BTreeMap::from([("ABACUS_SOCKET".into(), "path".into())]),
            envelope: EnvelopeSnapshot {
                content: "envelope".into(),
                content_hash: ContentHash::new(&"e".repeat(64)).unwrap(),
            },
            startup_deadline: Timestamp(100),
            delivery_deadline: Timestamp(200),
        }
    }

    #[test]
    fn runtime_control_preserves_ambiguity_and_fences() {
        let runtime = FakeRuntime {
            live: RuntimeHandle::new("gen-2-token"),
        };
        let port: &dyn RuntimePort = &runtime;
        let attempt_outcome = port.launch(&spec()).unwrap();
        let LaunchAttempt::Launched(outcome) = attempt_outcome else {
            panic!("expected launched")
        };
        assert_eq!(outcome.observation.kind, LivenessKind::Starting);
        assert_eq!(outcome.startup_delivery, StartupDelivery::Submitted);
        let live = outcome.handle;
        let stale = RuntimeHandle::new("gen-1-token");
        let d = Timestamp(5);

        assert_eq!(
            port.observe(&stale, d).unwrap().kind,
            LivenessKind::StaleGeneration
        );
        assert_eq!(port.doorbell(&stale, d), Err(RuntimeError::HandleStale));
        assert_eq!(
            port.prompt(&stale, "hello", d),
            Err(RuntimeError::HandleStale)
        );
        assert_eq!(
            port.control(&stale, ControlAction::CancelBlockedDialog, d),
            Err(RuntimeError::HandleStale)
        );
        assert_eq!(
            port.stop(&stale, StopMode::Graceful, d),
            Err(RuntimeError::HandleStale)
        );

        assert_eq!(
            port.control(&live, ControlAction::CancelBlockedDialog, d),
            Ok(EffectReport::Ambiguous)
        );
        assert_eq!(
            port.stop(&live, StopMode::Graceful, d),
            Ok(EffectReport::Ambiguous)
        );
        assert_eq!(
            port.stop(&live, StopMode::Forced, d),
            Ok(EffectReport::Applied)
        );

        let recovered = port.reassociate(&stale, d).unwrap();
        assert_eq!(recovered, live);
    }

    /// R5.11: the launch response is DISCARDED, and recovery still
    /// works from the pre-known correlation in the LaunchSpec.
    #[test]
    fn ambiguous_launch_recovers_without_reading_the_response() {
        let runtime = FakeRuntime {
            live: RuntimeHandle::new("gen-2-token"),
        };
        let port: &dyn RuntimePort = &runtime;
        let spec = spec();
        let _discarded = port.launch(&spec);
        let recovered = port
            .recover_launch(&spec.subject, &spec.correlation, Timestamp(9))
            .unwrap()
            .expect("recovers from the pre-known key");
        // Honest recovery: no provider receipt proves the startup
        // submission, so delivery is Ambiguous — with a handle, which
        // is what lets the caller stop and revoke (R5.17).
        assert_eq!(recovered.startup_delivery, StartupDelivery::Ambiguous);
        // A right correlation with the WRONG Attempt must not rebind.
        assert!(
            port.recover_launch(&worker_subject("att-9"), &spec.correlation, Timestamp(9))
                .unwrap()
                .is_none()
        );
        let unknown = LaunchCorrelation::new("corr-absent").unwrap();
        assert!(
            port.recover_launch(&spec.subject, &unknown, Timestamp(9))
                .unwrap()
                .is_none()
        );
        assert!(LaunchCorrelation::new("BAD").is_err());
    }

    /// A retry Attempt's locator resolves, and a conflicting retry
    /// does not disturb the installed durable relation.
    #[test]
    fn retry_attempt_locator_is_installed_and_resolvable() {
        let state = fake_state();
        let port: &dyn WorkflowStatePort = &state;
        assert_eq!(port.open_assignment(&opening()), Ok(StateApplied::Applied));
        let retry = AttemptOpening {
            authorizing: RetryDecision {
                operation: op("op-retry"),
                assignment: AssignmentId::new("asg-1").unwrap(),
                authority: authority("state:assign"),
                reason: reason("previous attempt expired"),
            },
            attempt: AttemptRecord {
                id: AttemptId::new("att-2").unwrap(),
                assignment: AssignmentId::new("asg-1").unwrap(),
                lease: Lease {
                    token: FencingToken(2),
                    expires_at: Timestamp(200),
                },
            },
        };
        assert_eq!(port.append_attempt(&retry), Ok(StateApplied::Applied));
        assert_eq!(port.runtime_handle(&worker_subject("att-2")), Ok(None));
        // A conflicting retry under the same operation leaves the
        // installed successor relation intact.
        let mut conflicting = retry;
        conflicting.attempt.lease.expires_at = Timestamp(201);
        assert_eq!(
            port.append_attempt(&conflicting),
            Err(StateError::ConflictingOperation)
        );
        assert_eq!(port.runtime_handle(&worker_subject("att-2")), Ok(None));
    }

    /// R5.10: shared profiles (the normal ten-worker case) have real
    /// membership — every occupant is listed, only an active member may
    /// deactivate, and release removes exactly that actor.
    #[test]
    fn shared_profile_membership_and_ownership() {
        let state = fake_state();
        let port: &dyn WorkflowStatePort = &state;
        assert_eq!(port.open_assignment(&opening()), Ok(StateApplied::Applied));
        // Two distinct workers occupy the shared profile.
        let w1 = activation_with(
            "s-1",
            "worker-1",
            "worker",
            ActivationCase::ActorAuthorizedRotation {
                authority: authority("state:assign"),
            },
        );
        assert_eq!(port.activate_profile(&w1), Ok(StateApplied::Applied));
        let mut second = opening_for("asg-3", "att-3", "op-assign-b");
        second.assignment.worker = DecisionActor {
            actor: ActorId::new("worker-2").unwrap(),
            class: AuthorityClass::Worker,
            profile: ProfileName::new("worker").unwrap(),
            profile_hash: ContentHash::new(&"a".repeat(64)).unwrap(),
        };
        assert_eq!(port.open_assignment(&second), Ok(StateApplied::Applied));
        let w2 = activation_with(
            "s-2",
            "worker-2",
            "worker",
            ActivationCase::ActorAuthorizedRotation {
                authority: authority("state:assign"),
            },
        );
        assert_eq!(port.activate_profile(&w2), Ok(StateApplied::Applied));
        let worker_profile = ProfileName::new("worker").unwrap();
        assert_eq!(
            port.active_occupants(&worker_profile).unwrap(),
            vec![
                ActorId::new("worker-1").unwrap(),
                ActorId::new("worker-2").unwrap()
            ]
        );
        // A non-member cannot deactivate, and leaves no residue.
        assert_eq!(
            port.deactivate_profile(
                &op("d-x"),
                &ActorId::new("worker-9").unwrap(),
                &worker_profile
            ),
            Err(StateError::NotTheOccupant)
        );
        assert_eq!(port.active_occupants(&worker_profile).unwrap().len(), 2);
        // Releasing one member leaves the co-occupant active.
        assert_eq!(
            port.deactivate_profile(
                &op("d-w1"),
                &ActorId::new("worker-1").unwrap(),
                &worker_profile
            ),
            Ok(StateApplied::Applied)
        );
        assert_eq!(
            port.active_occupants(&worker_profile).unwrap(),
            vec![ActorId::new("worker-2").unwrap()]
        );
    }

    /// Ended Attempts fail with the stale-fencing taxonomy while an
    /// unrelated Assignment remains writable.
    #[test]
    fn ended_attempt_is_stale_without_collateral() {
        let state = fake_state();
        let port: &dyn WorkflowStatePort = &state;
        assert_eq!(port.open_assignment(&opening()), Ok(StateApplied::Applied));
        let mut other = opening_for("asg-2", "att-7", "op-assign-c");
        other.assignment.worker.actor = ActorId::new("worker-2").unwrap();
        assert_eq!(port.open_assignment(&other), Ok(StateApplied::Applied));
        let revoke = DecisionRecord {
            operation: op("op-revoke"),
            assignment: AssignmentId::new("asg-1").unwrap(),
            authority: authority("state:assign"),
            kind: DecisionKind::Revoke {
                attempt: AttemptId::new("att-1").unwrap(),
                reason: reason("worker unresponsive"),
            },
            resolves: None,
        };
        assert_eq!(port.record_decision(&revoke), Ok(StateApplied::Applied));
        let decision_events = port
            .audit_events(&AuditQuery {
                class: Some(AuditClass::Decision),
                ..AuditQuery::default()
            })
            .unwrap();
        assert_eq!(decision_events.len(), 1);
        assert_eq!(
            decision_events[0].subject,
            AuditSubject::Workflow(SubjectRef::Attempt(AttemptId::new("att-1").unwrap()))
        );
        assert_eq!(
            *state.response_actions.borrow(),
            vec![
                ResponseAction {
                    seq: decision_events[0].seq,
                    kind: ResponseKind::FencedDecision { responds_to: None },
                },
                ResponseAction {
                    seq: decision_events[0].seq,
                    kind: ResponseKind::TerminalAttemptAction {
                        attempt: AttemptId::new("att-1").unwrap(),
                        abort_consistent: true,
                    },
                },
            ],
            "a decision terminal and its audit share one causal position"
        );
        assert_eq!(
            port.fenced_evidence(&good_action("op-after-revoke"), &passing_evidence()),
            Err(StateError::StaleFencing),
            "an ended Attempt is a stale predecessor, not an incoherent bundle"
        );
        let other_action = FencedAction {
            call: FencedCall {
                attempt: AttemptId::new("att-7").unwrap(),
                token: FencingToken(3),
                operation: op("op-other-worker"),
            },
            responds_to: None,
        };
        assert_eq!(
            port.fenced_evidence(&other_action, &passing_evidence())
                .unwrap()
                .0,
            EvidenceOutcome::Recorded
        );
    }

    /// Rotation fences the prior generation for that
    /// actor+profile and leaves everything else alone.
    #[test]
    fn rotation_revokes_only_the_prior_generation() {
        let state = fake_state();
        let port: &dyn WorkflowStatePort = &state;
        assert_eq!(
            port.activate_profile(&activation("act-1", "lead-1", "lead")),
            Ok(StateApplied::Applied)
        );
        let gen1 = actor_subject("lead-1", "lead", "act-1");
        assert_eq!(port.runtime_handle(&gen1), Ok(None));
        assert_eq!(
            port.activate_profile(&activation_with(
                "act-2",
                "lead-1",
                "lead",
                ActivationCase::ActorAuthorizedRotation {
                    authority: authority("state:assign")
                }
            )),
            Ok(StateApplied::Applied)
        );
        // Old generation dead, new generation valid.
        assert_eq!(port.runtime_handle(&gen1), Err(StateError::StaleFencing));
        assert_eq!(
            port.runtime_handle(&actor_subject("lead-1", "lead", "act-2")),
            Ok(None)
        );
    }

    /// R5.19: a spawned orchestrator/watchdog profile is launchable
    /// and associable through the same closed subject.
    #[test]
    fn orchestrator_profiles_are_launchable_subjects() {
        let state = fake_state();
        let port: &dyn WorkflowStatePort = &state;
        let activation = activation("act-1", "lead-1", "lead");
        assert_eq!(
            port.activate_profile(&activation),
            Ok(StateApplied::Applied)
        );
        let subject = actor_subject("lead-1", "lead", "act-1");
        // Envelope persistence and handle association accept it too.
        assert_eq!(
            port.persist_envelope(
                &op("env-1"),
                &subject,
                &EnvelopeSnapshot::new(
                    "profile envelope".into(),
                    ContentHash::new(&"e".repeat(64)).unwrap()
                )
                .unwrap()
            ),
            Ok(StateApplied::Applied)
        );
        // Read back the EXACT Envelope; it is immutable once written.
        assert_eq!(
            port.envelope(&subject).unwrap().content(),
            "profile envelope"
        );
        assert_eq!(
            port.persist_envelope(
                &op("env-2"),
                &subject,
                &EnvelopeSnapshot::new("other".into(), ContentHash::new(&"f".repeat(64)).unwrap())
                    .unwrap()
            ),
            Err(StateError::ConflictingOperation)
        );
        // Handle: bind, read back exactly, unbind to None.
        let handle = RuntimeHandle::new("gen-1");
        assert_eq!(
            port.bind_runtime_handle(&op("bind-1"), &subject, &handle),
            Ok(StateApplied::Applied)
        );
        assert_eq!(port.runtime_handle(&subject).unwrap(), Some(handle));
        // Cross-subject isolation: an unrelated worker subject is not
        // even resolvable here, let alone able to read these records.
        let worker = worker_subject("att-1");
        assert_eq!(port.runtime_handle(&worker), Err(StateError::UnknownRecord));
        assert_eq!(port.envelope(&worker), Err(StateError::UnknownRecord));
        assert_eq!(
            port.unbind_runtime_handle(&op("unbind-1"), &subject),
            Ok(StateApplied::Applied)
        );
        assert_eq!(port.runtime_handle(&subject).unwrap(), None);
    }

    /// R5.23 #1: an unknown Attempt locator must never alias a real
    /// subject's association.
    #[test]
    fn unknown_attempt_cannot_alias_associations() {
        let state = fake_state();
        let port: &dyn WorkflowStatePort = &state;
        assert_eq!(port.open_assignment(&opening()), Ok(StateApplied::Applied));
        let right = worker_subject("att-1");
        let unknown = worker_subject("att-nope");
        let envelope =
            EnvelopeSnapshot::new("real".into(), ContentHash::new(&"e".repeat(64)).unwrap())
                .unwrap();
        assert_eq!(
            port.persist_envelope(&op("env-1"), &right, &envelope),
            Ok(StateApplied::Applied)
        );
        assert_eq!(
            port.bind_runtime_handle(&op("bind-1"), &right, &RuntimeHandle::new("h-real")),
            Ok(StateApplied::Applied)
        );
        // The unknown locator is refused outright — no read, orphan
        // record, or mutation of the real association.
        assert_eq!(port.envelope(&unknown), Err(StateError::UnknownRecord));
        assert_eq!(
            port.runtime_handle(&unknown),
            Err(StateError::UnknownRecord)
        );
        assert_eq!(
            port.bind_runtime_handle(&op("bind-2"), &unknown, &RuntimeHandle::new("h-fake")),
            Err(StateError::UnknownRecord)
        );
        assert_eq!(
            port.unbind_runtime_handle(&op("unbind-1"), &unknown),
            Err(StateError::UnknownRecord)
        );
        assert_eq!(
            port.runtime_handle(&right).unwrap(),
            Some(RuntimeHandle::new("h-real"))
        );
    }

    /// R5.23 #2: verb-scoped idempotency — exact replay returns the
    /// stored result without reapplying, and a stale replay cannot
    /// disturb a later association.
    #[test]
    fn association_operations_are_idempotent_and_stale_replay_is_inert() {
        let state = fake_state();
        let port: &dyn WorkflowStatePort = &state;
        assert_eq!(port.open_assignment(&opening()), Ok(StateApplied::Applied));
        let second = opening_for("asg-2", "att-2", "op-assign-2");
        assert_eq!(port.open_assignment(&second), Ok(StateApplied::Applied));
        let subject = worker_subject("att-1");
        let h1 = RuntimeHandle::new("h1");
        let h2 = RuntimeHandle::new("h2");
        assert_eq!(
            port.bind_runtime_handle(&op("op-old"), &subject, &h1),
            Ok(StateApplied::Applied)
        );
        assert_eq!(
            port.unbind_runtime_handle(&op("op-u"), &subject),
            Ok(StateApplied::Applied)
        );
        assert_eq!(
            port.bind_runtime_handle(&op("op-new"), &subject, &h2),
            Ok(StateApplied::Applied)
        );
        // Stale bind replay: absorbed, and h1 is NOT resurrected.
        assert_eq!(
            port.bind_runtime_handle(&op("op-old"), &subject, &h1),
            Ok(StateApplied::AlreadyApplied)
        );
        assert_eq!(port.runtime_handle(&subject).unwrap(), Some(h2.clone()));
        // Stale unbind replay: absorbed, and h2 is NOT removed.
        assert_eq!(
            port.unbind_runtime_handle(&op("op-u"), &subject),
            Ok(StateApplied::AlreadyApplied)
        );
        assert_eq!(port.runtime_handle(&subject).unwrap(), Some(h2));
        // Same verb+operation against a DIFFERENT subject conflicts.
        assert_eq!(
            port.bind_runtime_handle(&op("op-new"), &worker_subject("att-2"), &h1),
            Err(StateError::ConflictingOperation)
        );
    }

    /// R5.23 #3: incoherent bundles refuse before any side effect.
    #[test]
    fn incoherent_bundles_are_refused() {
        let state = fake_state();
        let port: &dyn WorkflowStatePort = &state;
        let mut bad_assignment = opening();
        bad_assignment.authorizing.assignment = AssignmentId::new("asg-other").unwrap();
        assert_eq!(
            port.open_assignment(&bad_assignment),
            Err(StateError::IncoherentBundle)
        );
        let mut bad_attempt = opening();
        bad_attempt.authorizing.first_attempt = AttemptId::new("att-other").unwrap();
        assert_eq!(
            port.open_assignment(&bad_attempt),
            Err(StateError::IncoherentBundle)
        );
        // No side effects: the Assignment never opened.
        assert_eq!(
            port.runtime_handle(&worker_subject("att-1")),
            Err(StateError::UnknownRecord)
        );
        // Retry bundles are checked too.
        assert_eq!(port.open_assignment(&opening()), Ok(StateApplied::Applied));
        let mut bad_retry = AttemptOpening {
            authorizing: RetryDecision {
                operation: op("op-retry"),
                assignment: AssignmentId::new("asg-other").unwrap(),
                authority: authority("state:assign"),
                reason: reason("mismatched"),
            },
            attempt: AttemptRecord {
                id: AttemptId::new("att-2").unwrap(),
                assignment: AssignmentId::new("asg-1").unwrap(),
                lease: Lease {
                    token: FencingToken(2),
                    expires_at: Timestamp(200),
                },
            },
        };
        assert_eq!(
            port.append_attempt(&bad_retry),
            Err(StateError::IncoherentBundle)
        );
        bad_retry.authorizing.assignment = AssignmentId::new("asg-1").unwrap();
        assert_eq!(port.append_attempt(&bad_retry), Ok(StateApplied::Applied));
        // An Accept naming an unknown Handoff refuses before recording.
        let accept_unknown = DecisionRecord {
            operation: op("op-accept-x"),
            assignment: AssignmentId::new("asg-1").unwrap(),
            authority: authority("state:accept"),
            kind: DecisionKind::Accept {
                handoff: HandoffId::new("h-absent").unwrap(),
                reason: reason("no such handoff"),
            },
            resolves: None,
        };
        assert_eq!(
            port.record_decision(&accept_unknown),
            Err(StateError::UnknownRecord)
        );
        // The refused Accept created no projection; only the opening's
        // mark-in-progress is pending.
        let pending = port.pending_applications().unwrap();
        assert!(pending.iter().all(|p| p.operation != op("op-accept-x")));
        assert!(
            pending
                .iter()
                .all(|p| p.projection == WorkProjection::MarkInProgress)
        );
    }

    /// R5.23: an actor activation is launchable and recoverable
    /// through the same runtime contract, not only persistable.
    #[test]
    fn actor_activation_launches_and_recovers() {
        let runtime = FakeRuntime {
            live: RuntimeHandle::new("gen-2-token"),
        };
        let port: &dyn RuntimePort = &runtime;
        let subject = actor_subject("lead-1", "lead", "act-1");
        let mut spec = spec();
        spec.subject = subject.clone();
        spec.agent_kind = "claude".into();
        let attempt = port.launch(&spec).unwrap();
        assert!(matches!(attempt, LaunchAttempt::Launched(_)));
        // Ambiguous recovery resolves for the activation subject too.
        let recovered = port
            .recover_launch(&subject, &spec.correlation, Timestamp(9))
            .unwrap()
            .expect("activation recovers from its pre-known key");
        assert_eq!(recovered.startup_delivery, StartupDelivery::Ambiguous);
    }

    /// R5.24: the fenced seam binds the Attempt locator, token, and
    /// lease while resolving Assignment and worker authority itself.
    #[test]
    fn fenced_calls_are_fully_bound() {
        let state = fake_state();
        let port: &dyn WorkflowStatePort = &state;
        let evidence = crate::evidence::Evidence::new(
            Argv::new(vec!["cargo".into(), "test".into()]).unwrap(),
            VerificationSet::new(
                vec![Argv::new(vec!["cargo".into(), "test".into()]).unwrap()],
                PathSet::new(vec![WorkPath::new("tests/a.rs").unwrap()]).unwrap(),
            )
            .unwrap(),
            0,
            VerificationOutcome::Pass,
            CommitId::new(&"c".repeat(40)).unwrap(),
            crate::content::WorkspaceDigest::new(&"1".repeat(64)).unwrap(),
            crate::content::WorkspaceDigest::new(&"1".repeat(64)).unwrap(),
            None,
            FileDigestSet::default(),
            None,
        )
        .unwrap();

        // Unknown Attempt locator → loud refusal.
        let foreign_attempt = FencedAction {
            call: FencedCall {
                attempt: AttemptId::new("att-other").unwrap(),
                ..good_call("op-e2")
            },
            responds_to: None,
        };
        assert_eq!(
            port.fenced_evidence(&foreign_attempt, &evidence),
            Err(StateError::UnknownRecord)
        );
        // Stale token → distinct refusal.
        let stale = FencedAction {
            call: FencedCall {
                token: FencingToken(2),
                ..good_call("op-e3")
            },
            responds_to: None,
        };
        assert_eq!(
            port.fenced_evidence(&stale, &evidence),
            Err(StateError::StaleFencing)
        );
        // A Handoff for another Attempt cannot ride this call.
        let other_handoff = HandoffRecord {
            attempt: AttemptId::new("att-other").unwrap(),
            ..handoff("h-x", vec![op("op-evi")])
        };
        assert_eq!(
            port.fenced_submit_handoff(&good_action("op-h9"), &other_handoff),
            Err(StateError::IncoherentBundle)
        );
        // ReportDraft has no caller-supplied sender, subject, or
        // authority fields; those facts are resolved by Scribe.
        // Renewal is owned by its operation: exact replay is absorbed,
        // a different deadline under the same operation conflicts.
        assert_eq!(
            port.renew_lease(&good_call("op-renew"), Timestamp(200))
                .unwrap()
                .1
                .applied,
            StateApplied::Applied
        );
        assert_eq!(
            port.renew_lease(&good_call("op-renew"), Timestamp(200))
                .unwrap()
                .1
                .applied,
            StateApplied::AlreadyApplied
        );
        assert_eq!(
            port.renew_lease(&good_call("op-renew"), Timestamp(300))
                .err(),
            Some(StateError::ConflictingOperation)
        );

        // Renewal PERSISTS: at now=101 the old 100 deadline would have
        // expired, but the renewal to 200 holds (R5.27).
        *state.now.borrow_mut() = Timestamp(101);
        assert_eq!(
            port.fenced_evidence(&good_action("op-e4"), &evidence)
                .unwrap()
                .1
                .applied,
            StateApplied::Applied
        );
        // Past the renewed deadline it expires.
        *state.now.borrow_mut() = Timestamp(201);
        assert_eq!(
            port.fenced_evidence(&good_action("op-e5"), &evidence),
            Err(StateError::LeaseExpired)
        );
        // Committed replays still succeed AFTER expiry: the stored
        // outcome is returned without re-mutating (R5.27).
        assert_eq!(
            port.fenced_evidence(&good_action("op-e4"), &evidence)
                .unwrap()
                .1
                .applied,
            StateApplied::AlreadyApplied
        );
        assert_eq!(
            port.renew_lease(&good_call("op-renew"), Timestamp(200))
                .unwrap()
                .1
                .applied,
            StateApplied::AlreadyApplied
        );
    }

    /// R5.26: receipts are linked to an exact successful attempt;
    /// close projections derive real facts; pending is causally
    /// ordered.
    #[test]
    fn projection_saga_is_linked_ordered_and_typed() {
        let state = fake_state();
        let port: &dyn WorkflowStatePort = &state;
        // Two Assignments; the SECOND is accepted, so its close must
        // name the second bead — no parser, no fallback.
        assert_eq!(port.open_assignment(&opening()), Ok(StateApplied::Applied));
        let mut second = opening_for("asg-2", "att-2", "zz-open");
        second.assignment.bead = BeadId::new("ABACUS-second").unwrap();
        assert_eq!(port.open_assignment(&second), Ok(StateApplied::Applied));
        let cancel_second = DecisionRecord {
            operation: op("aa-cancel"),
            assignment: AssignmentId::new("asg-2").unwrap(),
            authority: authority("state:assign"),
            kind: DecisionKind::Cancel {
                reason: reason("obsolete"),
            },
            resolves: None,
        };
        assert_eq!(
            port.record_decision(&cancel_second),
            Ok(StateApplied::Applied)
        );
        let pending = port.pending_applications().unwrap();
        let close = pending
            .iter()
            .find(|p| matches!(p.projection, WorkProjection::Close { .. }))
            .expect("close projection");
        assert_eq!(close.bead.as_str(), "ABACUS-second");
        assert_eq!(close.assignment.as_str(), "asg-2");
        // Close carries no fabricated revision.
        assert_eq!(close.authorized_revision, None);
        // Causal order despite REVERSE-lexical operation ids: the
        // opening is "zz-open" and the later close is "aa-cancel", so
        // key ordering alone would invert the saga.
        assert_eq!(
            pending.first().unwrap().projection,
            WorkProjection::MarkInProgress
        );
        assert!(
            pending
                .windows(2)
                .all(|w| w[0].committed_at <= w[1].committed_at)
        );

        // A decision naming an unknown Assignment never commits.
        let foreign = DecisionRecord {
            operation: op("op-foreign"),
            assignment: AssignmentId::new("asg-nope").unwrap(),
            authority: authority("state:assign"),
            kind: DecisionKind::Cancel {
                reason: reason("nonexistent"),
            },
            resolves: None,
        };
        assert_eq!(
            port.record_decision(&foreign),
            Err(StateError::UnknownRecord)
        );

        // Receipt linkage: wrong attempt id and wrong revision refuse.
        assert_eq!(
            port.record_application_attempt(&ApplicationAttempt {
                id: op("app-ok"),
                target: op("aa-cancel"),
                outcome: ApplicationOutcome::Applied {
                    before: rev('a'),
                    after: rev('f'),
                },
            }),
            Ok(StateApplied::Applied)
        );
        assert_eq!(
            port.record_application_receipt(&ApplicationReceipt {
                target: op("aa-cancel"),
                attempt: op("app-absent"),
                after: rev('f'),
            }),
            Err(StateError::IncoherentBundle)
        );
        assert_eq!(
            port.record_application_receipt(&ApplicationReceipt {
                target: op("aa-cancel"),
                attempt: op("app-ok"),
                after: rev('9'),
            }),
            Err(StateError::IncoherentBundle)
        );
        // A refused receipt must not clear the derived pending set.
        assert!(
            port.pending_applications()
                .unwrap()
                .iter()
                .any(|p| p.operation == op("aa-cancel"))
        );
        // The exact linked receipt clears exactly that projection.
        assert_eq!(
            port.record_application_receipt(&ApplicationReceipt {
                target: op("aa-cancel"),
                attempt: op("app-ok"),
                after: rev('f'),
            }),
            Ok(StateApplied::Applied)
        );
        let remaining = port.pending_applications().unwrap();
        assert!(remaining.iter().all(|p| p.operation != op("aa-cancel")));
    }

    /// R5.27: a VALID worker Report succeeds, its call operation owns
    /// the result, and a different draft under that operation
    /// conflicts.
    #[test]
    fn valid_report_succeeds_and_is_call_owned() {
        let state = fake_state();
        let port: &dyn WorkflowStatePort = &state;
        let draft = report_draft("sig-ok");
        let (outcome, response) = port.fenced_report(&good_action("op-rep"), &draft).unwrap();
        let ReportOutcome::Recorded { signal } = outcome else {
            panic!("valid report was refused")
        };
        assert_eq!(signal.seq, Seq(1));
        assert_eq!(response.applied, StateApplied::Applied);
        // Exact replay of the same call is absorbed.
        let (again, replay) = port.fenced_report(&good_action("op-rep"), &draft).unwrap();
        assert_eq!(again, ReportOutcome::Recorded { signal });
        assert_eq!(replay.applied, StateApplied::AlreadyApplied);
        // A DIFFERENT draft under the same call operation conflicts —
        // SignalId is record identity, not call ownership.
        let other = report_draft("sig-other");
        assert_eq!(
            port.fenced_report(&good_action("op-rep"), &other).err(),
            Some(StateError::ConflictingOperation)
        );
    }

    #[test]
    fn every_fenced_response_surfaces_causally_current_directives() {
        let state = fake_state();
        let port: &dyn WorkflowStatePort = &state;
        let amend = directive_draft(
            "dir-current-1",
            "att-1",
            DirectiveKind::Amend {
                instruction: BoundedText::new("update the implementation").unwrap(),
            },
        );
        port.append_signal(&amend).unwrap();

        let (_, renewal) = port
            .renew_lease(&good_call("op-current-renew"), Timestamp(150))
            .unwrap();
        assert_eq!(
            renewal
                .binding_directives
                .iter()
                .map(|signal| signal.id.as_str())
                .collect::<Vec<_>>(),
            vec!["dir-current-1"]
        );

        let (_, report) = port
            .fenced_report(
                &good_action("op-current-report"),
                &report_draft("sig-current"),
            )
            .unwrap();
        assert_eq!(
            report
                .binding_directives
                .iter()
                .map(|signal| signal.id.as_str())
                .collect::<Vec<_>>(),
            vec!["dir-current-1"]
        );

        let evidence = passing_evidence();
        let evidence_action = good_action("op-current-evidence");
        let (evidence_outcome, evidence_response) =
            port.fenced_evidence(&evidence_action, &evidence).unwrap();
        assert_eq!(evidence_outcome, EvidenceOutcome::Recorded);
        assert_eq!(
            evidence_response
                .binding_directives
                .iter()
                .map(|signal| signal.id.as_str())
                .collect::<Vec<_>>(),
            vec!["dir-current-1"]
        );
        let action_count = worker_action_count(&state);

        // Replaying after another Directive commits allocates no new
        // action position, but still returns the current causal view.
        let second = directive_draft(
            "dir-current-2",
            "att-1",
            DirectiveKind::Amend {
                instruction: BoundedText::new("also update the regression").unwrap(),
            },
        );
        port.append_signal(&second).unwrap();
        let (replay_outcome, replay) = port.fenced_evidence(&evidence_action, &evidence).unwrap();
        assert_eq!(replay_outcome, EvidenceOutcome::Recorded);
        assert_eq!(replay.applied, StateApplied::AlreadyApplied);
        assert_eq!(worker_action_count(&state), action_count);
        assert_eq!(replay.head, state.current_head());
        assert_eq!(
            replay
                .binding_directives
                .iter()
                .map(|signal| signal.id.as_str())
                .collect::<Vec<_>>(),
            vec!["dir-current-1", "dir-current-2"]
        );

        let (outcome, handoff_response) = port
            .fenced_submit_handoff(
                &good_action("op-current-handoff"),
                &handoff("h-current", vec![op("op-current-evidence")]),
            )
            .unwrap();
        assert_eq!(
            outcome,
            SubmissionOutcome::Refused {
                reason: SubmissionRefusalReason::Directive(DirectiveGateRefusal::AmendUndischarged)
            }
        );
        assert_eq!(
            handoff_response
                .binding_directives
                .iter()
                .map(|signal| signal.id.as_str())
                .collect::<Vec<_>>(),
            vec!["dir-current-1", "dir-current-2"]
        );
    }

    #[test]
    fn abort_refuses_report_and_evidence_as_operation_owned_outcomes() {
        let state = fake_state();
        let port: &dyn WorkflowStatePort = &state;
        let abort_id = SignalId::new("dir-abort-appends").unwrap();
        port.append_signal(&directive_draft(
            abort_id.as_str(),
            "att-1",
            DirectiveKind::Abort {
                reason: BoundedText::new("stop substantive work").unwrap(),
            },
        ))
        .unwrap();

        let report_action = good_action("op-abort-report");
        let report = report_draft("sig-must-not-commit");
        let (report_outcome, report_response) =
            port.fenced_report(&report_action, &report).unwrap();
        assert_eq!(
            report_outcome,
            ReportOutcome::Refused {
                reason: DirectiveGateRefusal::AbortInForce
            }
        );
        assert_eq!(report_response.applied, StateApplied::Applied);
        assert_eq!(report_response.head, Seq(2));
        assert_eq!(report_response.binding_directives[0].id, abort_id.clone());
        assert!(
            state
                .stored_signals
                .borrow()
                .iter()
                .all(|signal| signal.id != report.id)
        );
        assert_eq!(worker_action_count(&state), 0);

        let evidence_action = good_action("op-abort-evidence");
        let evidence = passing_evidence();
        let (evidence_outcome, evidence_response) =
            port.fenced_evidence(&evidence_action, &evidence).unwrap();
        assert_eq!(
            evidence_outcome,
            EvidenceOutcome::Refused {
                reason: DirectiveGateRefusal::AbortInForce
            }
        );
        assert_eq!(evidence_response.applied, StateApplied::Applied);
        assert_eq!(evidence_response.head, Seq(3));
        assert_eq!(evidence_response.binding_directives[0].id, abort_id);
        assert!(state.evidence_records.borrow().is_empty());
        assert_eq!(worker_action_count(&state), 0);

        // A newer Directive makes the causal envelope newer without
        // changing either stored refusal or allocating a replay call.
        port.append_signal(&directive_draft(
            "dir-after-refusal",
            "att-1",
            DirectiveKind::Amend {
                instruction: BoundedText::new("record this on recovery").unwrap(),
            },
        ))
        .unwrap();
        let head = state.current_head();
        let signal_count = state.stored_signals.borrow().len();

        let (report_replay_outcome, report_replay) =
            port.fenced_report(&report_action, &report).unwrap();
        assert_eq!(report_replay_outcome, report_outcome);
        assert_eq!(report_replay.applied, StateApplied::AlreadyApplied);
        assert_eq!(report_replay.head, head);
        assert_eq!(
            report_replay
                .binding_directives
                .iter()
                .map(|signal| signal.id.as_str())
                .collect::<Vec<_>>(),
            vec!["dir-abort-appends", "dir-after-refusal"]
        );

        let (evidence_replay_outcome, evidence_replay) =
            port.fenced_evidence(&evidence_action, &evidence).unwrap();
        assert_eq!(evidence_replay_outcome, evidence_outcome);
        assert_eq!(evidence_replay.applied, StateApplied::AlreadyApplied);
        assert_eq!(evidence_replay.head, head);
        assert_eq!(
            evidence_replay
                .binding_directives
                .iter()
                .map(|signal| signal.id.as_str())
                .collect::<Vec<_>>(),
            vec!["dir-abort-appends", "dir-after-refusal"]
        );
        assert_eq!(state.current_head(), head);
        assert_eq!(state.stored_signals.borrow().len(), signal_count);
        assert!(state.evidence_records.borrow().is_empty());
        assert_eq!(worker_action_count(&state), 0);

        // The response link participates in identity even when the
        // operation owns a refusal rather than a recorded payload.
        let differently_linked = FencedAction {
            responds_to: Some(SignalId::new("dir-abort-appends").unwrap()),
            ..report_action
        };
        assert_eq!(
            port.fenced_report(&differently_linked, &report),
            Err(StateError::ConflictingOperation)
        );
        assert_eq!(state.current_head(), head);
    }

    #[test]
    fn explicit_abort_terminal_is_causal_idempotent_and_audited() {
        let state = fake_state();
        let port: &dyn WorkflowStatePort = &state;
        let call = good_call("op-explicit-abort");
        assert_eq!(
            port.fenced_abort_attempt(&call),
            Err(StateError::AbortNotInForce)
        );
        assert!(
            port.audit_events(&AuditQuery::default())
                .unwrap()
                .is_empty()
        );

        port.append_signal(&directive_draft(
            "dir-explicit-abort",
            "att-1",
            DirectiveKind::Abort {
                reason: BoundedText::new("stop this attempt").unwrap(),
            },
        ))
        .unwrap();
        let response = port.fenced_abort_attempt(&call).unwrap();
        assert_eq!(response.applied, StateApplied::Applied);
        assert!(response.binding_directives.is_empty());
        assert_eq!(
            state.attempt_states.borrow().get("att-1"),
            Some(&AttemptState::Aborted)
        );
        let events = port
            .audit_events(&AuditQuery {
                class: Some(AuditClass::Attempt),
                ..AuditQuery::default()
            })
            .unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].seq, response.head);
        assert_eq!(events[0].kind, AuditKind::AttemptAborted);
        assert_eq!(
            events[0].initiator,
            AuditInitiator::WorkerBinding {
                actor: worker_snapshot(),
                assignment: AssignmentId::new("asg-1").unwrap(),
                attempt: AttemptId::new("att-1").unwrap(),
            }
        );

        let all_events = port.audit_events(&AuditQuery::default()).unwrap();
        let replay = port.fenced_abort_attempt(&call).unwrap();
        assert_eq!(replay.applied, StateApplied::AlreadyApplied);
        assert_eq!(replay.head, response.head);
        assert_eq!(
            port.audit_events(&AuditQuery::default()).unwrap(),
            all_events
        );
        let changed = FencedCall {
            token: FencingToken(99),
            ..call
        };
        assert_eq!(
            port.fenced_abort_attempt(&changed),
            Err(StateError::ConflictingOperation)
        );
    }

    #[test]
    fn renewal_survives_abort_and_non_abort_directives_allow_appends() {
        let aborted = fake_state();
        let aborted_port: &dyn WorkflowStatePort = &aborted;
        aborted_port
            .append_signal(&directive_draft(
                "dir-renew-abort",
                "att-1",
                DirectiveKind::Abort {
                    reason: BoundedText::new("stop after observing this").unwrap(),
                },
            ))
            .unwrap();
        let (lease, renewal) = aborted_port
            .renew_lease(&good_call("op-renew-after-abort"), Timestamp(150))
            .unwrap();
        assert_eq!(lease.expires_at, Timestamp(150));
        assert_eq!(renewal.applied, StateApplied::Applied);
        assert_eq!(renewal.binding_directives[0].id.as_str(), "dir-renew-abort");
        assert_eq!(worker_action_count(&aborted), 0);

        let active = fake_state();
        let active_port: &dyn WorkflowStatePort = &active;
        active_port
            .append_signal(&directive_draft(
                "dir-append-amend",
                "att-1",
                DirectiveKind::Amend {
                    instruction: BoundedText::new("keep reporting while updating").unwrap(),
                },
            ))
            .unwrap();
        active_port
            .append_signal(&directive_draft(
                "dir-append-pause",
                "att-1",
                DirectiveKind::Pause {
                    reason: BoundedText::new("pause handoff only").unwrap(),
                },
            ))
            .unwrap();

        let (report_outcome, report_response) = active_port
            .fenced_report(
                &good_action("op-report-under-pause"),
                &report_draft("sig-report-under-pause"),
            )
            .unwrap();
        assert!(matches!(report_outcome, ReportOutcome::Recorded { .. }));
        assert_eq!(report_response.binding_directives.len(), 2);

        let (evidence_outcome, evidence_response) = active_port
            .fenced_evidence(&good_action("op-evidence-under-amend"), &passing_evidence())
            .unwrap();
        assert_eq!(evidence_outcome, EvidenceOutcome::Recorded);
        assert_eq!(evidence_response.binding_directives.len(), 2);
        assert_eq!(active.evidence_records.borrow().len(), 1);
        assert_eq!(worker_action_count(&active), 2);
    }

    #[test]
    fn validation_precedes_abort_gate_and_claims_no_operation() {
        let state = fake_state();
        let port: &dyn WorkflowStatePort = &state;
        port.append_signal(&directive_draft(
            "dir-validation-abort",
            "att-1",
            DirectiveKind::Abort {
                reason: BoundedText::new("stop work").unwrap(),
            },
        ))
        .unwrap();
        let head_before = state.current_head();
        let invalid = FencedAction {
            call: good_call("op-invalid-after-abort"),
            responds_to: Some(SignalId::new("dir-does-not-exist").unwrap()),
        };
        let evidence = passing_evidence();
        assert_eq!(
            port.fenced_evidence(&invalid, &evidence),
            Err(StateError::UnknownRecord)
        );
        assert_eq!(state.current_head(), head_before);
        assert!(
            !state
                .operations
                .borrow()
                .contains_key("fenced_evidence:op-invalid-after-abort")
        );
        assert!(
            !state
                .evidence_outcomes
                .borrow()
                .contains_key("op-invalid-after-abort")
        );

        // Reusing the operation with valid input reaches the ordinary
        // domain gate, proving the malformed request claimed nothing.
        let valid = FencedAction {
            responds_to: None,
            ..invalid
        };
        let (outcome, response) = port.fenced_evidence(&valid, &evidence).unwrap();
        assert_eq!(
            outcome,
            EvidenceOutcome::Refused {
                reason: DirectiveGateRefusal::AbortInForce
            }
        );
        assert_eq!(response.head, Seq(head_before.0 + 1));
        assert_eq!(worker_action_count(&state), 0);
    }

    #[test]
    fn linked_actions_discharge_only_the_exact_target_and_replay_once() {
        let state = fake_state();
        let port: &dyn WorkflowStatePort = &state;
        let report_id = SignalId::new("sig-question").unwrap();
        port.fenced_report(
            &good_action("op-question"),
            &report_draft(report_id.as_str()),
        )
        .unwrap();
        let amend = directive_draft(
            "dir-amend",
            "att-1",
            DirectiveKind::Amend {
                instruction: BoundedText::new("incorporate the requested change").unwrap(),
            },
        );
        let answer = directive_draft(
            "dir-answer",
            "att-1",
            DirectiveKind::Answer {
                report: report_id,
                answer: BoundedText::new("use the recorded decision").unwrap(),
            },
        );
        port.append_signal(&amend).unwrap();
        port.append_signal(&answer).unwrap();

        // An ordinary later action does not discharge either kind.
        let (unlinked_outcome, unlinked) = port
            .fenced_evidence(&good_action("op-unlinked"), &passing_evidence())
            .unwrap();
        assert_eq!(unlinked_outcome, EvidenceOutcome::Recorded);
        assert_eq!(
            unlinked
                .binding_directives
                .iter()
                .map(|signal| signal.id.as_str())
                .collect::<Vec<_>>(),
            vec!["dir-amend", "dir-answer"]
        );

        let linked_amend = FencedAction {
            call: good_call("op-linked-amend"),
            responds_to: Some(SignalId::new("dir-amend").unwrap()),
        };
        let (_, discharged) = port
            .fenced_report(&linked_amend, &report_draft("sig-amend-done"))
            .unwrap();
        // The response is post-commit: the exact target is already
        // absent, while the unrelated Answer remains binding.
        assert_eq!(
            discharged
                .binding_directives
                .iter()
                .map(|signal| signal.id.as_str())
                .collect::<Vec<_>>(),
            vec!["dir-answer"]
        );
        let action_count = state.response_actions.borrow().len();
        let replay_head = discharged.head;
        let (_, replay) = port
            .fenced_report(&linked_amend, &report_draft("sig-amend-done"))
            .unwrap();
        assert_eq!(replay.applied, StateApplied::AlreadyApplied);
        assert_eq!(replay.head, replay_head);
        assert_eq!(state.response_actions.borrow().len(), action_count);

        // The response link participates in the full operation
        // identity, independently of the otherwise identical call.
        let different_link = FencedAction {
            responds_to: Some(SignalId::new("dir-answer").unwrap()),
            ..linked_amend.clone()
        };
        assert_eq!(
            port.fenced_report(&different_link, &report_draft("sig-amend-done")),
            Err(StateError::ConflictingOperation)
        );
        assert_eq!(state.response_actions.borrow().len(), action_count);

        let linked_answer = FencedAction {
            call: good_call("op-linked-answer"),
            responds_to: Some(SignalId::new("dir-answer").unwrap()),
        };
        let (final_outcome, final_response) = port
            .fenced_evidence(&linked_answer, &passing_evidence())
            .unwrap();
        assert_eq!(final_outcome, EvidenceOutcome::Recorded);
        assert!(final_response.binding_directives.is_empty());
    }

    #[test]
    fn response_link_validation_refuses_without_committing() {
        let state = fake_state();
        let port: &dyn WorkflowStatePort = &state;
        let evidence = passing_evidence();
        let future_target = SignalId::new("dir-future").unwrap();
        let premature = FencedAction {
            call: good_call("op-premature"),
            responds_to: Some(future_target.clone()),
        };
        assert_eq!(
            port.fenced_evidence(&premature, &evidence),
            Err(StateError::UnknownRecord)
        );
        assert_eq!(state.current_head(), Seq(0));
        assert!(state.response_actions.borrow().is_empty());

        port.append_signal(&directive_draft(
            future_target.as_str(),
            "att-1",
            DirectiveKind::Amend {
                instruction: BoundedText::new("future instruction").unwrap(),
            },
        ))
        .unwrap();
        // The refused pre-Directive request claimed no operation and
        // created no earlier linked action. Reusing its operation for
        // an unlinked action succeeds and leaves the Directive binding.
        let unlinked_same_operation = FencedAction {
            responds_to: None,
            ..premature
        };
        let (outcome, response) = port
            .fenced_evidence(&unlinked_same_operation, &evidence)
            .unwrap();
        assert_eq!(outcome, EvidenceOutcome::Recorded);
        assert_eq!(
            response
                .binding_directives
                .iter()
                .map(|signal| signal.id.as_str())
                .collect::<Vec<_>>(),
            vec!["dir-future"]
        );

        let foreign_state = fake_state();
        foreign_state
            .attempt_states
            .borrow_mut()
            .insert("att-other".to_owned(), AttemptState::Active);
        let foreign_port: &dyn WorkflowStatePort = &foreign_state;
        foreign_port
            .append_signal(&directive_draft(
                "dir-foreign",
                "att-other",
                DirectiveKind::Amend {
                    instruction: BoundedText::new("foreign instruction").unwrap(),
                },
            ))
            .unwrap();
        let foreign_link = FencedAction {
            call: good_call("op-foreign-link"),
            responds_to: Some(SignalId::new("dir-foreign").unwrap()),
        };
        assert_eq!(
            foreign_port.fenced_evidence(&foreign_link, &evidence),
            Err(StateError::IncoherentBundle)
        );
        assert_eq!(foreign_state.current_head(), Seq(1));
        assert_eq!(worker_action_count(&foreign_state), 0);
    }

    #[test]
    fn link_carriage_does_not_replace_directive_kind_policy() {
        let state = fake_state();
        let port: &dyn WorkflowStatePort = &state;
        port.append_signal(&directive_draft(
            "dir-pause",
            "att-1",
            DirectiveKind::Pause {
                reason: BoundedText::new("wait for operator review").unwrap(),
            },
        ))
        .unwrap();
        let linked_pause = FencedAction {
            call: good_call("op-linked-pause"),
            responds_to: Some(SignalId::new("dir-pause").unwrap()),
        };
        let (outcome, response) = port
            .fenced_evidence(&linked_pause, &passing_evidence())
            .unwrap();
        assert_eq!(outcome, EvidenceOutcome::Recorded);
        assert_eq!(
            response
                .binding_directives
                .iter()
                .map(|signal| signal.id.as_str())
                .collect::<Vec<_>>(),
            vec!["dir-pause"]
        );
        assert_eq!(
            state.response_actions.borrow().last().unwrap().kind,
            ResponseKind::WorkerAction {
                attempt: AttemptId::new("att-1").unwrap(),
                responds_to: Some(SignalId::new("dir-pause").unwrap()),
            }
        );
    }

    #[test]
    fn linked_handoff_can_discharge_amend_but_refusal_cannot() {
        let state = fake_state();
        let port: &dyn WorkflowStatePort = &state;
        port.append_signal(&directive_draft(
            "dir-handoff",
            "att-1",
            DirectiveKind::Amend {
                instruction: BoundedText::new("update before handoff").unwrap(),
            },
        ))
        .unwrap();
        let candidate = handoff("h-linked", vec![op("op-evidence")]);
        let (refused, refused_response) = port
            .fenced_submit_handoff(&good_action("op-handoff-unlinked"), &candidate)
            .unwrap();
        assert_eq!(
            refused,
            SubmissionOutcome::Refused {
                reason: SubmissionRefusalReason::Directive(DirectiveGateRefusal::AmendUndischarged)
            }
        );
        assert_eq!(
            refused_response.binding_directives[0].id.as_str(),
            "dir-handoff"
        );
        assert_eq!(worker_action_count(&state), 0);

        let linked = FencedAction {
            call: good_call("op-handoff-linked"),
            responds_to: Some(SignalId::new("dir-handoff").unwrap()),
        };
        let (recorded, response) = port.fenced_submit_handoff(&linked, &candidate).unwrap();
        assert_eq!(
            recorded,
            SubmissionOutcome::Recorded {
                handoff: HandoffId::new("h-linked").unwrap()
            }
        );
        assert!(response.binding_directives.is_empty());
        assert_eq!(worker_action_count(&state), 1);
    }

    /// R5.28: the idempotency record binds FULL call identity, durable
    /// record ids belong to one operation, and lease taxonomy is
    /// honest.
    #[test]
    fn idempotency_binds_full_call_identity() {
        let state = fake_state();
        let port: &dyn WorkflowStatePort = &state;
        let evidence = crate::evidence::Evidence::new(
            Argv::new(vec!["cargo".into(), "test".into()]).unwrap(),
            VerificationSet::new(
                vec![Argv::new(vec!["cargo".into(), "test".into()]).unwrap()],
                PathSet::new(vec![WorkPath::new("tests/a.rs").unwrap()]).unwrap(),
            )
            .unwrap(),
            0,
            VerificationOutcome::Pass,
            CommitId::new(&"c".repeat(40)).unwrap(),
            crate::content::WorkspaceDigest::new(&"1".repeat(64)).unwrap(),
            crate::content::WorkspaceDigest::new(&"1".repeat(64)).unwrap(),
            None,
            FileDigestSet::default(),
            None,
        )
        .unwrap();
        let base = good_action("op-ev");
        assert_eq!(
            port.fenced_evidence(&base, &evidence).unwrap().1.applied,
            StateApplied::Applied
        );
        // Same operation + payload, but ANY altered call identity
        // field must NOT receive the prior result.
        for altered in [
            FencedAction {
                call: FencedCall {
                    attempt: AttemptId::new("att-other").unwrap(),
                    ..base.call.clone()
                },
                ..base.clone()
            },
            FencedAction {
                call: FencedCall {
                    token: FencingToken(2),
                    ..base.call.clone()
                },
                ..base.clone()
            },
        ] {
            assert_eq!(
                port.fenced_evidence(&altered, &evidence),
                Err(StateError::ConflictingOperation)
            );
        }
        // The exact original call still replays.
        assert_eq!(
            port.fenced_evidence(&base, &evidence).unwrap().1.applied,
            StateApplied::AlreadyApplied
        );

        // A NEW operation may not claim an existing SignalId.
        let draft = report_draft("sig-owned");
        assert_eq!(
            port.fenced_report(&good_action("op-r1"), &draft)
                .unwrap()
                .1
                .applied,
            StateApplied::Applied
        );
        assert_eq!(
            port.fenced_report(&good_action("op-r2"), &draft).err(),
            Some(StateError::ConflictingOperation)
        );

        // Non-extending renewal against a LIVE lease is its own
        // refusal, never LeaseExpired.
        assert_eq!(
            port.renew_lease(&good_call("op-rn"), Timestamp(50)).err(),
            Some(StateError::NonExtendingLease)
        );

        // Token supersession: a committed old-token replay is inert,
        // but a NEW operation on the old token is stale.
        let old_action = good_action("op-old-token");
        assert_eq!(
            port.fenced_evidence(&old_action, &evidence)
                .unwrap()
                .1
                .applied,
            StateApplied::Applied
        );
        *state.current_token.borrow_mut() = FencingToken(4);
        assert_eq!(
            port.fenced_evidence(&old_action, &evidence)
                .unwrap()
                .1
                .applied,
            StateApplied::AlreadyApplied
        );
        assert_eq!(
            port.fenced_evidence(&good_action("op-after-super"), &evidence),
            Err(StateError::StaleFencing)
        );
    }

    /// R5.29: record ownership is kind-scoped — a Signal and a Handoff
    /// may share textual ids without blocking each other, while
    /// same-kind reuse by a new operation still conflicts.
    #[test]
    fn record_ownership_is_kind_scoped() {
        let state = fake_state();
        let port: &dyn WorkflowStatePort = &state;
        let mut draft = report_draft("shared-id");
        draft.id = SignalId::new("shared-id").unwrap();
        assert_eq!(
            port.fenced_report(&good_action("op-a"), &draft)
                .unwrap()
                .1
                .applied,
            StateApplied::Applied
        );
        // A Handoff with the SAME textual id is unaffected.
        let h = HandoffRecord {
            id: HandoffId::new("shared-id").unwrap(),
            ..handoff("h-ignored", vec![op("op-evi")])
        };
        let (outcome, _) = port
            .fenced_submit_handoff(&good_action("op-b"), &h)
            .unwrap();
        assert_eq!(
            outcome,
            SubmissionOutcome::Recorded {
                handoff: HandoffId::new("shared-id").unwrap()
            }
        );
        // Same-kind reuse by a different operation still conflicts.
        assert_eq!(
            port.fenced_report(&good_action("op-c"), &draft).err(),
            Some(StateError::ConflictingOperation)
        );
        assert_eq!(
            port.fenced_submit_handoff(&good_action("op-d"), &h).err(),
            Some(StateError::ConflictingOperation)
        );
    }

    #[test]
    fn host_path_must_be_absolute() {
        assert!(HostPath::new("/home/user/wt").is_ok());
        assert_eq!(
            HostPath::new("relative/path"),
            Err(HostPathError::NotAbsolute)
        );
        assert_eq!(HostPath::new("/"), Err(HostPathError::NotAbsolute));
    }

    #[test]
    fn commit_verifier_distinguishes_unavailable_from_absent() {
        struct FakeVerifier;
        impl CommitVerifierPort for FakeVerifier {
            fn exists(&self, commit: &CommitId) -> Result<(), VerifyError> {
                if commit.as_str().starts_with('a') {
                    Ok(())
                } else {
                    Err(VerifyError::UnknownCommit)
                }
            }

            fn relation(
                &self,
                _base: &CommitId,
                commit: &CommitId,
            ) -> Result<BaseRelation, VerifyError> {
                self.exists(commit)?;
                Ok(BaseRelation::BasedOn)
            }

            fn changed_paths(
                &self,
                _base: &CommitId,
                _commit: &CommitId,
            ) -> Result<Vec<WorkPath>, VerifyError> {
                Err(VerifyError::Unavailable)
            }

            fn file_digest(
                &self,
                _commit: &CommitId,
                _path: &WorkPath,
            ) -> Result<Option<ContentHash>, VerifyError> {
                Ok(None)
            }

            fn worktree_view(&self, _worktree: &HostPath) -> Result<WorktreeView, VerifyError> {
                Ok(WorktreeView {
                    head: CommitId::new(&"a".repeat(40)).unwrap(),
                    workspace_digest: crate::content::WorkspaceDigest::new(&"9".repeat(64))
                        .unwrap(),
                    dirt: DirtSet::default(),
                })
            }
        }
        let v = FakeVerifier;
        let known = CommitId::new(&"a".repeat(40)).unwrap();
        let unknown = CommitId::new(&"b".repeat(40)).unwrap();
        assert_eq!(v.exists(&known), Ok(()));
        assert_eq!(v.exists(&unknown), Err(VerifyError::UnknownCommit));
        assert_eq!(v.relation(&known, &known), Ok(BaseRelation::BasedOn));
        assert_eq!(
            v.changed_paths(&known, &known),
            Err(VerifyError::Unavailable)
        );
    }
}
