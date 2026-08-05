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
//! rules — Scribe-allocated ordering, fenced envelopes, idempotency —
//! are encoded here in the types. The typed audit-lineage query is
//! deliberately absent until `abacus-state` defines the audit value
//! (9NH.7); it arrives as a C1 extension rather than fossilizing a
//! string placeholder.

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
use crate::signal::{AuthoritySnapshot, DirectiveGateRefusal, Seq, Signal, SignalDraft};

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

/// Read-before-write inspection view: normalized status/revision facts
/// for ambiguity reconciliation and out-of-band correlation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BeadStatusView {
    pub snapshot: BeadSnapshot,
    pub status: WorkStatus,
    pub revision: WorkRevision,
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

/// Outcome of a decision-gated work mutation. The provider cannot
/// attest ABACUS operation identity: an already-present effect reports
/// observed normalized facts, and correlating origin is core's job
/// against the Ledger.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MutationOutcome {
    Applied {
        before: WorkRevision,
        after: WorkRevision,
        /// Audit-safe normalized summary.
        summary: String,
    },
    EffectAlreadyPresent {
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

/// The single advice gate (core invariant 6): accept only advice bound
/// to the current revision whose order is an exact permutation of the
/// eligible set; otherwise the deterministic fallback.
pub fn apply_advice(
    ready: &[BeadSnapshot],
    outcome: &AdviceOutcome,
    current: &WorkRevision,
) -> Vec<BeadId> {
    let AdviceOutcome::Advice { order, bound_to } = outcome else {
        return fallback_order(ready);
    };
    if bound_to != current || order.len() != ready.len() {
        return fallback_order(ready);
    }
    let mut expected: Vec<&str> = ready.iter().map(|b| b.id.as_str()).collect();
    let mut proposed: Vec<&str> = order.iter().map(|b| b.as_str()).collect();
    expected.sort_unstable();
    proposed.sort_unstable();
    if expected != proposed {
        return fallback_order(ready);
    }
    order.clone()
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

/// The architecture §2.4 opening bundle, committed in ONE Scribe
/// transaction under the Assign decision's single operation identity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssignmentOpening {
    pub assignment: AssignmentRecord,
    pub first_attempt: AttemptRecord,
    pub authorizing: AssignDecision,
    pub bead_revision: WorkRevision,
}

/// Fenced worker call identity (state protocol rules): the
/// authenticated actor, its Assignment/Attempt, the current fencing
/// token, and the idempotency operation. A leaked token alone can
/// never impersonate the bound worker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FencedCall {
    pub assignment: AssignmentId,
    pub attempt: AttemptId,
    pub actor: ActorId,
    pub token: FencingToken,
    pub operation: OperationId,
}

