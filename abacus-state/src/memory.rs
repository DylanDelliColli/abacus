//! Transactional in-memory implementation of [`WorkflowStatePort`].
//!
//! This is a real empty-state implementation for hermetic use-case tests,
//! not a pre-seeded mock. One mutex is the transaction boundary: validation,
//! domain mutation, causal ordering, and idempotency ownership become visible
//! together or not at all. Time is an explicit constructor input so tests do
//! not sleep or read the host clock.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex, MutexGuard};

use abacus_core::ports::*;
use abacus_core::signal::validate_subject;
use abacus_core::{
    ActorId, AssignmentAction, AssignmentId, AssignmentState, AttemptAction, AttemptId,
    AttemptState, AuthorityClass, ContentHash, CredentialId, DecisionActor, DirectiveKind,
    HandoffId, Lease, OccupancyClass, OperationId, ProfileName, ResponseAction, ResponseKind, Seq,
    Signal, SignalBody, SignalDraft, SignalId, SubjectRef, Timestamp, assignment_transition,
    attempt_transition, binding_directives, handoff_gate, next_attempt_allowed, retry_within_cap,
    unresolved, worker_append_gate,
};
use subtle::ConstantTimeEq;

#[derive(Clone)]
pub(crate) struct CredentialBinding {
    pub(crate) credential: CredentialId,
    pub(crate) digest: ContentHash,
    pub(crate) actor: ActorId,
    pub(crate) profile: ProfileName,
    pub(crate) assignment: Option<AssignmentId>,
    pub(crate) revoked: bool,
}

#[derive(Clone)]
pub(crate) struct AttemptEntry {
    pub(crate) record: AttemptRecord,
    pub(crate) state: AttemptState,
    pub(crate) authorizing: OperationId,
}

#[derive(Clone)]
pub(crate) struct AssignmentEntry {
    pub(crate) record: AssignmentRecord,
    pub(crate) state: AssignmentState,
    pub(crate) attempts: Vec<AttemptEntry>,
}

#[derive(Clone)]
pub(crate) struct State {
    pub(crate) head: u64,
    pub(crate) operations: BTreeMap<String, String>,
    pub(crate) committed_operations: BTreeSet<String>,
    pub(crate) bootstrap_complete: bool,
    pub(crate) actor_classes: BTreeMap<String, AuthorityClass>,
    pub(crate) active_members: BTreeMap<String, BTreeSet<String>>,
    pub(crate) credentials: BTreeMap<String, CredentialBinding>,
    pub(crate) credential_owners: BTreeMap<String, String>,
    pub(crate) assignments: BTreeMap<String, AssignmentEntry>,
    pub(crate) attempt_owners: BTreeMap<String, String>,
    pub(crate) signals: Vec<Signal>,
    pub(crate) response_actions: Vec<ResponseAction>,
    pub(crate) report_outcomes: BTreeMap<String, ReportOutcome>,
    pub(crate) evidence: Vec<EvidenceRecord>,
    pub(crate) evidence_outcomes: BTreeMap<String, EvidenceOutcome>,
    pub(crate) submissions: BTreeMap<String, (String, SubmissionOutcome)>,
    pub(crate) handoffs: BTreeMap<String, HandoffRecord>,
    pub(crate) decisions: BTreeMap<String, DecisionRecord>,
    pub(crate) envelopes: BTreeMap<String, EnvelopeSnapshot>,
    pub(crate) handles: BTreeMap<String, RuntimeHandle>,
    pub(crate) projections: BTreeMap<String, PendingApplication>,
    pub(crate) application_attempts: BTreeMap<String, Vec<ApplicationAttempt>>,
    pub(crate) receipts: BTreeMap<String, ApplicationReceipt>,
    pub(crate) audit_events: BTreeMap<u64, AuditEvent>,
    pub(crate) runtime_observations: BTreeMap<String, RuntimeObservationRecord>,
}

impl State {
    pub(crate) fn new() -> Self {
        Self {
            head: 0,
            operations: BTreeMap::new(),
            committed_operations: BTreeSet::new(),
            bootstrap_complete: false,
            actor_classes: BTreeMap::new(),
            active_members: BTreeMap::new(),
            credentials: BTreeMap::new(),
            credential_owners: BTreeMap::new(),
            assignments: BTreeMap::new(),
            attempt_owners: BTreeMap::new(),
            signals: Vec::new(),
            response_actions: Vec::new(),
            report_outcomes: BTreeMap::new(),
            evidence: Vec::new(),
            evidence_outcomes: BTreeMap::new(),
            submissions: BTreeMap::new(),
            handoffs: BTreeMap::new(),
            decisions: BTreeMap::new(),
            envelopes: BTreeMap::new(),
            handles: BTreeMap::new(),
            projections: BTreeMap::new(),
            application_attempts: BTreeMap::new(),
            receipts: BTreeMap::new(),
            audit_events: BTreeMap::new(),
            runtime_observations: BTreeMap::new(),
        }
    }
}

/// Empty, deterministic workflow state for hermetic use-case tests.
///
/// Every method implements the same public port the SQLite state will use.
/// The portable suite in `crate::contract` is intentionally expressed only
/// through that port so both implementations inherit the same expectations.
pub struct InMemoryState<C> {
    clock: C,
    inner: Mutex<State>,
}

impl<C> InMemoryState<C> {
    pub fn new(clock: C) -> Self {
        Self {
            clock,
            inner: Mutex::new(State::new()),
        }
    }

    pub(crate) fn from_state(clock: C, state: State) -> Self {
        Self {
            clock,
            inner: Mutex::new(state),
        }
    }

    pub(crate) fn snapshot(&self) -> Result<State, StateError> {
        Ok(self.lock()?.clone())
    }

    pub(crate) fn restore(&self, state: State) -> Result<(), StateError> {
        *self.lock()? = state;
        Ok(())
    }

