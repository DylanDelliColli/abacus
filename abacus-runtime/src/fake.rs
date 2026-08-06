//! Stateful fake protocol peer for hermetic runtime tests.
//!
//! Behaves like a miniature provider: namespaced sessions, correlation
//! and fingerprint storage, terminal generations that rotate on
//! demand, and scriptable failure modes. The portable suite in
//! [`crate::contract`] drives the real facade over this peer; the
//! `gyh.2` live Herdr adapter later runs the same suite's compatible
//! subset in the explicit live lane.

use std::cell::RefCell;
use std::collections::BTreeMap;

use abacus_core::Timestamp;

use crate::adapter::{
    RawRunError, RawSessionIdentity, RawStartupDelivery, RawStatus, RuntimeProvider,
};

/// Scriptable failure behavior for the NEXT matching verb.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FakeFailure {
    /// `start_agent` reports deadline-after-submission but the session
    /// IS created (the ambiguous-launch-recoverable case).
    AmbiguousStartCreated,
    /// `start_agent` reports deadline-after-submission and nothing was
    /// created (recovery finds none).
    AmbiguousStartLost,
    /// `deliver_startup` reports deadline-after-submission.
    AmbiguousDelivery,
    /// `deliver_startup` definitively refuses.
    RefusedDelivery,
    /// The next prompt reports deadline-after-submission.
    AmbiguousPrompt,
    /// The next prompt reports deadline-before-submission.
    PromptDeadlineBefore,
    /// The next stop reports deadline-after-submission.
    AmbiguousStop,
    /// Every verb refuses `NotPermitted` (sandbox refusal).
    NotPermitted,
    /// Identity verification fails (pinned-version drift).
    VersionDrift,
}

#[derive(Debug, Clone)]
struct FakeSession {
    identity: RawSessionIdentity,
    fingerprint: String,
    status: String,
    /// (text, deadline) of every accepted prompt, in order.
    prompts: Vec<(String, Timestamp)>,
    /// Every accepted startup Envelope, in order.
    startup_deliveries: Vec<String>,
    stops: Vec<bool>,
    cancels: u32,
    view: String,
}

#[derive(Debug, Clone)]
pub struct StartRecord {
    pub correlation: String,
    pub args: Vec<String>,
    pub environment: BTreeMap<String, String>,
}

#[derive(Debug, Default)]
struct FakeState {
    sessions: BTreeMap<String, FakeSession>,
    starts: Vec<StartRecord>,
    next_pane: u32,
    next_generation: u32,
    failure: Option<FakeFailure>,
    ensured: Vec<String>,
}

/// The fake peer. Interior-mutable so the facade holds it by value
/// while tests keep a handle through [`FakeRuntimePeer::clone`]-free
/// accessors on the facade's `provider()`.
#[derive(Debug, Default)]
pub struct FakeRuntimePeer {
    state: RefCell<FakeState>,
}

impl FakeRuntimePeer {
    pub fn new() -> Self {
        Self::default()
    }

    /// Arm exactly one scripted failure for the next matching verb.
    pub fn arm(&self, failure: FakeFailure) {
        self.state.borrow_mut().failure = Some(failure);
    }

    fn take_if(&self, failure: FakeFailure) -> bool {
        let mut state = self.state.borrow_mut();
        if state.failure == Some(failure) {
            state.failure = None;
            true
        } else {
            false
        }
    }

    fn not_permitted(&self) -> bool {
        self.state.borrow().failure == Some(FakeFailure::NotPermitted)
    }

    /// Set the raw status word the provider reports for a session.
    pub fn set_status(&self, correlation: &str, raw: &str) {
        let mut state = self.state.borrow_mut();
        let session = state
            .sessions
            .get_mut(correlation)
            .expect("fake session exists");
        session.status = raw.to_owned();
    }

    /// Rotate the session's terminal generation (restart/handoff).
    pub fn rotate_generation(&self, correlation: &str) {
        let mut state = self.state.borrow_mut();
        state.next_generation += 1;
        let generation = format!("gen-{}", state.next_generation);
        let session = state
            .sessions
            .get_mut(correlation)
            .expect("fake session exists");
        session.identity.generation = generation;
    }

    pub fn set_view(&self, correlation: &str, view: &str) {
        let mut state = self.state.borrow_mut();
        let session = state
            .sessions
            .get_mut(correlation)
            .expect("fake session exists");
        session.view = view.to_owned();
    }

    /// Prompts accepted for the session, in order.
    pub fn prompts(&self, correlation: &str) -> Vec<String> {
        self.state.borrow().sessions[correlation]
            .prompts
            .iter()
            .map(|(text, _)| text.clone())
            .collect()
    }

