//! Domain language and pure rules for ABACUS.
//!
//! The binding contract is `README.md` in this folder. This crate owns
//! identifiers, the two authority classes, assignment/attempt lifecycles,
//! leases and fencing, typed Signals, evidence semantics, and the
//! provider-neutral ports. It is deterministic: no I/O, no clock or ID
//! generation of its own, and no knowledge of SQLite, subprocesses,
//! `br`, `bv`, or Herdr.

#![forbid(unsafe_code)]

pub mod assignment;
pub mod authority;
pub mod content;
pub mod edit_scope;
pub mod evidence;
pub mod id;
pub mod lease;
pub mod lifecycle;
pub mod ports;
pub mod profile;
pub mod scope;
pub mod signal;
pub mod usecase;

pub use assignment::{
    AttemptCap, AttemptCapError, AttemptCapReached, AttemptPolicy, AttemptSequenceError,
    BeadHashMismatch, DecisionActor, next_attempt_allowed, recheck_bead_hash, retry_within_cap,
};
pub use authority::AuthorityClass;
pub use content::{CommitId, ContentError, ContentHash, WorkspaceDigest};
pub use edit_scope::{EditScope, EditScopeError, PathError, WorkPath};
pub use evidence::{
    Argv, CollectionError, Evidence, EvidenceShapeError, FileDigestSet, OverlayCapture,
    OverlayFile, PairRefusal, PathSet, RedGreenPolicy, VerificationOutcome, VerificationSet,
    evaluate_red_green_pair,
};
pub use id::{
    ActorId, AssignmentId, AttemptId, BeadId, CapabilityId, HandoffId, IdError, OperationId,
    ProfileName, SignalId,
};
pub use lease::{FencingError, FencingToken, Lease, Timestamp, validate_fencing};
pub use lifecycle::{
    AssignmentAction, AssignmentState, AttemptAction, AttemptState, TransitionError,
    assignment_transition, attempt_transition,
};
pub use profile::{
    AuthorizationRefusal, AuthorizationTarget, Bundle, CapabilityDescriptor, CheckClass, Grant,
    OccupancyClass, ProfileConfigError, ProfileSpec, RouteOutcome, ValidatedProfileSet,
    validate_profiles,
};
pub use scope::{Atom, ScopeError, ScopeExpr, ScopeKey, ScopeMap, ScopeValue, Selector};
pub use signal::{
    AppendOutcome, ConflictingDuplicate, DirectiveGateRefusal, DirectiveKind, DirectiveStatus,
    ReportDraft, ReportKind, RequestKind, ResponseAction, ResponseKind, Seq, Signal, SignalBody,
    SignalDraft, SignalSender, SubjectError, SubjectRef, append_idempotent, binding_directives,
    directive_status, handoff_gate, unresolved, worker_append_gate,
};
pub use signal::{AuthoritySnapshot, BoundedText, BoundedTextError, SemanticPhase};