    fn lock(&self) -> Result<MutexGuard<'_, State>, StateError> {
        self.inner.lock().map_err(|_| StateError::Corrupt)
    }

    fn stored_identity(
        identity: Result<String, crate::stored::StoredError>,
    ) -> Result<String, StateError> {
        identity.map_err(|_| StateError::Corrupt)
    }

    fn operation_key(verb: &str, operation: &OperationId) -> String {
        format!("{verb}:{}", operation.as_str())
    }

    fn replay(
        state: &State,
        verb: &str,
        operation: &OperationId,
        request: &str,
    ) -> Result<bool, StateError> {
        match state.operations.get(&Self::operation_key(verb, operation)) {
            None => Ok(false),
            Some(stored) if stored == request => Ok(true),
            Some(_) => Err(StateError::ConflictingOperation),
        }
    }

    fn remember(state: &mut State, verb: &str, operation: &OperationId, request: String) {
        state
            .operations
            .insert(Self::operation_key(verb, operation), request);
        state
            .committed_operations
            .insert(operation.as_str().to_owned());
    }

    fn next_seq(state: &mut State) -> Seq {
        state.head += 1;
        Seq(state.head)
    }

    fn current_head(state: &State) -> Seq {
        Seq(state.head)
    }

    fn worker_locator(attempt: &AttemptId) -> String {
        format!("attempt:{}", attempt.as_str())
    }

    fn activation_locator(
        actor: &ActorId,
        profile: &ProfileName,
        generation: &OperationId,
    ) -> String {
        format!(
            "activation:{}:{}:{}",
            actor.as_str(),
            profile.as_str(),
            generation.as_str()
        )
    }

    fn owner_locator(subject: &LaunchSubject) -> String {
        match subject {
            LaunchSubject::WorkerAttempt { attempt, .. } => Self::worker_locator(attempt),
            LaunchSubject::ActorActivation {
                actor,
                profile,
                generation,
                ..
            } => Self::activation_locator(actor, profile, generation),
        }
    }

    fn association_key(subject: &LaunchSubject) -> String {
        format!(
            "{}:credential:{}",
            Self::owner_locator(subject),
            subject.credential().as_str()
        )
    }

    fn resolve_subject<'a>(
        state: &'a State,
        subject: &LaunchSubject,
    ) -> Result<&'a CredentialBinding, StateError> {
        let binding = state
            .credentials
            .get(&Self::owner_locator(subject))
            .ok_or(StateError::UnknownRecord)?;
        if &binding.credential != subject.credential() {
            return Err(StateError::CredentialBindingMismatch);
        }
        Ok(binding)
    }

    fn credential_available(
        state: &State,
        credential: &CredentialId,
        owner: &str,
    ) -> Result<(), StateError> {
        match state.credential_owners.get(credential.as_str()) {
            None => Ok(()),
            Some(existing) if existing == owner => Ok(()),
            Some(_) => Err(StateError::ConflictingOperation),
        }
    }

    fn insert_credential(state: &mut State, owner: String, binding: CredentialBinding) {
        state
            .credential_owners
            .insert(binding.credential.as_str().to_owned(), owner.clone());
        state.credentials.insert(owner, binding);
    }

    fn attempt_entry<'a>(
        state: &'a State,
        assignment: &AssignmentId,
        attempt: &AttemptId,
    ) -> Option<&'a AttemptEntry> {
        state
            .assignments
            .get(assignment.as_str())?
            .attempts
            .iter()
            .find(|entry| &entry.record.id == attempt)
    }

    fn attempt_entry_mut<'a>(
        state: &'a mut State,
        assignment: &AssignmentId,
        attempt: &AttemptId,
    ) -> Option<&'a mut AttemptEntry> {
        state
            .assignments
            .get_mut(assignment.as_str())?
            .attempts
            .iter_mut()
            .find(|entry| &entry.record.id == attempt)
    }

    fn validate_fence(state: &State, call: &FencedCall, now: Timestamp) -> Result<(), StateError> {
        let assignment = state
            .assignments
            .get(call.assignment.as_str())
            .ok_or(StateError::IncoherentBundle)?;
        let attempt = assignment
            .attempts
            .iter()
            .find(|entry| entry.record.id == call.attempt)
            .ok_or(StateError::IncoherentBundle)?;
        if assignment.record.worker.actor != call.actor {
            return Err(StateError::ActorMismatch);
        }
        if attempt.record.lease.token != call.token {
            return Err(StateError::StaleFencing);
        }
        if now > attempt.record.lease.expires_at {
            return Err(StateError::LeaseExpired);
        }
        if assignment.state.is_terminal() || attempt.state.is_ended() {
            return Err(StateError::IncoherentBundle);
        }
        Ok(())
    }

    fn validate_active_attempt(
        state: &State,
        call: &FencedCall,
        now: Timestamp,
    ) -> Result<(), StateError> {
        Self::validate_fence(state, call, now)?;
        if Self::attempt_entry(state, &call.assignment, &call.attempt)
            .is_none_or(|entry| entry.state != AttemptState::Active)
        {
            return Err(StateError::IncoherentBundle);
        }
        Ok(())
    }

    fn call_identity(call: &FencedCall) -> String {
        format!(
            "asg={}|att={}|actor={}|tok={}",
            call.assignment.as_str(),
            call.attempt.as_str(),
            call.actor.as_str(),
            call.token.0
        )
    }

    fn action_identity(action: &FencedAction) -> String {
        format!(
            "{}|responds_to={}",
            Self::call_identity(&action.call),
            action.responds_to.as_ref().map_or("-", SignalId::as_str)
        )
    }

    fn validate_response_target(state: &State, action: &FencedAction) -> Result<(), StateError> {
        let Some(target) = &action.responds_to else {
            return Ok(());
        };
        let signal = state
            .signals
            .iter()
            .find(|signal| &signal.id == target)
            .ok_or(StateError::UnknownRecord)?;
        match (&signal.subject, &signal.body) {
            (
                SubjectRef::Attempt(subject),
                SignalBody::Directive {
                    assignment,
                    attempt,
                    ..
                },
            ) if assignment == &action.call.assignment
                && subject == &action.call.attempt
                && attempt == &action.call.attempt =>
            {
                Ok(())
            }
            _ => Err(StateError::IncoherentBundle),
        }
    }

    fn worker_action(action: &FencedAction, seq: Seq) -> ResponseAction {
        ResponseAction {
            seq,
            kind: ResponseKind::WorkerAction {
                attempt: action.call.attempt.clone(),
                responds_to: action.responds_to.clone(),
            },
        }
    }

    fn commit_fenced_call(
        state: &mut State,
        action: Option<&FencedAction>,
        substantive: bool,
    ) -> Seq {
        let seq = Self::next_seq(state);
        if substantive && let Some(action) = action {
            state
                .response_actions
                .push(Self::worker_action(action, seq));
        }
        seq
    }

    fn response(state: &State, attempt: &AttemptId, applied: StateApplied) -> FencedResponse {
        FencedResponse {
            applied,
            binding_directives: binding_directives(
                attempt,
                &state.signals,
                &state.response_actions,
            )
            .into_iter()
            .cloned()
            .collect(),
            head: Self::current_head(state),
        }
    }

    fn signal_replay(state: &State, draft: &SignalDraft) -> Result<Option<Signal>, StateError> {
        let Some(existing) = state.signals.iter().find(|signal| signal.id == draft.id) else {
            return Ok(None);
        };
        let redraft = SignalDraft {
            id: existing.id.clone(),
            sender: existing.sender.clone(),
            subject: existing.subject.clone(),
            body: existing.body.clone(),
        };
        if redraft == *draft {
            Ok(Some(existing.clone()))
        } else {
            Err(StateError::ConflictingOperation)
        }
    }

    fn commit_new_signal(state: &mut State, draft: &SignalDraft) -> Result<Signal, StateError> {
        validate_subject(&draft.body, &draft.subject).map_err(|_| StateError::IncoherentBundle)?;
        let signal = draft.clone().commit(Self::next_seq(state));
        if let SignalBody::Directive { attempt, .. } = &signal.body {
            state.response_actions.push(ResponseAction {
                seq: signal.seq,
                kind: ResponseKind::DirectiveCommitted {
                    attempt: attempt.clone(),
                    directive: signal.id.clone(),
                },
            });
        }
        state.signals.push(signal.clone());
        Ok(signal)
    }

    fn revoke_attempt_credential(state: &mut State, attempt: &AttemptId) {
        if let Some(binding) = state.credentials.get_mut(&Self::worker_locator(attempt)) {
            binding.revoked = true;
        }
    }

    fn validate_decision_authority(
        assignment: &AssignmentEntry,
        authority: &DecisionActor,
    ) -> Result<(), StateError> {
        if &assignment.record.decision_actor == authority {
            Ok(())
        } else {
            Err(StateError::ActorMismatch)
        }
    }

    fn worker_initiator(state: &State, call: &FencedCall) -> Result<AuditInitiator, StateError> {
        let assignment = state
            .assignments
            .get(call.assignment.as_str())
            .ok_or(StateError::IncoherentBundle)?;
        Ok(AuditInitiator::WorkerBinding {
            actor: assignment.record.worker.clone(),
            assignment: call.assignment.clone(),
            attempt: call.attempt.clone(),
        })
    }

    fn append_audit(
        state: &mut State,
        seq: Seq,
        at: Timestamp,
        initiator: AuditInitiator,
        operation: AuditOperation,
        subject: AuditSubject,
        kind: AuditKind,
    ) {
        let prior = state.audit_events.insert(
            seq.0,
            AuditEvent {
                seq,
                at,
                initiator,
                operation,
                subject,
                kind,
            },
        );
        assert!(prior.is_none(), "one audit event per Ledger position");
    }

    fn subject_authorizing_operation(
        state: &State,
        subject: &LaunchSubject,
    ) -> Result<OperationId, StateError> {
        let operation = match subject {
            LaunchSubject::WorkerAttempt { attempt, .. } => {
                let assignment = state
                    .attempt_owners
                    .get(attempt.as_str())
                    .and_then(|assignment| state.assignments.get(assignment))
                    .ok_or(StateError::Corrupt)?;
                assignment
                    .attempts
                    .iter()
                    .find(|entry| &entry.record.id == attempt)
                    .map(|entry| entry.authorizing.clone())
                    .ok_or(StateError::Corrupt)?
            }
            LaunchSubject::ActorActivation { generation, .. } => generation.clone(),
        };
        if state.committed_operations.contains(operation.as_str()) {
            Ok(operation)
        } else {
            Err(StateError::Corrupt)
        }
    }

    fn system_projection(
        state: &State,
        authorizing: &OperationId,
    ) -> Result<AuditInitiator, StateError> {
        if state.committed_operations.contains(authorizing.as_str()) {
            Ok(AuditInitiator::SystemProjection {
                authorizing: authorizing.clone(),
            })
        } else {
            Err(StateError::Corrupt)
        }
    }

    fn receipt_candidate(state: &State, target: &OperationId) -> Option<ReceiptCandidate> {
        // Attempts append under the Ledger lock, and SQLite reconstructs
        // each target's vector by committed sequence. The first Applied
        // entry is therefore the portable earliest-Ledger-order choice.
        state
            .application_attempts
            .get(target.as_str())?
            .iter()
            .find_map(|attempt| match &attempt.outcome {
                ApplicationOutcome::Applied { after, .. } => Some(ReceiptCandidate {
                    attempt: attempt.id.clone(),
                    after: after.clone(),
                }),
                ApplicationOutcome::FoundPresent { .. }
                | ApplicationOutcome::ObservedAfterAmbiguous { .. }
                | ApplicationOutcome::Failed { .. }
                | ApplicationOutcome::Ambiguous => None,
            })
    }

    fn superseding_projection<'a>(
        state: &'a State,
        projection: &PendingApplication,
    ) -> Option<&'a PendingApplication> {
        if projection.projection != WorkProjection::MarkInProgress {
            return None;
        }
        state
            .projections
            .values()
            .filter(|candidate| {
                candidate.assignment == projection.assignment
                    && candidate.committed_at > projection.committed_at
                    && matches!(candidate.projection, WorkProjection::Close { .. })
            })
            .min_by_key(|candidate| candidate.committed_at)
    }

    fn pending_view(state: &State, projection: &PendingApplication) -> PendingApplication {
        let mut application = projection.clone();
        application.receipt_candidate = Self::receipt_candidate(state, &projection.operation);
        application
    }
}

