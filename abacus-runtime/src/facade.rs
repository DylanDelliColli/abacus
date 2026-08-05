//! The runtime facade: core's `RuntimePort` over any [`RuntimeProvider`].
//!
//! Interpretation lives here, in one place:
//!
//! - subject-mismatch refusal BEFORE any provider mutation;
//! - the opaque handle privately binding namespace, pane, and
//!   terminal/process generation — no provider structure escapes;
//! - generation fencing: mutating verbs refuse `HandleStale` on drift,
//!   `observe` alone reports it as the `StaleGeneration` observation;
//! - raw-status normalization with `Unknown` for unrecognized words;
//! - the ambiguity doctrine: a deadline AFTER submission is reported
//!   ambiguous, never an error and never a retry — mirroring the work
//!   seam's definitively-not-applied contract.

use abacus_core::Timestamp;
use abacus_core::ports::{
    ClockPort, ControlAction, DeliveryReport, EffectReport, EphemeralLaunchSecret, LaunchAttempt,
    LaunchCorrelation, LaunchOutcome, LaunchSpec, LaunchSubject, LivenessKind, LivenessObservation,
    RuntimeError, RuntimeHandle, RuntimePort, StartupDelivery, StopMode,
};

use crate::adapter::{RawRunError, RawSessionIdentity, RawStartupDelivery, RuntimeProvider};

/// Handle encoding version. A handle is opaque to callers; this
/// versioned encoding is what lets a handle survive a facade restart
/// without a private registry that would not.
const HANDLE_VERSION: &str = "arh1";

/// Deterministic, non-secret subject marker stored with the provider
/// session so recovery validates the (subject, correlation) pair
/// together (R5.17). Not a credential: it carries identity, not proof.
fn subject_fingerprint(subject: &LaunchSubject) -> String {
    match subject {
        LaunchSubject::WorkerAttempt {
            attempt,
            credential,
        } => format!("worker:{}:{}", attempt.as_str(), credential.as_str()),
        LaunchSubject::ActorActivation {
            actor,
            profile,
            generation,
            credential,
        } => format!(
            "actor:{}:{}:{}:{}",
            actor.as_str(),
            profile.as_str(),
            generation.as_str(),
            credential.as_str()
        ),
    }
}

fn encode_handle(identity: &RawSessionIdentity) -> RuntimeHandle {
    RuntimeHandle::new(format!(
        "{HANDLE_VERSION}|{}|{}|{}",
        identity.namespace, identity.pane, identity.generation
    ))
}

fn decode_handle(handle: &RuntimeHandle) -> Result<RawSessionIdentity, RuntimeError> {
    let mut parts = handle.as_str().split('|');
    match (
        parts.next(),
        parts.next(),
        parts.next(),
        parts.next(),
        parts.next(),
    ) {
        (Some(HANDLE_VERSION), Some(namespace), Some(pane), Some(generation), None)
            if !namespace.is_empty() && !pane.is_empty() && !generation.is_empty() =>
        {
            Ok(RawSessionIdentity {
                namespace: namespace.to_owned(),
                pane: pane.to_owned(),
                generation: generation.to_owned(),
            })
        }
        // A handle this facade never minted: not ours, not stale.
        _ => Err(RuntimeError::NotFound),
    }
}

/// Map a raw failure on a READ path. Reads never mutate, so both
/// deadline forms are definite; generation drift is stale.
fn read_error(error: RawRunError) -> RuntimeError {
    match error {
        RawRunError::Unavailable => RuntimeError::ProviderUnavailable,
        RawRunError::NotPermitted => RuntimeError::NotPermitted,
        RawRunError::NotFound => RuntimeError::NotFound,
        RawRunError::Rejected => RuntimeError::Rejected,
        RawRunError::DeadlineBeforeSubmission | RawRunError::DeadlineAfterSubmission => {
            RuntimeError::Timeout
        }
        RawRunError::MalformedResponse => RuntimeError::MalformedOutput,
        RawRunError::VersionMismatch => RuntimeError::VersionMismatch,
        RawRunError::GenerationMismatch => RuntimeError::HandleStale,
    }
}

/// Normalize the provider's raw status word. Unknown words are the
/// `Unknown` observation, never an error: detection drift must not
/// break the seam.
fn normalize_status(raw: &str) -> LivenessKind {
    match raw {
        "starting" => LivenessKind::Starting,
        "working" | "running" => LivenessKind::Running,
        "idle" | "done" => LivenessKind::Idle,
        "blocked" => LivenessKind::Blocked,
        "exited" => LivenessKind::Exited,
        "not_found" => LivenessKind::NotFound,
        "unavailable" => LivenessKind::Unavailable,
        _ => LivenessKind::Unknown,
    }
}