    /// Startup deliveries accepted for the session.
    pub fn startup_deliveries(&self, correlation: &str) -> Vec<String> {
        self.state.borrow().sessions[correlation]
            .startup_deliveries
            .clone()
    }

    /// Stop calls recorded for the session (`true` = forced).
    pub fn stops(&self, correlation: &str) -> Vec<bool> {
        self.state.borrow().sessions[correlation].stops.clone()
    }

    pub fn cancels(&self, correlation: &str) -> u32 {
        self.state.borrow().sessions[correlation].cancels
    }

    /// Every recorded `start_agent` invocation.
    pub fn starts(&self) -> Vec<StartRecord> {
        self.state.borrow().starts.clone()
    }

    pub fn session_count(&self) -> usize {
        self.state.borrow().sessions.len()
    }

    fn create_session(
        state: &mut FakeState,
        namespace: &str,
        correlation: &str,
        fingerprint: &str,
    ) -> RawSessionIdentity {
        state.next_pane += 1;
        state.next_generation += 1;
        let identity = RawSessionIdentity {
            namespace: namespace.to_owned(),
            pane: format!("p{}", state.next_pane),
            generation: format!("gen-{}", state.next_generation),
        };
        state.sessions.insert(
            correlation.to_owned(),
            FakeSession {
                identity: identity.clone(),
                fingerprint: fingerprint.to_owned(),
                status: "starting".to_owned(),
                prompts: Vec::new(),
                startup_deliveries: Vec::new(),
                stops: Vec::new(),
                cancels: 0,
                view: String::new(),
            },
        );
        identity
    }

    /// The session owning this exact identity, honoring generations.
    fn resolve<'a>(
        state: &'a mut FakeState,
        identity: &RawSessionIdentity,
    ) -> Result<&'a mut FakeSession, RawRunError> {
        let session = state
            .sessions
            .values_mut()
            .find(|session| {
                session.identity.namespace == identity.namespace
                    && session.identity.pane == identity.pane
            })
            .ok_or(RawRunError::NotFound)?;
        if session.identity.generation != identity.generation {
            return Err(RawRunError::GenerationMismatch);
        }
        Ok(session)
    }
}

impl RuntimeProvider for FakeRuntimePeer {
    fn verify_identity(&self) -> Result<(), RawRunError> {
        if self.take_if(FakeFailure::VersionDrift) {
            return Err(RawRunError::VersionMismatch);
        }
        if self.not_permitted() {
            return Err(RawRunError::NotPermitted);
        }
        Ok(())
    }

    fn ensure_namespace(&self, namespace: &str, _deadline: Timestamp) -> Result<(), RawRunError> {
        if self.not_permitted() {
            return Err(RawRunError::NotPermitted);
        }
        self.state.borrow_mut().ensured.push(namespace.to_owned());
        Ok(())
    }

    fn start_agent(
        &self,
        namespace: &str,
        correlation: &str,
        subject_fingerprint: &str,
        _agent_kind: &str,
        _executable: &str,
        args: &[String],
        _working_directory: &str,
        environment: &BTreeMap<String, String>,
        _deadline: Timestamp,
    ) -> Result<RawSessionIdentity, RawRunError> {
        if self.not_permitted() {
            return Err(RawRunError::NotPermitted);
        }
        let mut state = self.state.borrow_mut();
        state.starts.push(StartRecord {
            correlation: correlation.to_owned(),
            args: args.to_vec(),
            environment: environment.clone(),
        });
        drop(state);
        if self.take_if(FakeFailure::AmbiguousStartLost) {
            return Err(RawRunError::DeadlineAfterSubmission);
        }
        let created_ambiguous = self.take_if(FakeFailure::AmbiguousStartCreated);
        let mut state = self.state.borrow_mut();
        let identity =
            Self::create_session(&mut state, namespace, correlation, subject_fingerprint);
        if created_ambiguous {
            return Err(RawRunError::DeadlineAfterSubmission);
        }
        Ok(identity)
    }