/// Cloneable clock control used by hermetic state-contract fixtures.
/// Production state injects its Scribe clock through the same core port.
#[derive(Clone)]
pub struct ManualClock {
    now: Arc<Mutex<Timestamp>>,
}

impl ManualClock {
    pub fn new(now: Timestamp) -> Self {
        Self {
            now: Arc::new(Mutex::new(now)),
        }
    }

    pub fn set(&self, now: Timestamp) {
        // ClockPort cannot return an error. If a test panicked while holding
        // the clock, retain and update the guarded value rather than hiding a
        // later state assertion behind a second panic.
        *self
            .now
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = now;
    }
}

impl ClockPort for ManualClock {
    fn now(&self) -> Timestamp {
        *self
            .now
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

impl<C: ClockPort> WorkflowStatePort for InMemoryState<C> {
    fn open_assignment(&self, opening: &AssignmentOpening) -> Result<StateApplied, StateError> {
        if opening.assignment.id != opening.authorizing.assignment
            || opening.first_attempt.assignment != opening.assignment.id
            || opening.first_attempt.id != opening.authorizing.first_attempt
            || opening.authorizing.authority.actor != opening.assignment.decision_actor
            || opening.assignment.worker.class != AuthorityClass::Worker
            || opening.assignment.decision_actor.class != AuthorityClass::Orchestrator
        {
            return Err(StateError::IncoherentBundle);
        }

        let at = self.clock.now();
        let mut state = self.lock()?;
        let request = Self::stored_identity(crate::stored::assignment_opening_identity(opening))?;
        if Self::replay(
            &state,
            "open_assignment",
            &opening.authorizing.operation,
            &request,
        )? {
            return Ok(StateApplied::AlreadyApplied);
        }
        if state
            .assignments
            .contains_key(opening.assignment.id.as_str())
            || state
                .attempt_owners
                .contains_key(opening.first_attempt.id.as_str())
        {
            return Err(StateError::ConflictingOperation);
        }
        let worker = &opening.assignment.worker;
        if let Some(existing) = state.actor_classes.get(worker.actor.as_str())
            && *existing != worker.class
        {
            return Err(StateError::ActorClassMismatch);
        }
        let credential_owner = Self::worker_locator(&opening.first_attempt.id);
        Self::credential_available(&state, &opening.worker_credential.id, &credential_owner)?;

        let seq = Self::next_seq(&mut state);
        state
            .actor_classes
            .insert(worker.actor.as_str().to_owned(), worker.class);
        state
            .active_members
            .entry(worker.profile.as_str().to_owned())
            .or_default()
            .insert(worker.actor.as_str().to_owned());
        Self::insert_credential(
            &mut state,
            credential_owner,
            CredentialBinding {
                credential: opening.worker_credential.id.clone(),
                digest: opening.worker_credential.digest.clone(),
                actor: worker.actor.clone(),
                profile: worker.profile.clone(),
                assignment: Some(opening.assignment.id.clone()),
                revoked: false,
            },
        );
        state.attempt_owners.insert(
            opening.first_attempt.id.as_str().to_owned(),
            opening.assignment.id.as_str().to_owned(),
        );
        state.assignments.insert(
            opening.assignment.id.as_str().to_owned(),
            AssignmentEntry {
                record: opening.assignment.clone(),
                state: AssignmentState::Active,
                attempts: vec![AttemptEntry {
                    record: opening.first_attempt.clone(),
                    state: AttemptState::Active,
                    authorizing: opening.authorizing.operation.clone(),
                }],
            },
        );
        state.projections.insert(
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
        Self::append_audit(
            &mut state,
            seq,
            at,
            AuditInitiator::Authority(opening.authorizing.authority.clone()),
            AuditOperation::Operation(opening.authorizing.operation.clone()),
            AuditSubject::Workflow(SubjectRef::Assignment(opening.assignment.id.clone())),
            AuditKind::AssignmentOpened,
        );
        Self::remember(
            &mut state,
            "open_assignment",
            &opening.authorizing.operation,
            request,
        );
        Ok(StateApplied::Applied)
    }

    fn append_attempt(&self, opening: &AttemptOpening) -> Result<StateApplied, StateError> {
        if opening.attempt.assignment != opening.authorizing.assignment {
            return Err(StateError::IncoherentBundle);
        }
        let at = self.clock.now();
        let mut state = self.lock()?;
        let request = Self::stored_identity(crate::stored::attempt_opening_identity(opening))?;
        if Self::replay(
            &state,
            "append_attempt",
            &opening.authorizing.operation,
            &request,
        )? {
            return Ok(StateApplied::AlreadyApplied);
        }
        if state
            .attempt_owners
            .contains_key(opening.attempt.id.as_str())
        {
            return Err(StateError::ConflictingOperation);
        }
        let credential_owner = Self::worker_locator(&opening.attempt.id);
        Self::credential_available(&state, &opening.worker_credential.id, &credential_owner)?;

        let assignment = state
            .assignments
            .get(opening.attempt.assignment.as_str())
            .ok_or(StateError::UnknownRecord)?;
        Self::validate_decision_authority(assignment, &opening.authorizing.authority.actor)?;
        next_attempt_allowed(
            assignment.state,
            assignment.attempts.last().map(|attempt| attempt.state),
        )
        .map_err(|_| StateError::IncoherentBundle)?;
        retry_within_cap(
            &assignment.record.attempt_policy,
            assignment.attempts.len() as u32,
        )
        .map_err(|_| StateError::IncoherentBundle)?;
        if assignment
            .attempts
            .last()
            .is_some_and(|prior| opening.attempt.lease.token.0 <= prior.record.lease.token.0)
        {
            return Err(StateError::IncoherentBundle);
        }
        let worker = assignment.record.worker.clone();
        let assignment_id = assignment.record.id.clone();

        let seq = Self::next_seq(&mut state);
        Self::insert_credential(
            &mut state,
            credential_owner,
            CredentialBinding {
                credential: opening.worker_credential.id.clone(),
                digest: opening.worker_credential.digest.clone(),
                actor: worker.actor,
                profile: worker.profile,
                assignment: Some(assignment_id.clone()),
                revoked: false,
            },
        );
        state.attempt_owners.insert(
            opening.attempt.id.as_str().to_owned(),
            assignment_id.as_str().to_owned(),
        );
        state
            .assignments
            .get_mut(assignment_id.as_str())
            .expect("assignment validated above")
            .attempts
            .push(AttemptEntry {
                record: opening.attempt.clone(),
                state: AttemptState::Active,
                authorizing: opening.authorizing.operation.clone(),
            });
        Self::append_audit(
            &mut state,
            seq,
            at,
            AuditInitiator::Authority(opening.authorizing.authority.clone()),
            AuditOperation::Operation(opening.authorizing.operation.clone()),
            AuditSubject::Workflow(SubjectRef::Attempt(opening.attempt.id.clone())),
            AuditKind::AttemptOpened,
        );
        Self::remember(
            &mut state,
            "append_attempt",
            &opening.authorizing.operation,
            request,
        );
        Ok(StateApplied::Applied)
    }

    fn record_decision(&self, record: &DecisionRecord) -> Result<StateApplied, StateError> {
        enum Effect {
            Accept(AttemptId),
            Reject(AttemptId),
            Cancel(Vec<AttemptId>),
            Revoke(AttemptId),
            Reclaim(AttemptId),
            Transfer(DecisionActor),
        }

        let now = self.clock.now();
        let mut state = self.lock()?;
        let request = Self::stored_identity(crate::stored::decision_identity(record))?;
        if Self::replay(&state, "record_decision", &record.operation, &request)? {
            return Ok(StateApplied::AlreadyApplied);
        }
        let assignment = state
            .assignments
            .get(record.assignment.as_str())
            .ok_or(StateError::UnknownRecord)?;
        Self::validate_decision_authority(assignment, &record.authority.actor)?;
        if let Some(target) = &record.resolves {
            let signal = state
                .signals
                .iter()
                .find(|signal| &signal.id == target)
                .ok_or(StateError::UnknownRecord)?;
            match &signal.body {
                SignalBody::Report { attempt, .. } => {
                    if state
                        .attempt_owners
                        .get(attempt.as_str())
                        .map(String::as_str)
                        != Some(record.assignment.as_str())
                    {
                        return Err(StateError::IncoherentBundle);
                    }
                }
                SignalBody::Request { recipient, .. } => {
                    if recipient != &record.authority.actor.actor {
                        return Err(StateError::ActorMismatch);
                    }
                }
                SignalBody::Directive { .. } => return Err(StateError::IncoherentBundle),
            }
        }

        let effect = match &record.kind {
            DecisionKind::Accept { handoff, .. } => {
                let handoff = state
                    .handoffs
                    .get(handoff.as_str())
                    .ok_or(StateError::UnknownRecord)?;
                if state
                    .attempt_owners
                    .get(handoff.attempt.as_str())
                    .map(String::as_str)
                    != Some(record.assignment.as_str())
                {
                    return Err(StateError::IncoherentBundle);
                }
                let attempt = Self::attempt_entry(&state, &record.assignment, &handoff.attempt)
                    .ok_or(StateError::IncoherentBundle)?;
                attempt_transition(attempt.state, AttemptAction::Accept, false)
                    .map_err(|_| StateError::IncoherentBundle)?;
                assignment_transition(assignment.state, AssignmentAction::Accept)
                    .map_err(|_| StateError::IncoherentBundle)?;
                Effect::Accept(handoff.attempt.clone())
            }
            DecisionKind::Reject { handoff, .. } => {
                let handoff = state
                    .handoffs
                    .get(handoff.as_str())
                    .ok_or(StateError::UnknownRecord)?;
                if state
                    .attempt_owners
                    .get(handoff.attempt.as_str())
                    .map(String::as_str)
                    != Some(record.assignment.as_str())
                {
                    return Err(StateError::IncoherentBundle);
                }
                let attempt = Self::attempt_entry(&state, &record.assignment, &handoff.attempt)
                    .ok_or(StateError::IncoherentBundle)?;
                attempt_transition(attempt.state, AttemptAction::Reject, false)
                    .map_err(|_| StateError::IncoherentBundle)?;
                Effect::Reject(handoff.attempt.clone())
            }
            DecisionKind::Cancel { .. } => {
                assignment_transition(assignment.state, AssignmentAction::Cancel)
                    .map_err(|_| StateError::IncoherentBundle)?;
                Effect::Cancel(
                    assignment
                        .attempts
                        .iter()
                        .filter(|attempt| !attempt.state.is_ended())
                        .map(|attempt| attempt.record.id.clone())
                        .collect(),
                )
            }
            DecisionKind::Revoke { attempt, .. } => {
                let entry = Self::attempt_entry(&state, &record.assignment, attempt)
                    .ok_or(StateError::IncoherentBundle)?;
                attempt_transition(entry.state, AttemptAction::Revoke, false)
                    .map_err(|_| StateError::IncoherentBundle)?;
                Effect::Revoke(attempt.clone())
            }
            DecisionKind::Reclaim { attempt, .. } => {
                let entry = Self::attempt_entry(&state, &record.assignment, attempt)
                    .ok_or(StateError::IncoherentBundle)?;
                let expired = now > entry.record.lease.expires_at;
                attempt_transition(entry.state, AttemptAction::Reclaim, expired)
                    .map_err(|_| StateError::IncoherentBundle)?;
                Effect::Reclaim(attempt.clone())
            }
            DecisionKind::TransferAuthority { to, .. } => {
                if assignment.state.is_terminal() || to.class != AuthorityClass::Orchestrator {
                    return Err(StateError::IncoherentBundle);
                }
                Effect::Transfer(to.clone())
            }
        };

        let audit_subject = match &effect {
            Effect::Accept(attempt)
            | Effect::Reject(attempt)
            | Effect::Revoke(attempt)
            | Effect::Reclaim(attempt) => {
                AuditSubject::Workflow(SubjectRef::Attempt(attempt.clone()))
            }
            Effect::Cancel(_) | Effect::Transfer(_) => {
                AuditSubject::Workflow(SubjectRef::Assignment(record.assignment.clone()))
            }
        };

        let seq = Self::next_seq(&mut state);
        {
            let assignment = state
                .assignments
                .get_mut(record.assignment.as_str())
                .expect("assignment validated above");
            match &effect {
                Effect::Accept(attempt) => {
                    assignment.state = AssignmentState::Accepted;
                    assignment
                        .attempts
                        .iter_mut()
                        .find(|entry| &entry.record.id == attempt)
                        .expect("attempt validated above")
                        .state = AttemptState::Accepted;
                }
                Effect::Reject(attempt) => {
                    assignment
                        .attempts
                        .iter_mut()
                        .find(|entry| &entry.record.id == attempt)
                        .expect("attempt validated above")
                        .state = AttemptState::Rejected;
                }
                Effect::Cancel(attempts) => {
                    assignment.state = AssignmentState::Cancelled;
                    for attempt in attempts {
                        assignment
                            .attempts
                            .iter_mut()
                            .find(|entry| &entry.record.id == attempt)
                            .expect("attempt validated above")
                            .state = AttemptState::Revoked;
                    }
                }
                Effect::Revoke(attempt) => {
                    assignment
                        .attempts
                        .iter_mut()
                        .find(|entry| &entry.record.id == attempt)
                        .expect("attempt validated above")
                        .state = AttemptState::Revoked;
                }
                Effect::Reclaim(attempt) => {
                    assignment
                        .attempts
                        .iter_mut()
                        .find(|entry| &entry.record.id == attempt)
                        .expect("attempt validated above")
                        .state = AttemptState::Expired;
                }
                Effect::Transfer(to) => assignment.record.decision_actor = to.clone(),
            }
        }
        let ended: Vec<AttemptId> = match &effect {
            Effect::Accept(attempt)
            | Effect::Reject(attempt)
            | Effect::Revoke(attempt)
            | Effect::Reclaim(attempt) => vec![attempt.clone()],
            Effect::Cancel(attempts) => attempts.clone(),
            Effect::Transfer(_) => Vec::new(),
        };
        for attempt in &ended {
            Self::revoke_attempt_credential(&mut state, attempt);
        }
        state.response_actions.push(ResponseAction {
            seq,
            kind: ResponseKind::FencedDecision {
                responds_to: record.resolves.clone(),
            },
        });
        for attempt in &ended {
            state.response_actions.push(ResponseAction {
                seq,
                kind: ResponseKind::TerminalAttemptAction {
                    attempt: attempt.clone(),
                    abort_consistent: true,
                },
            });
        }
        if let Some(reason) = record.kind.close_reason() {
            let bead = state
                .assignments
                .get(record.assignment.as_str())
                .expect("assignment validated above")
                .record
                .bead
                .clone();
            state.projections.insert(
                record.operation.as_str().to_owned(),
                PendingApplication {
                    operation: record.operation.clone(),
                    assignment: record.assignment.clone(),
                    bead,
                    projection: WorkProjection::Close { reason },
                    committed_at: seq,
                    authorized_revision: None,
                    receipt_candidate: None,
                },
            );
        }
        state
            .decisions
            .insert(record.operation.as_str().to_owned(), record.clone());
        Self::append_audit(
            &mut state,
            seq,
            now,
            AuditInitiator::Authority(record.authority.clone()),
            AuditOperation::Operation(record.operation.clone()),
            audit_subject,
            AuditKind::decision(&record.kind),
        );
        Self::remember(&mut state, "record_decision", &record.operation, request);
        Ok(StateApplied::Applied)
    }

    fn activate_profile(&self, opening: &ActivationOpening) -> Result<StateApplied, StateError> {
        let activation = &opening.activation;
        let at = self.clock.now();
        let mut state = self.lock()?;
        let request = Self::stored_identity(crate::stored::activation_identity(opening))?;
        if Self::replay(&state, "activate_profile", &activation.operation, &request)? {
            return Ok(StateApplied::AlreadyApplied);
        }
        match &opening.case {
            ActivationCase::OperatorBootstrap => {
                if activation.class() != AuthorityClass::Orchestrator {
                    return Err(StateError::ActivationCaseInvalid);
                }
                if state.bootstrap_complete {
                    return Err(StateError::BootstrapAlreadyComplete);
                }
            }
            ActivationCase::ActorAuthorizedRotation { .. } => {
                match state.actor_classes.get(activation.actor.as_str()) {
                    None => return Err(StateError::UnknownActor),
                    Some(class) if *class != activation.class() => {
                        return Err(StateError::ActorClassMismatch);
                    }
                    Some(_) => {}
                }
            }
            ActivationCase::OperatorRecovery => {
                if activation.class() != AuthorityClass::Orchestrator {
                    return Err(StateError::ActivationCaseInvalid);
                }
                match state.actor_classes.get(activation.actor.as_str()) {
                    None => return Err(StateError::UnknownActor),
                    Some(class) if *class != activation.class() => {
                        return Err(StateError::ActorClassMismatch);
                    }
                    Some(_) => {}
                }
            }
            ActivationCase::OperatorOrchestratorEnrolment => {
                if activation.class() != AuthorityClass::Orchestrator {
                    return Err(StateError::ActivationCaseInvalid);
                }
                if state.actor_classes.contains_key(activation.actor.as_str()) {
                    return Err(StateError::ActivationCaseInvalid);
                }
            }
        }
        if let Some(class) = state.actor_classes.get(activation.actor.as_str())
            && *class != activation.class()
        {
            return Err(StateError::ActorClassMismatch);
        }
        if activation.occupancy() == OccupancyClass::Singleton
            && let Some(members) = state.active_members.get(activation.profile.as_str())
            && !members.is_empty()
            && !members.contains(activation.actor.as_str())
        {
            return Err(StateError::ProfileOccupied);
        }
        let owner = Self::activation_locator(
            &activation.actor,
            &activation.profile,
            &activation.operation,
        );
        Self::credential_available(&state, &opening.credential.id, &owner)?;

        let seq = Self::next_seq(&mut state);
        state
            .actor_classes
            .insert(activation.actor.as_str().to_owned(), activation.class());
        for binding in state.credentials.values_mut() {
            if binding.assignment.is_none()
                && binding.actor == activation.actor
                && binding.profile == activation.profile
            {
                binding.revoked = true;
            }
        }
        Self::insert_credential(
            &mut state,
            owner,
            CredentialBinding {
                credential: opening.credential.id.clone(),
                digest: opening.credential.digest.clone(),
                actor: activation.actor.clone(),
                profile: activation.profile.clone(),
                assignment: None,
                revoked: false,
            },
        );
        state
            .active_members
            .entry(activation.profile.as_str().to_owned())
            .or_default()
            .insert(activation.actor.as_str().to_owned());
        if matches!(opening.case, ActivationCase::OperatorBootstrap) {
            state.bootstrap_complete = true;
        }
        let initiator = match &opening.case {
            ActivationCase::ActorAuthorizedRotation { authority } => {
                AuditInitiator::Authority(authority.clone())
            }
            ActivationCase::OperatorBootstrap
            | ActivationCase::OperatorRecovery
            | ActivationCase::OperatorOrchestratorEnrolment => AuditInitiator::OperatorChannel,
        };
        Self::append_audit(
            &mut state,
            seq,
            at,
            initiator,
            AuditOperation::Operation(activation.operation.clone()),
            AuditSubject::ActorProfile {
                actor: activation.actor.clone(),
                profile: activation.profile.clone(),
            },
            AuditKind::activation(&opening.case),
        );
        Self::remember(
            &mut state,
            "activate_profile",
            &activation.operation,
            request,
        );
        Ok(StateApplied::Applied)
    }

    fn deactivate_profile(
        &self,
        operation: &OperationId,
        actor: &ActorId,
        profile: &ProfileName,
    ) -> Result<StateApplied, StateError> {
        let at = self.clock.now();
        let mut state = self.lock()?;
        let request = format!("{}|{}", actor.as_str(), profile.as_str());
        if Self::replay(&state, "deactivate_profile", operation, &request)? {
            return Ok(StateApplied::AlreadyApplied);
        }
        let is_member = state
            .active_members
            .get(profile.as_str())
            .is_some_and(|members| members.contains(actor.as_str()));
        if !is_member {
            return Err(StateError::NotTheOccupant);
        }
        let seq = Self::next_seq(&mut state);
        state
            .active_members
            .get_mut(profile.as_str())
            .expect("membership validated above")
            .remove(actor.as_str());
        for binding in state.credentials.values_mut() {
            if binding.actor == *actor && binding.profile == *profile {
                binding.revoked = true;
            }
        }
        Self::append_audit(
            &mut state,
            seq,
            at,
            AuditInitiator::OperatorChannel,
            AuditOperation::Operation(operation.clone()),
            AuditSubject::ActorProfile {
                actor: actor.clone(),
                profile: profile.clone(),
            },
            AuditKind::ProfileDeactivated,
        );
        Self::remember(&mut state, "deactivate_profile", operation, request);
        Ok(StateApplied::Applied)
    }

    fn append_signal(&self, draft: &SignalDraft) -> Result<(Signal, StateApplied), StateError> {
        // Reports are fenced worker mutations. Even an existing Report
        // cannot be replayed through the orchestrator append path, or the
        // public seam would acquire a second, unfenced carriage for it.
        if matches!(draft.body, SignalBody::Report { .. }) {
            return Err(StateError::IncoherentBundle);
        }
        let at = self.clock.now();
        let mut state = self.lock()?;
        if let Some(signal) = Self::signal_replay(&state, draft)? {
            return Ok((signal, StateApplied::AlreadyApplied));
        }
        validate_subject(&draft.body, &draft.subject).map_err(|_| StateError::IncoherentBundle)?;
        match &draft.body {
            SignalBody::Report { .. } => unreachable!("reports were rejected before replay"),
            SignalBody::Directive {
                assignment,
                attempt,
                kind,
            } => {
                let owner = state
                    .assignments
                    .get(assignment.as_str())
                    .ok_or(StateError::UnknownRecord)?;
                if owner
                    .attempts
                    .iter()
                    .find(|entry| &entry.record.id == attempt)
                    .is_none_or(|entry| entry.state != AttemptState::Active)
                {
                    return Err(StateError::IncoherentBundle);
                }
                if owner.record.decision_actor != draft.sender.actor {
                    return Err(StateError::ActorMismatch);
                }
                if let DirectiveKind::Answer { report, .. } = kind {
                    let answered = state
                        .signals
                        .iter()
                        .find(|signal| &signal.id == report)
                        .ok_or(StateError::UnknownRecord)?;
                    if !matches!(
                        &answered.body,
                        SignalBody::Report {
                            attempt: answered_attempt,
                            ..
                        } if answered_attempt == attempt
                    ) {
                        return Err(StateError::IncoherentBundle);
                    }
                }
            }
            SignalBody::Request { .. } => {}
        }
        let signal = Self::commit_new_signal(&mut state, draft)?;
        Self::append_audit(
            &mut state,
            signal.seq,
            at,
            AuditInitiator::Authority(draft.sender.clone()),
            AuditOperation::Signal(signal.id.clone()),
            AuditSubject::Workflow(draft.subject.clone()),
            AuditKind::signal(&signal),
        );
        Ok((signal, StateApplied::Applied))
    }

    fn fenced_report(
        &self,
        action: &FencedAction,
        draft: &SignalDraft,
    ) -> Result<(ReportOutcome, FencedResponse), StateError> {
        let call = &action.call;
        let now = self.clock.now();
        let mut state = self.lock()?;
        let request = Self::stored_identity(crate::stored::report_identity(
            &Self::action_identity(action),
            draft,
        ))?;
        if Self::replay(&state, "fenced_report", &call.operation, &request)? {
            let outcome = state
                .report_outcomes
                .get(call.operation.as_str())
                .cloned()
                .ok_or(StateError::Corrupt)?;
            return Ok((
                outcome,
                Self::response(&state, &call.attempt, StateApplied::AlreadyApplied),
            ));
        }
        Self::validate_active_attempt(&state, call, now)?;
        Self::validate_response_target(&state, action)?;
        if state.signals.iter().any(|signal| signal.id == draft.id) {
            return Err(StateError::ConflictingOperation);
        }
        let valid_report_shape = matches!(
            (&draft.subject, &draft.body),
            (SubjectRef::Attempt(subject), SignalBody::Report { attempt, .. })
                if subject == &call.attempt && attempt == &call.attempt
        );
        if !valid_report_shape {
            return Err(StateError::IncoherentBundle);
        }
        let assignment = state
            .assignments
            .get(call.assignment.as_str())
            .expect("fence validated the assignment above");
        if assignment.record.worker != draft.sender.actor {
            return Err(StateError::ActorMismatch);
        }
        let initiator = Self::worker_initiator(&state, call)?;
        let binding = binding_directives(&call.attempt, &state.signals, &state.response_actions);
        let (outcome, seq) = match worker_append_gate(&binding) {
            Ok(()) => {
                let signal = Self::commit_new_signal(&mut state, draft)?;
                let seq = Self::commit_fenced_call(&mut state, Some(action), true);
                (
                    ReportOutcome::Recorded {
                        signal: Box::new(signal),
                    },
                    seq,
                )
            }
            Err(reason) => {
                let seq = Self::commit_fenced_call(&mut state, Some(action), false);
                (ReportOutcome::Refused { reason }, seq)
            }
        };
        state
            .report_outcomes
            .insert(call.operation.as_str().to_owned(), outcome.clone());
        Self::append_audit(
            &mut state,
            seq,
            now,
            initiator,
            AuditOperation::Operation(call.operation.clone()),
            AuditSubject::Workflow(SubjectRef::Attempt(call.attempt.clone())),
            AuditKind::report(&outcome),
        );
        Self::remember(&mut state, "fenced_report", &call.operation, request);
        Ok((
            outcome,
            Self::response(&state, &call.attempt, StateApplied::Applied),
        ))
    }

    fn fenced_evidence(
        &self,
        action: &FencedAction,
        evidence: &abacus_core::Evidence,
    ) -> Result<(EvidenceOutcome, FencedResponse), StateError> {
        let call = &action.call;
        let now = self.clock.now();
        let mut state = self.lock()?;
        let request = Self::stored_identity(crate::stored::evidence_identity(
            &Self::action_identity(action),
            evidence,
        ))?;
        if Self::replay(&state, "fenced_evidence", &call.operation, &request)? {
            let outcome = *state
                .evidence_outcomes
                .get(call.operation.as_str())
                .ok_or(StateError::Corrupt)?;
            return Ok((
                outcome,
                Self::response(&state, &call.attempt, StateApplied::AlreadyApplied),
            ));
        }
        Self::validate_active_attempt(&state, call, now)?;
        Self::validate_response_target(&state, action)?;
        let initiator = Self::worker_initiator(&state, call)?;
        let binding = binding_directives(&call.attempt, &state.signals, &state.response_actions);
        let (outcome, seq) = match worker_append_gate(&binding) {
            Ok(()) => {
                state.evidence.push(EvidenceRecord {
                    operation: call.operation.clone(),
                    attempt: call.attempt.clone(),
                    evidence: evidence.clone(),
                });
                let seq = Self::commit_fenced_call(&mut state, Some(action), true);
                (EvidenceOutcome::Recorded, seq)
            }
            Err(reason) => {
                let seq = Self::commit_fenced_call(&mut state, Some(action), false);
                (EvidenceOutcome::Refused { reason }, seq)
            }
        };
        state
            .evidence_outcomes
            .insert(call.operation.as_str().to_owned(), outcome);
        Self::append_audit(
            &mut state,
            seq,
            now,
            initiator,
            AuditOperation::Operation(call.operation.clone()),
            AuditSubject::Workflow(SubjectRef::Attempt(call.attempt.clone())),
            AuditKind::evidence(outcome),
        );
        Self::remember(&mut state, "fenced_evidence", &call.operation, request);
        Ok((
            outcome,
            Self::response(&state, &call.attempt, StateApplied::Applied),
        ))
    }

    fn fenced_submit_handoff(
        &self,
        action: &FencedAction,
        handoff: &HandoffRecord,
    ) -> Result<(SubmissionOutcome, FencedResponse), StateError> {
        let call = &action.call;
        let now = self.clock.now();
        let mut state = self.lock()?;
        let request = Self::stored_identity(crate::stored::handoff_identity(
            &Self::action_identity(action),
            handoff,
        ))?;
        if Self::replay(&state, "fenced_handoff", &call.operation, &request)? {
            let (_, outcome) = state
                .submissions
                .get(call.operation.as_str())
                .cloned()
                .ok_or(StateError::Corrupt)?;
            return Ok((
                outcome,
                Self::response(&state, &call.attempt, StateApplied::AlreadyApplied),
            ));
        }
        Self::validate_active_attempt(&state, call, now)?;
        Self::validate_response_target(&state, action)?;
        let initiator = Self::worker_initiator(&state, call)?;
        if handoff.attempt != call.attempt {
            return Err(StateError::IncoherentBundle);
        }
        if state.handoffs.contains_key(handoff.id.as_str()) {
            return Err(StateError::ConflictingOperation);
        }

        let has_all_evidence = !handoff.evidence_operations.is_empty()
            && handoff.evidence_operations.iter().all(|operation| {
                state
                    .evidence
                    .iter()
                    .any(|record| &record.operation == operation && record.attempt == call.attempt)
            });
        let outcome = if !has_all_evidence {
            SubmissionOutcome::Refused {
                reason: SubmissionRefusalReason::MissingEvidence,
            }
        } else {
            let candidate = Self::worker_action(action, Seq(state.head + 1));
            let mut actions = state.response_actions.clone();
            actions.push(candidate);
            let binding = binding_directives(&call.attempt, &state.signals, &actions);
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
            state
                .handoffs
                .insert(handoff.id.as_str().to_owned(), handoff.clone());
            Self::attempt_entry_mut(&mut state, &call.assignment, &call.attempt)
                .expect("fence validated above")
                .state = AttemptState::Submitted;
        }
        let seq = Self::commit_fenced_call(&mut state, Some(action), substantive);
        state.submissions.insert(
            call.operation.as_str().to_owned(),
            (request.clone(), outcome.clone()),
        );
        Self::append_audit(
            &mut state,
            seq,
            now,
            initiator,
            AuditOperation::Operation(call.operation.clone()),
            AuditSubject::Workflow(SubjectRef::Attempt(call.attempt.clone())),
            AuditKind::handoff(&outcome),
        );
        Self::remember(&mut state, "fenced_handoff", &call.operation, request);
        Ok((
            outcome,
            Self::response(&state, &call.attempt, StateApplied::Applied),
        ))
    }

    fn fenced_abort_attempt(&self, call: &FencedCall) -> Result<FencedResponse, StateError> {
        let now = self.clock.now();
        let mut state = self.lock()?;
        let request = Self::call_identity(call);
        if Self::replay(&state, "fenced_abort_attempt", &call.operation, &request)? {
            return Ok(Self::response(
                &state,
                &call.attempt,
                StateApplied::AlreadyApplied,
            ));
        }

        Self::validate_active_attempt(&state, call, now)?;
        let initiator = Self::worker_initiator(&state, call)?;
        let binding = binding_directives(&call.attempt, &state.signals, &state.response_actions);
        if worker_append_gate(&binding) != Err(abacus_core::DirectiveGateRefusal::AbortInForce) {
            return Err(StateError::AbortNotInForce);
        }

        let next = attempt_transition(AttemptState::Active, AttemptAction::Abort, false)
            .map_err(|_| StateError::IncoherentBundle)?;
        let seq = Self::next_seq(&mut state);
        Self::attempt_entry_mut(&mut state, &call.assignment, &call.attempt)
            .expect("active Attempt validated above")
            .state = next;
        Self::revoke_attempt_credential(&mut state, &call.attempt);
        state.response_actions.push(ResponseAction {
            seq,
            kind: ResponseKind::TerminalAttemptAction {
                attempt: call.attempt.clone(),
                abort_consistent: true,
            },
        });
        Self::append_audit(
            &mut state,
            seq,
            now,
            initiator,
            AuditOperation::Operation(call.operation.clone()),
            AuditSubject::Workflow(SubjectRef::Attempt(call.attempt.clone())),
            AuditKind::AttemptAborted,
        );
        Self::remember(&mut state, "fenced_abort_attempt", &call.operation, request);
        Ok(Self::response(&state, &call.attempt, StateApplied::Applied))
    }

    fn renew_lease(
        &self,
        call: &FencedCall,
        until: Timestamp,
    ) -> Result<(Lease, FencedResponse), StateError> {
        let now = self.clock.now();
        let mut state = self.lock()?;
        let request = Self::stored_identity(crate::stored::renewal_identity(
            &Self::call_identity(call),
            until,
        ))?;
        if Self::replay(&state, "renew_lease", &call.operation, &request)? {
            return Ok((
                Lease {
                    token: call.token,
                    expires_at: until,
                },
                Self::response(&state, &call.attempt, StateApplied::AlreadyApplied),
            ));
        }
        Self::validate_fence(&state, call, now)?;
        let initiator = Self::worker_initiator(&state, call)?;
        let current = Self::attempt_entry(&state, &call.assignment, &call.attempt)
            .expect("fence validated above")
            .record
            .lease
            .expires_at;
        if until <= current {
            return Err(StateError::NonExtendingLease);
        }
        Self::attempt_entry_mut(&mut state, &call.assignment, &call.attempt)
            .expect("fence validated above")
            .record
            .lease
            .expires_at = until;
        let seq = Self::commit_fenced_call(&mut state, None, false);
        Self::append_audit(
            &mut state,
            seq,
            now,
            initiator,
            AuditOperation::Operation(call.operation.clone()),
            AuditSubject::Workflow(SubjectRef::Attempt(call.attempt.clone())),
            AuditKind::LeaseRenewed,
        );
        Self::remember(&mut state, "renew_lease", &call.operation, request);
        Ok((
            Lease {
                token: call.token,
                expires_at: until,
            },
            Self::response(&state, &call.attempt, StateApplied::Applied),
        ))
    }

    fn persist_envelope(
        &self,
        operation: &OperationId,
        subject: &LaunchSubject,
        envelope: &EnvelopeSnapshot,
    ) -> Result<StateApplied, StateError> {
        let at = self.clock.now();
        let mut state = self.lock()?;
        Self::resolve_subject(&state, subject)?;
        let authorizing = Self::subject_authorizing_operation(&state, subject)?;
        let initiator = Self::system_projection(&state, &authorizing)?;
        let key = Self::association_key(subject);
        let request = Self::stored_identity(crate::stored::envelope_identity(&key, envelope))?;
        if Self::replay(&state, "persist_envelope", operation, &request)? {
            return Ok(StateApplied::AlreadyApplied);
        }
        if let Some(existing) = state.envelopes.get(&key)
            && existing != envelope
        {
            return Err(StateError::ConflictingOperation);
        }
        let seq = Self::next_seq(&mut state);
        state.envelopes.insert(key, envelope.clone());
        Self::append_audit(
            &mut state,
            seq,
            at,
            initiator,
            AuditOperation::Operation(operation.clone()),
            AuditSubject::Launch(subject.clone()),
            AuditKind::EnvelopePersisted,
        );
        Self::remember(&mut state, "persist_envelope", operation, request);
        Ok(StateApplied::Applied)
    }

    fn envelope(&self, subject: &LaunchSubject) -> Result<EnvelopeSnapshot, StateError> {
        let state = self.lock()?;
        Self::resolve_subject(&state, subject)?;
        state
            .envelopes
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
        let at = self.clock.now();
        let mut state = self.lock()?;
        Self::resolve_subject(&state, subject)?;
        let authorizing = Self::subject_authorizing_operation(&state, subject)?;
        let initiator = Self::system_projection(&state, &authorizing)?;
        let key = Self::association_key(subject);
        let request = format!("{key}|{}", handle.as_str());
        if Self::replay(&state, "bind_runtime_handle", operation, &request)? {
            return Ok(StateApplied::AlreadyApplied);
        }
        if let Some(existing) = state.handles.get(&key)
            && existing != handle
        {
            return Err(StateError::ConflictingOperation);
        }
        let seq = Self::next_seq(&mut state);
        state.handles.insert(key, handle.clone());
        Self::append_audit(
            &mut state,
            seq,
            at,
            initiator,
            AuditOperation::Operation(operation.clone()),
            AuditSubject::Launch(subject.clone()),
            AuditKind::RuntimeHandleBound,
        );
        Self::remember(&mut state, "bind_runtime_handle", operation, request);
        Ok(StateApplied::Applied)
    }

    fn unbind_runtime_handle(
        &self,
        operation: &OperationId,
        subject: &LaunchSubject,
    ) -> Result<StateApplied, StateError> {
        let at = self.clock.now();
        let mut state = self.lock()?;
        Self::resolve_subject(&state, subject)?;
        let authorizing = Self::subject_authorizing_operation(&state, subject)?;
        let initiator = Self::system_projection(&state, &authorizing)?;
        let key = Self::association_key(subject);
        if Self::replay(&state, "unbind_runtime_handle", operation, &key)? {
            return Ok(StateApplied::AlreadyApplied);
        }
        let seq = Self::next_seq(&mut state);
        state.handles.remove(&key);
        Self::append_audit(
            &mut state,
            seq,
            at,
            initiator,
            AuditOperation::Operation(operation.clone()),
            AuditSubject::Launch(subject.clone()),
            AuditKind::RuntimeHandleUnbound,
        );
        Self::remember(&mut state, "unbind_runtime_handle", operation, key);
        Ok(StateApplied::Applied)
    }

    fn runtime_handle(&self, subject: &LaunchSubject) -> Result<Option<RuntimeHandle>, StateError> {
        let state = self.lock()?;
        Self::resolve_subject(&state, subject)?;
        Ok(state.handles.get(&Self::association_key(subject)).cloned())
    }

    fn record_runtime_observation(
        &self,
        operation: &OperationId,
        record: &RuntimeObservationRecord,
    ) -> Result<StateApplied, StateError> {
        let at = self.clock.now();
        let mut state = self.lock()?;
        Self::resolve_subject(&state, &record.subject)?;
        let request = Self::stored_identity(crate::stored::runtime_observation_identity(record))?;
        if Self::replay(&state, "record_runtime_observation", operation, &request)? {
            return Ok(StateApplied::AlreadyApplied);
        }
        let seq = Self::next_seq(&mut state);
        state
            .runtime_observations
            .insert(operation.as_str().to_owned(), record.clone());
        Self::append_audit(
            &mut state,
            seq,
            at,
            AuditInitiator::Authority(record.reporter.clone()),
            AuditOperation::Operation(operation.clone()),
            AuditSubject::Launch(record.subject.clone()),
            AuditKind::RuntimeObservationRecorded,
        );
        Self::remember(&mut state, "record_runtime_observation", operation, request);
        Ok(StateApplied::Applied)
    }

    fn runtime_observation(
        &self,
        operation: &OperationId,
    ) -> Result<RuntimeObservationRecord, StateError> {
        self.lock()?
            .runtime_observations
            .get(operation.as_str())
            .cloned()
            .ok_or(StateError::UnknownRecord)
    }

    fn record_application_attempt(
        &self,
        attempt: &ApplicationAttempt,
    ) -> Result<StateApplied, StateError> {
        let at = self.clock.now();
        let mut state = self.lock()?;
        let request = Self::stored_identity(crate::stored::application_attempt_identity(attempt))?;
        if Self::replay(&state, "record_application_attempt", &attempt.id, &request)? {
            return Ok(StateApplied::AlreadyApplied);
        }
        if !state.projections.contains_key(attempt.target.as_str()) {
            return Err(StateError::UnknownRecord);
        }
        let initiator = Self::system_projection(&state, &attempt.target)?;
        let seq = Self::next_seq(&mut state);
        state
            .application_attempts
            .entry(attempt.target.as_str().to_owned())
            .or_default()
            .push(attempt.clone());
        Self::append_audit(
            &mut state,
            seq,
            at,
            initiator,
            AuditOperation::Operation(attempt.id.clone()),
            AuditSubject::Projection(attempt.target.clone()),
            AuditKind::application(&attempt.outcome),
        );
        Self::remember(
            &mut state,
            "record_application_attempt",
            &attempt.id,
            request,
        );
        Ok(StateApplied::Applied)
    }

    fn record_application_receipt(
        &self,
        receipt: &ApplicationReceipt,
    ) -> Result<StateApplied, StateError> {
        let at = self.clock.now();
        let mut state = self.lock()?;
        let request = Self::stored_identity(crate::stored::application_receipt_identity(receipt))?;
        if Self::replay(
            &state,
            "record_application_receipt",
            &receipt.target,
            &request,
        )? {
            return Ok(StateApplied::AlreadyApplied);
        }
        let projection = state
            .projections
            .get(receipt.target.as_str())
            .ok_or(StateError::UnknownRecord)?;
        // Replay wins above: a receipt committed before a later Close
        // remains an immutable historical fact. A NEW receipt must not
        // cross the opposite Ledger ordering, though. Re-derive causal
        // supersession under this same lock so a candidate read just
        // before the Close cannot mint a receipt just after it.
        if Self::superseding_projection(&state, projection).is_some() {
            return Err(StateError::IncoherentBundle);
        }
        let attempt = state
            .application_attempts
            .get(receipt.target.as_str())
            .and_then(|attempts| {
                attempts
                    .iter()
                    .find(|attempt| attempt.id == receipt.attempt)
            })
            .ok_or(StateError::IncoherentBundle)?;
        let revision_matches = match &attempt.outcome {
            ApplicationOutcome::Applied { after, .. } => after == &receipt.after,
            ApplicationOutcome::FoundPresent { .. }
            | ApplicationOutcome::ObservedAfterAmbiguous { .. }
            | ApplicationOutcome::Failed { .. }
            | ApplicationOutcome::Ambiguous => false,
        };
        if !revision_matches {
            return Err(StateError::IncoherentBundle);
        }
        let initiator = Self::system_projection(&state, &receipt.target)?;
        let seq = Self::next_seq(&mut state);
        state
            .receipts
            .insert(receipt.target.as_str().to_owned(), receipt.clone());
        Self::append_audit(
            &mut state,
            seq,
            at,
            initiator,
            AuditOperation::Operation(receipt.target.clone()),
            AuditSubject::Projection(receipt.target.clone()),
            AuditKind::ApplicationReceiptRecorded,
        );
        Self::remember(
            &mut state,
            "record_application_receipt",
            &receipt.target,
            request,
        );
        Ok(StateApplied::Applied)
    }

    fn assignment(&self, id: &AssignmentId) -> Result<AssignmentView, StateError> {
        let state = self.lock()?;
        let entry = state
            .assignments
            .get(id.as_str())
            .ok_or(StateError::UnknownRecord)?;
        Ok(AssignmentView {
            record: entry.record.clone(),
            state: entry.state,
            attempts: entry
                .attempts
                .iter()
                .map(|attempt| (attempt.record.id.clone(), attempt.state))
                .collect(),
            head: Self::current_head(&state),
        })
    }

    fn evidence_for(&self, attempt: &AttemptId) -> Result<Vec<EvidenceRecord>, StateError> {
        let state = self.lock()?;
        if !state.attempt_owners.contains_key(attempt.as_str()) {
            return Err(StateError::UnknownRecord);
        }
        Ok(state
            .evidence
            .iter()
            .filter(|record| &record.attempt == attempt)
            .cloned()
            .collect())
    }

    fn signals_for(&self, attempt: &AttemptId) -> Result<Vec<Signal>, StateError> {
        let state = self.lock()?;
        if !state.attempt_owners.contains_key(attempt.as_str()) {
            return Err(StateError::UnknownRecord);
        }
        Ok(state
            .signals
            .iter()
            .filter(|signal| {
                matches!(&signal.subject, SubjectRef::Attempt(subject) if subject == attempt)
            })
            .cloned()
            .collect())
    }

    fn verify_launch_subject(
        &self,
        subject: &LaunchSubject,
        presented_digest: &ContentHash,
    ) -> Result<(), StateError> {
        let state = self.lock()?;
        let binding = Self::resolve_subject(&state, subject)?;
        if binding.revoked {
            Err(StateError::CredentialRevoked)
        } else if !bool::from(
            binding
                .digest
                .as_str()
                .as_bytes()
                .ct_eq(presented_digest.as_str().as_bytes()),
        ) {
            Err(StateError::CredentialInvalid)
        } else {
            Ok(())
        }
    }

    fn handoff(&self, id: &HandoffId) -> Result<HandoffRecord, StateError> {
        self.lock()?
            .handoffs
            .get(id.as_str())
            .cloned()
            .ok_or(StateError::UnknownRecord)
    }

    fn decision(&self, operation: &OperationId) -> Result<DecisionRecord, StateError> {
        self.lock()?
            .decisions
            .get(operation.as_str())
            .cloned()
            .ok_or(StateError::UnknownRecord)
    }

    fn active_occupants(&self, profile: &ProfileName) -> Result<Vec<ActorId>, StateError> {
        Ok(self
            .lock()?
            .active_members
            .get(profile.as_str())
            .map(|members| {
                members
                    .iter()
                    .map(|actor| ActorId::new(actor).expect("stored ActorId was validated"))
                    .collect()
            })
            .unwrap_or_default())
    }

    fn pending_applications(&self) -> Result<Vec<PendingApplication>, StateError> {
        let state = self.lock()?;
        let mut pending: Vec<PendingApplication> = state
            .projections
            .values()
            .filter(|projection| !state.receipts.contains_key(projection.operation.as_str()))
            .filter(|projection| Self::superseding_projection(&state, projection).is_none())
            .map(|projection| Self::pending_view(&state, projection))
            .collect();
        pending.sort_by_key(|projection| projection.committed_at);
        Ok(pending)
    }

    fn superseded_applications(&self) -> Result<Vec<SupersededApplication>, StateError> {
        let state = self.lock()?;
        let mut superseded: Vec<SupersededApplication> = state
            .projections
            .values()
            // A successful receipt already resolved the projection; this
            // view names the receiptless projections removed from the
            // actionable set specifically by causal supersession.
            .filter(|projection| !state.receipts.contains_key(projection.operation.as_str()))
            .filter_map(|projection| {
                let superseding = Self::superseding_projection(&state, projection)?;
                let mut application = projection.clone();
                application.receipt_candidate = None;
                Some(SupersededApplication {
                    application,
                    superseded_by: superseding.operation.clone(),
                })
            })
            .collect();
        superseded.sort_by_key(|item| item.application.committed_at);
        Ok(superseded)
    }

    fn unresolved_signals(&self, recipient: Option<&ActorId>) -> Result<Vec<Signal>, StateError> {
        let state = self.lock()?;
        let unresolved = unresolved(&state.signals, &state.response_actions, None);
        Ok(unresolved
            .into_iter()
            .filter(|signal| match (&signal.body, recipient) {
                (_, None) => true,
                (
                    SignalBody::Request {
                        recipient: stored, ..
                    },
                    Some(requested),
                ) => stored == requested,
                (SignalBody::Report { attempt, .. }, Some(requested)) => state
                    .attempt_owners
                    .get(attempt.as_str())
                    .and_then(|assignment| state.assignments.get(assignment))
                    .is_some_and(|assignment| &assignment.record.decision_actor.actor == requested),
                (SignalBody::Directive { .. }, Some(_)) => false,
            })
            .cloned()
            .collect())
    }

    fn audit_events(&self, query: &AuditQuery) -> Result<Vec<AuditEvent>, StateError> {
        let state = self.lock()?;
        Ok(state
            .audit_events
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
