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
pub mod signal;

pub use assignment::{
    next_attempt_allowed, recheck_bead_hash, AttemptSequenceError, BeadHashMismatch, DecisionActor,
};
pub use authority::AuthorityClass;
pub use content::{CommitId, ContentError, ContentHash, WorkspaceDigest};
pub use edit_scope::{EditScope, EditScopeError, PathError, WorkPath};
pub use evidence::{
    evaluate_red_green_pair, Evidence, OverlayCapture, OverlayFile, PairRefusal, RedGreenPolicy,
    VerificationOutcome,
};
pub use id::{
    ActorId, AssignmentId, AttemptId, BeadId, CapabilityId, IdError, ProfileName, SignalId,
};
pub use signal::{
    append_idempotent, binding_directives, directive_status, handoff_gate, unresolved,
    AppendOutcome, ConflictingDuplicate, DirectiveGateRefusal, DirectiveKind, DirectiveStatus,
    ReportKind, RequestKind, ResponseAction, ResponseKind, ScopeText, SenderFence, Seq, Signal,
    SignalBody, SubjectError, SubjectRef,
};
pub use lease::{validate_fencing, FencingError, FencingToken, Lease, Timestamp};
pub use lifecycle::{
    assignment_transition, attempt_transition, AssignmentAction, AssignmentState, AttemptAction,
    AttemptState, TransitionError,
};