    fn deliver_startup(
        &self,
        identity: &RawSessionIdentity,
        envelope_text: &str,
        _deadline: Timestamp,
    ) -> Result<RawStartupDelivery, RawRunError> {
        let ambiguous = self.take_if(FakeFailure::AmbiguousDelivery);
        if self.take_if(FakeFailure::RefusedDelivery) {
            return Ok(RawStartupDelivery::Refused);
        }
        let mut state = self.state.borrow_mut();
        let session = Self::resolve(&mut state, identity)?;
        if ambiguous {
            // The submission WENT OUT; only the acknowledgement was
            // lost. Recording it is what lets the portable suite prove
            // The Envelope reached the provider, so no automatic
            // redelivery may occur after an ambiguous response.
            session.startup_deliveries.push(envelope_text.to_owned());
            session.status = "working".to_owned();
            return Err(RawRunError::DeadlineAfterSubmission);
        }
        session.startup_deliveries.push(envelope_text.to_owned());
        session.status = "working".to_owned();
        Ok(RawStartupDelivery::Accepted)
    }

    fn lookup(
        &self,
        namespace: &str,
        correlation: &str,
        subject_fingerprint: &str,
        _deadline: Timestamp,
    ) -> Result<Option<RawSessionIdentity>, RawRunError> {
        let state = self.state.borrow();
        match state.sessions.get(correlation) {
            None => Ok(None),
            Some(session) if session.identity.namespace != namespace => Ok(None),
            Some(session) if session.fingerprint != subject_fingerprint => {
                // Correlation matches a session bound to a DIFFERENT
                // subject: loud, never a rebind.
                Err(RawRunError::Rejected)
            }
            Some(session) => Ok(Some(session.identity.clone())),
        }
    }

    fn current_identity(
        &self,
        namespace: &str,
        pane: &str,
        _deadline: Timestamp,
    ) -> Result<RawSessionIdentity, RawRunError> {
        let state = self.state.borrow();
        state
            .sessions
            .values()
            .find(|session| {
                session.identity.namespace == namespace && session.identity.pane == pane
            })
            .map(|session| session.identity.clone())
            .ok_or(RawRunError::NotFound)
    }

    fn status(
        &self,
        identity: &RawSessionIdentity,
        _deadline: Timestamp,
    ) -> Result<RawStatus, RawRunError> {
        let mut state = self.state.borrow_mut();
        let session = Self::resolve(&mut state, identity)?;
        Ok(RawStatus {
            raw: session.status.clone(),
            generation: session.identity.generation.clone(),
        })
    }

    fn wait_status(
        &self,
        identity: &RawSessionIdentity,
        desired_raw: &str,
        _deadline: Timestamp,
    ) -> Result<RawStatus, RawRunError> {
        let mut state = self.state.borrow_mut();
        let session = Self::resolve(&mut state, identity)?;
        if session.status == desired_raw {
            Ok(RawStatus {
                raw: session.status.clone(),
                generation: session.identity.generation.clone(),
            })
        } else {
            Err(RawRunError::DeadlineBeforeSubmission)
        }
    }

    fn read_view(
        &self,
        identity: &RawSessionIdentity,
        max_bytes: u32,
        _deadline: Timestamp,
    ) -> Result<String, RawRunError> {
        let mut state = self.state.borrow_mut();
        let session = Self::resolve(&mut state, identity)?;
        let mut view = session.view.clone();
        view.truncate(max_bytes as usize);
        Ok(view)
    }

    fn prompt(
        &self,
        identity: &RawSessionIdentity,
        text: &str,
        deadline: Timestamp,
    ) -> Result<(), RawRunError> {
        if self.take_if(FakeFailure::PromptDeadlineBefore) {
            return Err(RawRunError::DeadlineBeforeSubmission);
        }
        let ambiguous = self.take_if(FakeFailure::AmbiguousPrompt);
        let mut state = self.state.borrow_mut();
        let session = Self::resolve(&mut state, identity)?;
        if ambiguous {
            // The prompt WAS submitted; the acknowledgement was lost.
            session.prompts.push((text.to_owned(), deadline));
            return Err(RawRunError::DeadlineAfterSubmission);
        }
        session.prompts.push((text.to_owned(), deadline));
        Ok(())
    }

    fn cancel_dialog(
        &self,
        identity: &RawSessionIdentity,
        _deadline: Timestamp,
    ) -> Result<(), RawRunError> {
        let mut state = self.state.borrow_mut();
        let session = Self::resolve(&mut state, identity)?;
        session.cancels += 1;
        Ok(())
    }

    fn stop(
        &self,
        identity: &RawSessionIdentity,
        forced: bool,
        _deadline: Timestamp,
    ) -> Result<(), RawRunError> {
        let ambiguous = self.take_if(FakeFailure::AmbiguousStop);
        let mut state = self.state.borrow_mut();
        let session = Self::resolve(&mut state, identity)?;
        session.stops.push(forced);
        if ambiguous {
            return Err(RawRunError::DeadlineAfterSubmission);
        }
        session.status = "exited".to_owned();
        Ok(())
    }
}