/// Every fenced response mechanically surfaces the Attempt's current
/// binding Directives and Ledger head — present even when empty.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FencedResponse {
    pub applied: StateApplied,
    pub binding_directives: Vec<Signal>,
    pub head: Seq,
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
/// whatever happened; only confirmed success also yields a receipt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApplicationOutcome {
    Applied {
        before: WorkRevision,
        after: WorkRevision,
    },
    EffectAlreadyPresent {
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
    pub decision: OperationId,
    pub outcome: ApplicationOutcome,
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
    LeaseExpired,
    /// The authenticated actor is not the record's bound actor.
    ActorMismatch,
    /// Actor's current grant does not cover the subject (I17).
    ScopeUnauthorized,
    /// A singleton-occupancy profile is already occupied (ADR-0002 §7).
    ProfileOccupied,
    /// An ActorId registers/resumes exactly one stable AuthorityClass;
    /// activating under a different class is refused (this structurally
    /// preserves "a worker cannot accept its own handoff").
    ActorClassMismatch,
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

/// A committed decision lacking its successful application receipt,
/// carrying the full Decision so reconciliation needs no second read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingApplication {
    pub decision: DecisionRecord,
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

/// Durable workflow persistence: the Scribe seam, shaped as
/// transactional use-case operations. `abacus-state` implements it over
/// SQLite; the 9NH.11 fake implements it for use-case tests. Scribe
/// allocates all commit ordering; worker mutations ride
/// [`FencedCall`]/[`FencedResponse`]; "pending"/"unresolved" are
/// derived queries, never flags (I10).
pub trait WorkflowStatePort {
    /// Commit the complete opening bundle in one transaction under the
    /// Assign decision's operation identity.
    fn open_assignment(&self, opening: &AssignmentOpening) -> Result<StateApplied, StateError>;

    /// Append a new fenced Attempt atomically with its authorizing
    /// Retry decision.
    fn append_attempt(
        &self,
        retry: &RetryDecision,
        attempt: &AttemptRecord,
    ) -> Result<StateApplied, StateError>;

    /// Record an orchestrator decision (accept/reject/cancel/revoke/
    /// reclaim/transfer).
    fn record_decision(&self, record: &DecisionRecord) -> Result<StateApplied, StateError>;

    /// Audited profile activation (ADR-0002 §7): a singleton-occupancy
    /// profile refuses a second active occupant; shared profiles
    /// multi-occupy freely.
    fn activate_profile(&self, activation: &ProfileActivation) -> Result<StateApplied, StateError>;

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

    /// Fenced worker Report append; Scribe allocates `Seq`.
    fn fenced_report(
        &self,
        call: &FencedCall,
        draft: &SignalDraft,
    ) -> Result<(Signal, FencedResponse), StateError>;

    /// Fenced worker Evidence append.
    fn fenced_evidence(
        &self,
        call: &FencedCall,
        evidence: &Evidence,
    ) -> Result<FencedResponse, StateError>;

    /// Fenced Handoff submission: records the immutable Handoff and the
    /// Attempt's `Submitted` transition, or audits a Submission refusal
    /// that creates no Handoff and leaves the Attempt active. The
    /// call's operation idempotently owns whichever outcome occurred.
    fn fenced_submit_handoff(
        &self,
        call: &FencedCall,
        handoff: &HandoffRecord,
    ) -> Result<(SubmissionOutcome, FencedResponse), StateError>;

    /// Fenced lease renewal.
    fn renew_lease(
        &self,
        call: &FencedCall,
        until: Timestamp,
    ) -> Result<(Lease, FencedResponse), StateError>;

    /// Persist the canonical sanitized Envelope snapshot before any
    /// live delivery (architecture §3.3).
    fn persist_envelope(
        &self,
        operation: &OperationId,
        attempt: &AttemptId,
        envelope: &EnvelopeSnapshot,
    ) -> Result<StateApplied, StateError>;

    /// Read the persisted Envelope for an Attempt.
    fn envelope(&self, attempt: &AttemptId) -> Result<EnvelopeSnapshot, StateError>;

    /// Associate an opaque runtime handle with an Attempt (architecture
    /// §3.5). Re-association after generation change is a new explicit
    /// bind; reconciling an uncertain association is unbind+bind under
    /// their own operations.
    fn bind_runtime_handle(
        &self,
        operation: &OperationId,
        attempt: &AttemptId,
        handle: &RuntimeHandle,
    ) -> Result<StateApplied, StateError>;

    fn unbind_runtime_handle(
        &self,
        operation: &OperationId,
        attempt: &AttemptId,
    ) -> Result<StateApplied, StateError>;

    fn runtime_handle(&self, attempt: &AttemptId) -> Result<Option<RuntimeHandle>, StateError>;

    /// Record one immutable application attempt (any outcome).
    fn record_application_attempt(
        &self,
        attempt: &ApplicationAttempt,
    ) -> Result<StateApplied, StateError>;

    /// Record the confirmed-success receipt; pending remains "decisions
    /// lacking a successful receipt".
    fn record_application_receipt(
        &self,
        decision: &OperationId,
        after: &WorkRevision,
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

    /// Derived reconciliation set, typed: decisions lacking receipts.
    fn pending_applications(&self) -> Result<Vec<PendingApplication>, StateError>;

    /// Derived unresolved-Signal set, per recipient or global.
    fn unresolved_signals(&self, recipient: Option<&ActorId>) -> Result<Vec<Signal>, StateError>;
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
    pub attempt: AttemptId,
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

/// Normalized launch facts returned with the handle.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaunchOutcome {
    pub handle: RuntimeHandle,
    pub observation: LivenessObservation,
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
    fn launch(&self, spec: &LaunchSpec) -> Result<LaunchOutcome, RuntimeError>;

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
    use crate::authority::AuthorityClass;
    use crate::evidence::{Argv, FileDigestSet, VerificationOutcome, VerificationSet};
    use crate::id::CapabilityId;
    use crate::profile::{
        CapabilityDescriptor, CheckClass, Grant, OccupancyClass, ProfileSpec, ValidatedProfileSet,
        validate_profiles,
    };
    use crate::scope::{ScopeExpr, ScopeKey, ScopeValue};
    use crate::signal::{BoundedText, ReportKind, SignalBody, SubjectRef};
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
                return Ok(MutationOutcome::EffectAlreadyPresent {
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
            Ok(MutationOutcome::EffectAlreadyPresent {
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
        current_token: FencingToken,
        bound_worker: ActorId,
        next_seq: RefCell<u64>,
        stored_signals: RefCell<Vec<Signal>>,
        submissions: RefCell<BTreeMap<String, (String, SubmissionOutcome)>>,
        receipts: RefCell<Vec<String>>,
        decisions: RefCell<Vec<DecisionRecord>>,
        handoffs: RefCell<Vec<HandoffRecord>>,
        actor_classes: RefCell<BTreeMap<String, AuthorityClass>>,
    }

    impl FakeState {
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
            if call.actor != self.bound_worker {
                return Err(StateError::ActorMismatch);
            }
            if call.token != self.current_token {
                return Err(StateError::StaleFencing);
            }
            Ok(())
        }

        fn respond(&self, applied: StateApplied) -> FencedResponse {
            FencedResponse {
                applied,
                binding_directives: Vec::new(),
                head: Seq(1),
            }
        }

        fn commit_signal(&self, draft: &SignalDraft) -> Result<(Signal, StateApplied), StateError> {
            if let Some(existing) = self
                .stored_signals
                .borrow()
                .iter()
                .find(|s| s.id == draft.id)
            {
                let redraft = SignalDraft {
                    id: existing.id.clone(),
                    sender: existing.sender.clone(),
                    subject: existing.subject.clone(),
                    body: existing.body.clone(),
                };
                if redraft == *draft {
                    return Ok((existing.clone(), StateApplied::AlreadyApplied));
                }
                return Err(StateError::ConflictingOperation);
            }
            let mut next = self.next_seq.borrow_mut();
            *next += 1;
            let committed = draft.clone().commit(Seq(*next));
            self.stored_signals.borrow_mut().push(committed.clone());
            Ok((committed, StateApplied::Applied))
        }
    }

    impl WorkflowStatePort for FakeState {
        fn open_assignment(&self, opening: &AssignmentOpening) -> Result<StateApplied, StateError> {
            self.apply(
                format!("open:{}", opening.authorizing.operation.as_str()),
                format!("{opening:?}"),
            )
        }

        fn append_attempt(
            &self,
            retry: &RetryDecision,
            attempt: &AttemptRecord,
        ) -> Result<StateApplied, StateError> {
            self.apply(
                format!("attempt:{}", retry.operation.as_str()),
                format!("{retry:?}{attempt:?}"),
            )
        }

        fn record_decision(&self, record: &DecisionRecord) -> Result<StateApplied, StateError> {
            let applied = self.apply(
                format!("dec:{}", record.operation.as_str()),
                format!("{record:?}"),
            )?;
            if applied == StateApplied::Applied {
                self.decisions.borrow_mut().push(record.clone());
            }
            Ok(applied)
        }

        fn activate_profile(
            &self,
            activation: &ProfileActivation,
        ) -> Result<StateApplied, StateError> {
            let mut classes = self.actor_classes.borrow_mut();
            match classes.get(activation.actor.as_str()) {
                Some(existing) if *existing != activation.class() => {
                    return Err(StateError::ActorClassMismatch);
                }
                _ => {
                    classes.insert(activation.actor.as_str().to_owned(), activation.class());
                }
            }
            drop(classes);
            let holder = format!(
                "{}:{}",
                activation.actor.as_str(),
                activation.profile_hash.as_str()
            );
            if activation.occupancy() == OccupancyClass::Singleton {
                let occupied_key = format!("occupied:{}", activation.profile.as_str());
                let existing = self.committed.borrow().get(&occupied_key).cloned();
                match existing {
                    Some(current) if current != holder => {
                        return Err(StateError::ProfileOccupied);
                    }
                    _ => {
                        self.committed
                            .borrow_mut()
                            .insert(occupied_key, holder.clone());
                    }
                }
            }
            self.apply(format!("act:{}", activation.operation.as_str()), holder)
        }

        fn deactivate_profile(
            &self,
            operation: &OperationId,
            actor: &ActorId,
            profile: &ProfileName,
        ) -> Result<StateApplied, StateError> {
            self.committed
                .borrow_mut()
                .remove(&format!("occupied:{}", profile.as_str()));
            self.apply(
                format!("deact:{}", operation.as_str()),
                actor.as_str().to_owned(),
            )
        }

        fn append_signal(&self, draft: &SignalDraft) -> Result<(Signal, StateApplied), StateError> {
            self.commit_signal(draft)
        }

        fn fenced_report(
            &self,
            call: &FencedCall,
            draft: &SignalDraft,
        ) -> Result<(Signal, FencedResponse), StateError> {
            self.fence(call)?;
            let (signal, applied) = self.commit_signal(draft)?;
            Ok((signal, self.respond(applied)))
        }

        fn fenced_evidence(
            &self,
            call: &FencedCall,
            evidence: &Evidence,
        ) -> Result<FencedResponse, StateError> {
            self.fence(call)?;
            let applied = self.apply(
                format!("evi:{}", call.operation.as_str()),
                format!("{evidence:?}"),
            )?;
            Ok(self.respond(applied))
        }

        fn fenced_submit_handoff(
            &self,
            call: &FencedCall,
            handoff: &HandoffRecord,
        ) -> Result<(SubmissionOutcome, FencedResponse), StateError> {
            self.fence(call)?;
            let key = call.operation.as_str().to_owned();
            let content = format!("{handoff:?}");
            if let Some((stored_content, stored_outcome)) = self.submissions.borrow().get(&key) {
                if *stored_content == content {
                    return Ok((
                        stored_outcome.clone(),
                        self.respond(StateApplied::AlreadyApplied),
                    ));
                }
                return Err(StateError::ConflictingOperation);
            }
            let outcome = if handoff.evidence_operations.is_empty() {
                SubmissionOutcome::Refused {
                    reason: SubmissionRefusalReason::MissingEvidence,
                }
            } else {
                SubmissionOutcome::Recorded {
                    handoff: handoff.id.clone(),
                }
            };
            if matches!(outcome, SubmissionOutcome::Recorded { .. }) {
                self.handoffs.borrow_mut().push(handoff.clone());
            }
            self.submissions
                .borrow_mut()
                .insert(key, (content, outcome.clone()));
            Ok((outcome, self.respond(StateApplied::Applied)))
        }

        fn renew_lease(
            &self,
            call: &FencedCall,
            until: Timestamp,
        ) -> Result<(Lease, FencedResponse), StateError> {
            self.fence(call)?;
            Ok((
                Lease {
                    token: call.token,
                    expires_at: until,
                },
                self.respond(StateApplied::Applied),
            ))
        }

        fn persist_envelope(
            &self,
            operation: &OperationId,
            attempt: &AttemptId,
            envelope: &EnvelopeSnapshot,
        ) -> Result<StateApplied, StateError> {
            self.apply(
                format!("env:{}:{}", operation.as_str(), attempt.as_str()),
                envelope.content_hash.as_str().to_owned(),
            )
        }

        fn envelope(&self, _attempt: &AttemptId) -> Result<EnvelopeSnapshot, StateError> {
            Err(StateError::UnknownRecord)
        }

        fn bind_runtime_handle(
            &self,
            operation: &OperationId,
            attempt: &AttemptId,
            handle: &RuntimeHandle,
        ) -> Result<StateApplied, StateError> {
            self.apply(
                format!("bind:{}:{}", operation.as_str(), attempt.as_str()),
                handle.as_str().to_owned(),
            )
        }

        fn unbind_runtime_handle(
            &self,
            operation: &OperationId,
            attempt: &AttemptId,
        ) -> Result<StateApplied, StateError> {
            self.apply(
                format!("unbind:{}:{}", operation.as_str(), attempt.as_str()),
                "".into(),
            )
        }

        fn runtime_handle(
            &self,
            _attempt: &AttemptId,
        ) -> Result<Option<RuntimeHandle>, StateError> {
            Ok(None)
        }

        fn record_application_attempt(
            &self,
            attempt: &ApplicationAttempt,
        ) -> Result<StateApplied, StateError> {
            self.apply(
                format!("app-attempt:{}", attempt.id.as_str()),
                format!("{attempt:?}"),
            )
        }

        fn record_application_receipt(
            &self,
            decision: &OperationId,
            after: &WorkRevision,
        ) -> Result<StateApplied, StateError> {
            self.receipts
                .borrow_mut()
                .push(decision.as_str().to_owned());
            self.apply(
                format!("receipt:{}", decision.as_str()),
                format!("{after:?}"),
            )
        }

        fn assignment(&self, _id: &AssignmentId) -> Result<AssignmentView, StateError> {
            Err(StateError::UnknownRecord)
        }

        fn evidence_for(&self, _attempt: &AttemptId) -> Result<Vec<EvidenceRecord>, StateError> {
            Ok(Vec::new())
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
            let committed = self.committed.borrow();
            Ok(committed
                .get(&format!("occupied:{}", profile.as_str()))
                .and_then(|holder| holder.split(':').next())
                .map(|actor| vec![ActorId::new(actor).unwrap()])
                .unwrap_or_default())
        }

        fn pending_applications(&self) -> Result<Vec<PendingApplication>, StateError> {
            let receipts = self.receipts.borrow();
            Ok(self
                .decisions
                .borrow()
                .iter()
                .filter(|d| !receipts.iter().any(|r| r == d.operation.as_str()))
                .map(|d| PendingApplication {
                    decision: d.clone(),
                })
                .collect())
        }

        fn unresolved_signals(
            &self,
            _recipient: Option<&ActorId>,
        ) -> Result<Vec<Signal>, StateError> {
            Ok(Vec::new())
        }
    }

    fn fake_state() -> FakeState {
        FakeState {
            committed: RefCell::new(BTreeMap::new()),
            current_token: FencingToken(3),
            bound_worker: ActorId::new("worker-1").unwrap(),
            next_seq: RefCell::new(0),
            stored_signals: RefCell::new(Vec::new()),
            submissions: RefCell::new(BTreeMap::new()),
            receipts: RefCell::new(Vec::new()),
            decisions: RefCell::new(Vec::new()),
            handoffs: RefCell::new(Vec::new()),
            actor_classes: RefCell::new(BTreeMap::new()),
        }
    }

    fn good_call(operation: &str) -> FencedCall {
        FencedCall {
            assignment: AssignmentId::new("asg-1").unwrap(),
            attempt: AttemptId::new("att-1").unwrap(),
            actor: ActorId::new("worker-1").unwrap(),
            token: FencingToken(3),
            operation: op(operation),
        }
    }

    fn report_draft(id: &str) -> SignalDraft {
        SignalDraft {
            id: SignalId::new(id).unwrap(),
            sender: authority("state:report"),
            subject: SubjectRef::Attempt(AttemptId::new("att-1").unwrap()),
            body: SignalBody::Report {
                attempt: AttemptId::new("att-1").unwrap(),
                kind: ReportKind::Progress {
                    phase: crate::signal::SemanticPhase::Verifying,
                    summary: None,
                },
            },
        }
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
    fn scribe_allocates_signal_order_and_absorbs_retries() {
        let state = fake_state();
        let port: &dyn WorkflowStatePort = &state;
        let draft = report_draft("sig-1");
        let (first, applied) = port.append_signal(&draft).unwrap();
        assert_eq!(applied, StateApplied::Applied);
        assert_eq!(first.seq, Seq(1));
        let (again, retry) = port.append_signal(&draft).unwrap();
        assert_eq!(retry, StateApplied::AlreadyApplied);
        assert_eq!(again, first);
        let mut altered = draft.clone();
        altered.body = SignalBody::Report {
            attempt: AttemptId::new("att-1").unwrap(),
            kind: ReportKind::BlockedWithReason {
                reason: BoundedText::new("blocked on dependency").unwrap(),
            },
        };
        assert_eq!(
            port.append_signal(&altered),
            Err(StateError::ConflictingOperation)
        );
    }

    #[test]
    fn fenced_calls_authenticate_actor_and_token() {
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
        let wrong_actor = FencedCall {
            actor: ActorId::new("intruder").unwrap(),
            ..good_call("op-evi")
        };
        assert_eq!(
            port.fenced_evidence(&wrong_actor, &evidence),
            Err(StateError::ActorMismatch)
        );
        let stale = FencedCall {
            token: FencingToken(2),
            ..good_call("op-evi")
        };
        assert_eq!(
            port.fenced_evidence(&stale, &evidence),
            Err(StateError::StaleFencing)
        );
        let response = port
            .fenced_evidence(&good_call("op-evi"), &evidence)
            .unwrap();
        assert_eq!(response.applied, StateApplied::Applied);
    }

    #[test]
    fn submission_operation_owns_recorded_and_refused_outcomes() {
        let state = fake_state();
        let port: &dyn WorkflowStatePort = &state;

        let refused = handoff("h-1", vec![]);
        let (outcome, response) = port
            .fenced_submit_handoff(&good_call("op-h1"), &refused)
            .unwrap();
        assert_eq!(
            outcome,
            SubmissionOutcome::Refused {
                reason: SubmissionRefusalReason::MissingEvidence
            }
        );
        assert_eq!(response.applied, StateApplied::Applied);
        let (retry_outcome, retry_response) = port
            .fenced_submit_handoff(&good_call("op-h1"), &refused)
            .unwrap();
        assert_eq!(retry_outcome, outcome);
        assert_eq!(retry_response.applied, StateApplied::AlreadyApplied);
        let different = handoff("h-2", vec![]);
        assert_eq!(
            port.fenced_submit_handoff(&good_call("op-h1"), &different),
            Err(StateError::ConflictingOperation)
        );

        let recorded = handoff("h-3", vec![op("op-evi")]);
        let (outcome, _) = port
            .fenced_submit_handoff(&good_call("op-h2"), &recorded)
            .unwrap();
        assert_eq!(
            outcome,
            SubmissionOutcome::Recorded {
                handoff: HandoffId::new("h-3").unwrap()
            }
        );
        let (retry_outcome, retry_response) = port
            .fenced_submit_handoff(&good_call("op-h2"), &recorded)
            .unwrap();
        assert_eq!(retry_outcome, outcome);
        assert_eq!(retry_response.applied, StateApplied::AlreadyApplied);
        let different = handoff("h-4", vec![op("op-evi")]);
        assert_eq!(
            port.fenced_submit_handoff(&good_call("op-h2"), &different),
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
        assert_eq!(port.record_decision(&decision), Ok(StateApplied::Applied));
        assert_eq!(port.pending_applications().unwrap().len(), 1);

        let first = ApplicationAttempt {
            id: op("app-1"),
            decision: op("op-accept"),
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
            decision: op("op-accept"),
            outcome: ApplicationOutcome::Ambiguous,
        };
        assert_eq!(
            port.record_application_attempt(&conflicting),
            Err(StateError::ConflictingOperation)
        );
        let second = ApplicationAttempt {
            id: op("app-2"),
            decision: op("op-accept"),
            outcome: ApplicationOutcome::Ambiguous,
        };
        assert_eq!(
            port.record_application_attempt(&second),
            Ok(StateApplied::Applied)
        );
        assert_eq!(port.pending_applications().unwrap().len(), 1);
        assert_eq!(
            port.record_application_receipt(&op("op-accept"), &rev('f')),
            Ok(StateApplied::Applied)
        );
        assert!(port.pending_applications().unwrap().is_empty());
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
        let profiles = vec![
            ProfileSpec {
                name: ProfileName::new("lead").unwrap(),
                class: AuthorityClass::Orchestrator,
                grants: vec![Grant {
                    capability: CapabilityId::new("work:select").unwrap(),
                    scope: ScopeExpr::Universal,
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

    fn activation(op_id: &str, actor_id: &str, profile: &str) -> ProfileActivation {
        ProfileActivation::from_validated(
            &small_validated_set(),
            op(op_id),
            ActorId::new(actor_id).unwrap(),
            ProfileName::new(profile).unwrap(),
            ContentHash::new(&"a".repeat(64)).unwrap(),
        )
        .unwrap()
    }

    #[test]
    fn occupancy_class_governs_activation() {
        let state = fake_state();
        let port: &dyn WorkflowStatePort = &state;
        assert_eq!(
            port.activate_profile(&activation("a-1", "worker-1", "worker")),
            Ok(StateApplied::Applied)
        );
        assert_eq!(
            port.activate_profile(&activation("a-2", "worker-2", "worker")),
            Ok(StateApplied::Applied)
        );
        assert_eq!(
            port.activate_profile(&activation("a-3", "lead-1", "lead")),
            Ok(StateApplied::Applied)
        );
        assert_eq!(
            port.activate_profile(&activation("a-4", "lead-2", "lead")),
            Err(StateError::ProfileOccupied)
        );
        assert_eq!(
            port.deactivate_profile(
                &op("d-1"),
                &ActorId::new("lead-1").unwrap(),
                &ProfileName::new("lead").unwrap()
            ),
            Ok(StateApplied::Applied)
        );
        assert_eq!(
            port.activate_profile(&activation("a-5", "lead-2", "lead")),
            Ok(StateApplied::Applied)
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
        fn launch(&self, _spec: &LaunchSpec) -> Result<LaunchOutcome, RuntimeError> {
            Ok(LaunchOutcome {
                handle: self.live.clone(),
                observation: LivenessObservation {
                    observed_at: Timestamp(1),
                    kind: LivenessKind::Starting,
                },
            })
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

    fn spec() -> LaunchSpec {
        LaunchSpec {
            attempt: AttemptId::new("att-1").unwrap(),
            agent_kind: "claude".into(),
            executable: "/usr/local/bin/agent".into(),
            args: vec!["--project".into()],
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
        let outcome = port.launch(&spec()).unwrap();
        assert_eq!(outcome.observation.kind, LivenessKind::Starting);
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