/// Implements [`RuntimePort`] over any [`RuntimeProvider`].
pub struct RuntimeFacade<P, C> {
    provider: P,
    clock: C,
    /// Repo-derived collision-resistant workspace namespace,
    /// supplied by the composition root (it is not in `LaunchSpec`).
    namespace: String,
    identity_verified: std::cell::Cell<bool>,
}

impl<P, C> RuntimeFacade<P, C>
where
    P: RuntimeProvider,
    C: ClockPort,
{
    pub fn new(provider: P, clock: C, namespace: impl Into<String>) -> Self {
        Self {
            provider,
            clock,
            namespace: namespace.into(),
            identity_verified: std::cell::Cell::new(false),
        }
    }

    pub fn provider(&self) -> &P {
        &self.provider
    }

    /// One cached pinned-identity probe per facade instance, before
    /// the first provider verb. Fails closed on drift.
    fn ensure_identity(&self) -> Result<(), RuntimeError> {
        if self.identity_verified.get() {
            return Ok(());
        }
        self.provider.verify_identity().map_err(read_error)?;
        self.identity_verified.set(true);
        Ok(())
    }

    fn observation(&self, kind: LivenessKind) -> LivenessObservation {
        LivenessObservation {
            observed_at: self.clock.now(),
            kind,
        }
    }

    /// Observe the identified session, honoring generation fencing:
    /// drift is the `StaleGeneration` OBSERVATION on this read path.
    fn observe_identity(
        &self,
        identity: &RawSessionIdentity,
        deadline: Timestamp,
    ) -> LivenessObservation {
        match self.provider.status(identity, deadline) {
            Ok(status) if status.generation == identity.generation => {
                self.observation(normalize_status(&status.raw))
            }
            Ok(_) | Err(RawRunError::GenerationMismatch) => {
                self.observation(LivenessKind::StaleGeneration)
            }
            Err(RawRunError::NotFound) => self.observation(LivenessKind::NotFound),
            Err(RawRunError::Unavailable) => self.observation(LivenessKind::Unavailable),
            Err(_) => self.observation(LivenessKind::Unknown),
        }
    }
}

