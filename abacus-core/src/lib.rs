//! Domain language and pure rules for ABACUS.
//!
//! The binding contract is `README.md` in this folder. This crate owns
//! identifiers, the two authority classes, assignment/attempt lifecycles,
//! leases and fencing, typed Signals, evidence semantics, and the
//! provider-neutral ports. It is deterministic: no I/O, no clock or ID
//! generation of its own, and no knowledge of SQLite, subprocesses,
//! `br`, `bv`, or Herdr.

#![forbid(unsafe_code)]

pub mod authority;
pub mod id;
pub mod lifecycle;

pub use authority::AuthorityClass;
pub use id::{ActorId, AssignmentId, AttemptId, BeadId, IdError, ProfileName};
pub use lifecycle::{
    assignment_transition, attempt_transition, AssignmentAction, AssignmentState, AttemptAction,
    AttemptState, TransitionError,
};
