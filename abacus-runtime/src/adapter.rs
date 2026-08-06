//! Internal provider seam: raw runtime facts, no domain meaning.
//!
//! A [`RuntimeProvider`] reports what the substrate said — raw status
//! strings, provider session coordinates, submission outcomes. The
//! facade owns every interpretation: subject binding, generation
//! fencing, liveness normalization, and the ambiguity doctrine. An
//! adapter cannot manufacture a normalized [`LivenessKind`] or a
//! domain handle because neither exists in this vocabulary.
//!
//! Deadline semantics mirror the work seam's mutation contract: a
//! provider `Err` on a mutating verb asserts the effect definitively
//! did not happen; anything uncertain after submission MUST be
//! [`RawRunError::DeadlineAfterSubmission`] so the facade reports
//! ambiguity instead of inviting a retry.

use std::collections::BTreeMap;

use abacus_core::Timestamp;

/// Provider-private session coordinates. Never leaves this module
/// tree: the facade folds these into the opaque core handle.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawSessionIdentity {
    /// Collision-resistant workspace namespace (repo-derived label).
    pub namespace: String,
    /// Provider pane/session locator (e.g. `w1:p1`). Reusable by the
    /// provider; NEVER identity on its own.
    pub pane: String,
    /// Terminal/process generation. Changes on restart and on
    /// provider live-handoff; the fencing component of identity.
    pub generation: String,
}

/// One raw provider status observation, unstamped and uninterpreted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawStatus {
    /// The provider's own status word (e.g. `working`, `idle`,
    /// `blocked`, `done`, or anything a future manifest emits). The
    /// facade maps unknown words to `Unknown`, never an error.
    pub raw: String,
    /// The generation the observation was taken against.
    pub generation: String,
}

/// What one startup-Envelope submission reported.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RawStartupDelivery {
    /// The provider accepted one submission. Never proof of
    /// application.
    Accepted,
    /// Definitely not submitted; the session exists.
    Refused,
}

/// Transport-level failures at the provider seam.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RawRunError {
    /// The provider is unreachable; for mutating verbs this asserts
    /// the effect did not happen (nothing was submitted).
    Unavailable,
    /// Host approval absent (sandbox refusal). Fail closed.
    NotPermitted,
    /// The session/pane does not exist.
    NotFound,
    /// The provider rejected the request definitively.
    Rejected,
    /// The deadline elapsed BEFORE submission: definite non-effect.
    DeadlineBeforeSubmission,
    /// The deadline elapsed AFTER submission: the effect may have
    /// happened. The facade must report ambiguity, never retry.
    DeadlineAfterSubmission,
    /// Provider output could not be understood.
    MalformedResponse,
    /// The pinned provider identity check failed. Fail closed.
    VersionMismatch,
    /// The pane exists but its terminal/process generation differs
    /// from the identity's. The facade maps this to `HandleStale`;
    /// only explicit re-association may cross a generation change.
    GenerationMismatch,
}

/// Raw agent-session verbs against one provider. Every verb is
/// bounded by the caller-supplied deadline; the provider enforces it.
pub trait RuntimeProvider {
    /// Verify pinned provider identity (version/protocol). Called
    /// once per facade instance before the first session verb.
    fn verify_identity(&self) -> Result<(), RawRunError>;

    /// Idempotently ensure the repo's agent namespace exists. The
    /// adapter chooses the provider container (a Herdr workspace);
    /// callers name only the namespace.
    fn ensure_namespace(&self, namespace: &str, deadline: Timestamp) -> Result<(), RawRunError>;

    /// Start an agent process. Returns the full session identity
    /// including its terminal/process generation.
    /// `subject_fingerprint` is an opaque caller-derived marker the
    /// provider stores with the session, so recovery can validate the
    /// (subject, correlation) pair together. The provider never
    /// interprets it.
    #[allow(clippy::too_many_arguments)]
    fn start_agent(
        &self,
        namespace: &str,
        correlation: &str,
        subject_fingerprint: &str,
        agent_kind: &str,
        executable: &str,
        args: &[String],
        working_directory: &str,
        environment: &BTreeMap<String, String>,
        deadline: Timestamp,
    ) -> Result<RawSessionIdentity, RawRunError>;

    /// Deliver the sanitized Envelope as one submission over the
    /// pinned socket schema, host-side. MUST NOT place it in argv,
    /// environment, or logs.
    fn deliver_startup(
        &self,
        identity: &RawSessionIdentity,
        envelope_text: &str,
        deadline: Timestamp,
    ) -> Result<RawStartupDelivery, RawRunError>;

    /// Resolve a possibly-created session by its correlation marker,
    /// validating the stored subject fingerprint. Pure lookup: MUST
    /// NOT create anything. A correlation match with a DIFFERENT
    /// stored fingerprint is `Rejected` — loud, never a rebind, and
    /// distinct from absence.
    fn lookup(
        &self,
        namespace: &str,
        correlation: &str,
        subject_fingerprint: &str,
        deadline: Timestamp,
    ) -> Result<Option<RawSessionIdentity>, RawRunError>;