impl<P, C> RuntimePort for RuntimeFacade<P, C>
where
    P: RuntimeProvider,
    C: ClockPort,
{
    fn launch(
        &self,
        spec: &LaunchSpec,
        secret: EphemeralLaunchSecret,
    ) -> Result<LaunchAttempt, RuntimeError> {
        // Cross-subject material swap: refused before ANY provider
        // interaction, including the identity probe.
        if secret.subject() != &spec.subject {
            return Err(RuntimeError::Rejected);
        }
        self.ensure_identity()?;
        self.provider
            .ensure_namespace(&self.namespace, spec.startup_deadline)
            .map_err(read_error)?;

        let fingerprint = subject_fingerprint(&spec.subject);
        let identity = match self.provider.start_agent(
            &self.namespace,
            spec.correlation.as_str(),
            &fingerprint,
            &spec.agent_kind,
            &spec.executable,
            &spec.args,
            spec.working_directory.as_str(),
            &spec.environment,
            spec.startup_deadline,
        ) {
            Ok(identity) => identity,
            // The provider may have created the session: recovery via
            // the pre-known (subject, correlation) pair, never retry.
            Err(RawRunError::DeadlineAfterSubmission | RawRunError::MalformedResponse) => {
                return Ok(LaunchAttempt::Ambiguous {
                    subject: spec.subject.clone(),
                    correlation: spec.correlation.clone(),
                });
            }
            Err(error) => return Err(read_error(error)),
        };

        let startup_delivery = match self.provider.deliver_startup(
            &identity,
            spec.envelope.content(),
            secret.reveal(),
            spec.delivery_deadline,
        ) {
            Ok(RawStartupDelivery::Accepted) => StartupDelivery::Submitted,
            Ok(RawStartupDelivery::Refused) => {
                StartupDelivery::NotDelivered(RuntimeError::Rejected)
            }
            // Submission may have gone through: never claim delivery,
            // never claim non-delivery.
            Err(RawRunError::DeadlineAfterSubmission | RawRunError::MalformedResponse) => {
                StartupDelivery::Ambiguous
            }
            Err(RawRunError::DeadlineBeforeSubmission) => {
                StartupDelivery::NotDelivered(RuntimeError::Timeout)
            }
            Err(error) => StartupDelivery::NotDelivered(read_error(error)),
        };

        let observation = self.observe_identity(&identity, spec.startup_deadline);
        Ok(LaunchAttempt::Launched(LaunchOutcome {
            handle: encode_handle(&identity),
            observation,
            startup_delivery,
        }))
    }

    fn recover_launch(
        &self,
        subject: &LaunchSubject,
        correlation: &LaunchCorrelation,
        deadline: Timestamp,
    ) -> Result<Option<LaunchOutcome>, RuntimeError> {
        self.ensure_identity()?;
        let fingerprint = subject_fingerprint(subject);
        let identity = match self
            .provider
            .lookup(
                &self.namespace,
                correlation.as_str(),
                &fingerprint,
                deadline,
            )
            .map_err(read_error)?
        {
            None => return Ok(None),
            Some(identity) => identity,
        };
        let observation = self.observe_identity(&identity, deadline);
        Ok(Some(LaunchOutcome {
            handle: encode_handle(&identity),
            observation,
            // The provider issues no receipt for a startup submission
            // after a lost response; a recovered `Submitted` would be
            // manufactured (R5.17).
            startup_delivery: StartupDelivery::Ambiguous,
        }))
    }

    fn observe(
        &self,
        handle: &RuntimeHandle,
        deadline: Timestamp,
    ) -> Result<LivenessObservation, RuntimeError> {
        self.ensure_identity()?;
        let identity = decode_handle(handle)?;
        Ok(self.observe_identity(&identity, deadline))
    }

    fn wait(
        &self,
        handle: &RuntimeHandle,
        desired: LivenessKind,
        deadline: Timestamp,
    ) -> Result<LivenessObservation, RuntimeError> {
        self.ensure_identity()?;
        let identity = decode_handle(handle)?;
        let desired_raw = match desired {
            LivenessKind::Starting => "starting",
            LivenessKind::Running => "working",
            LivenessKind::Idle => "idle",
            LivenessKind::Blocked => "blocked",
            LivenessKind::Exited => "exited",
            // Waiting for a non-provider condition is a caller defect.
            _ => return Err(RuntimeError::Rejected),
        };
        let status = self
            .provider
            .wait_status(&identity, desired_raw, deadline)
            .map_err(read_error)?;
        if status.generation != identity.generation {
            return Err(RuntimeError::HandleStale);
        }
        Ok(self.observation(normalize_status(&status.raw)))
    }

    fn read_view(
        &self,
        handle: &RuntimeHandle,
        max_bytes: u32,
        deadline: Timestamp,
    ) -> Result<String, RuntimeError> {
        self.ensure_identity()?;
        let identity = decode_handle(handle)?;
        self.provider
            .read_view(&identity, max_bytes, deadline)
            .map_err(read_error)
    }

    fn doorbell(
        &self,
        handle: &RuntimeHandle,
        deadline: Timestamp,
    ) -> Result<DeliveryReport, RuntimeError> {
        // Content-free by construction: the text names no Signal, no
        // subject, no content. The durable fact is already committed.
        self.prompt(
            handle,
            "workflow signal available; query unresolved",
            deadline,
        )
    }

    fn prompt(
        &self,
        handle: &RuntimeHandle,
        text: &str,
        deadline: Timestamp,
    ) -> Result<DeliveryReport, RuntimeError> {
        self.ensure_identity()?;
        let identity = decode_handle(handle)?;
        match self.provider.prompt(&identity, text, deadline) {
            Ok(()) => Ok(DeliveryReport::Submitted),
            Err(RawRunError::DeadlineAfterSubmission) => Ok(DeliveryReport::Ambiguous),
            Err(error) => Err(read_error(error)),
        }
    }

    fn control(
        &self,
        handle: &RuntimeHandle,
        action: ControlAction,
        deadline: Timestamp,
    ) -> Result<EffectReport, RuntimeError> {
        self.ensure_identity()?;
        let identity = decode_handle(handle)?;
        let ControlAction::CancelBlockedDialog = action;
        match self.provider.cancel_dialog(&identity, deadline) {
            Ok(()) => Ok(EffectReport::Applied),
            Err(RawRunError::DeadlineAfterSubmission) => Ok(EffectReport::Ambiguous),
            Err(error) => Err(read_error(error)),
        }
    }

    fn stop(
        &self,
        handle: &RuntimeHandle,
        mode: StopMode,
        deadline: Timestamp,
    ) -> Result<EffectReport, RuntimeError> {
        self.ensure_identity()?;
        let identity = decode_handle(handle)?;
        let forced = matches!(mode, StopMode::Forced);
        match self.provider.stop(&identity, forced, deadline) {
            Ok(()) => Ok(EffectReport::Applied),
            Err(RawRunError::DeadlineAfterSubmission) => Ok(EffectReport::Ambiguous),
            Err(error) => Err(read_error(error)),
        }
    }

    fn reassociate(
        &self,
        stale: &RuntimeHandle,
        deadline: Timestamp,
    ) -> Result<RuntimeHandle, RuntimeError> {
        self.ensure_identity()?;
        let identity = decode_handle(stale)?;
        let current = self
            .provider
            .current_identity(&identity.namespace, &identity.pane, deadline)
            .map_err(read_error)?;
        Ok(encode_handle(&current))
    }
}