    /// The pane's CURRENT identity (fresh generation), for explicit
    /// re-association. Pure read.
    fn current_identity(
        &self,
        namespace: &str,
        pane: &str,
        deadline: Timestamp,
    ) -> Result<RawSessionIdentity, RawRunError>;

    /// Current raw status of the identified session.
    fn status(
        &self,
        identity: &RawSessionIdentity,
        deadline: Timestamp,
    ) -> Result<RawStatus, RawRunError>;

    /// Bounded wait until the provider reports the requested raw
    /// status word, or the deadline.
    fn wait_status(
        &self,
        identity: &RawSessionIdentity,
        desired_raw: &str,
        deadline: Timestamp,
    ) -> Result<RawStatus, RawRunError>;

    /// Bounded text view for diagnosis.
    fn read_view(
        &self,
        identity: &RawSessionIdentity,
        max_bytes: u32,
        deadline: Timestamp,
    ) -> Result<String, RawRunError>;

    /// Atomic prompt submission (text and Enter are the provider's
    /// concern; no keystroke synthesis exists at this seam).
    fn prompt(
        &self,
        identity: &RawSessionIdentity,
        text: &str,
        deadline: Timestamp,
    ) -> Result<(), RawRunError>;

    /// Cancel a blocked dialog — the only supported non-prompt input.
    fn cancel_dialog(
        &self,
        identity: &RawSessionIdentity,
        deadline: Timestamp,
    ) -> Result<(), RawRunError>;

    /// Stop the agent process; `forced` escalates per policy.
    fn stop(
        &self,
        identity: &RawSessionIdentity,
        forced: bool,
        deadline: Timestamp,
    ) -> Result<(), RawRunError>;
}

/// Shared-ownership delegation, so a test harness can hold the peer it
/// hands to the facade.
impl<P: RuntimeProvider> RuntimeProvider for std::rc::Rc<P> {
    fn verify_identity(&self) -> Result<(), RawRunError> {
        (**self).verify_identity()
    }

    fn ensure_namespace(&self, namespace: &str, deadline: Timestamp) -> Result<(), RawRunError> {
        (**self).ensure_namespace(namespace, deadline)
    }

    #[allow(clippy::too_many_arguments)]
    fn start_agent(
        &self,
        namespace: &str,
        correlation: &str,
        subject_fingerprint: &str,
        agent_kind: &str,
        executable: &str,
        args: &[String],
        working_directory: &str,
        environment: &BTreeMap<String, String>,
        deadline: Timestamp,
    ) -> Result<RawSessionIdentity, RawRunError> {
        (**self).start_agent(
            namespace,
            correlation,
            subject_fingerprint,
            agent_kind,
            executable,
            args,
            working_directory,
            environment,
            deadline,
        )
    }

    fn deliver_startup(
        &self,
        identity: &RawSessionIdentity,
        envelope_text: &str,
        deadline: Timestamp,
    ) -> Result<RawStartupDelivery, RawRunError> {
        (**self).deliver_startup(identity, envelope_text, deadline)
    }

    fn lookup(
        &self,
        namespace: &str,
        correlation: &str,
        subject_fingerprint: &str,
        deadline: Timestamp,
    ) -> Result<Option<RawSessionIdentity>, RawRunError> {
        (**self).lookup(namespace, correlation, subject_fingerprint, deadline)
    }

    fn current_identity(
        &self,
        namespace: &str,
        pane: &str,
        deadline: Timestamp,
    ) -> Result<RawSessionIdentity, RawRunError> {
        (**self).current_identity(namespace, pane, deadline)
    }

    fn status(
        &self,
        identity: &RawSessionIdentity,
        deadline: Timestamp,
    ) -> Result<RawStatus, RawRunError> {
        (**self).status(identity, deadline)
    }

    fn wait_status(
        &self,
        identity: &RawSessionIdentity,
        desired_raw: &str,
        deadline: Timestamp,
    ) -> Result<RawStatus, RawRunError> {
        (**self).wait_status(identity, desired_raw, deadline)
    }

    fn read_view(
        &self,
        identity: &RawSessionIdentity,
        max_bytes: u32,
        deadline: Timestamp,
    ) -> Result<String, RawRunError> {
        (**self).read_view(identity, max_bytes, deadline)
    }

    fn prompt(
        &self,
        identity: &RawSessionIdentity,
        text: &str,
        deadline: Timestamp,
    ) -> Result<(), RawRunError> {
        (**self).prompt(identity, text, deadline)
    }

    fn cancel_dialog(
        &self,
        identity: &RawSessionIdentity,
        deadline: Timestamp,
    ) -> Result<(), RawRunError> {
        (**self).cancel_dialog(identity, deadline)
    }

    fn stop(
        &self,
        identity: &RawSessionIdentity,
        forced: bool,
        deadline: Timestamp,
    ) -> Result<(), RawRunError> {
        (**self).stop(identity, forced, deadline)
    }
}
