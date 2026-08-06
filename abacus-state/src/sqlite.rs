//! SQLite-backed [`WorkflowStatePort`] implementation.
//!
//! One outer mutex maps an in-process port call onto one `BEGIN IMMEDIATE`
//! SQLite transaction. The relational rows are authoritative. Rebuilding the
//! canonical [`InMemoryState`] engine on open is only a v1 policy-sharing
//! cache: no aggregate snapshot is persisted and no command journal is
//! replayed. Immutable facts have INSERT-only storage paths; updates are
//! restricted to genuinely current state.

use std::path::Path;
use std::sync::{Mutex, MutexGuard};

use abacus_core::ports::*;
use abacus_core::*;
use rusqlite::{Connection, OpenFlags, TransactionBehavior};

use crate::memory::InMemoryState;
use crate::migrations::{MigrationError, apply_migrations};
use crate::stored::{StoredError, load_state, persist_delta};

#[derive(Debug, thiserror::Error)]
pub enum SqliteStateOpenError {
    #[error(transparent)]
    Migration(#[from] MigrationError),
    #[error("sqlite open/configuration failed: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("stored Ledger representation is invalid: {0}")]
    Stored(String),
}

impl From<StoredError> for SqliteStateOpenError {
    fn from(value: StoredError) -> Self {
        Self::Stored(value.to_string())
    }
}

struct SqliteInner<C> {
    connection: Connection,
    cache: InMemoryState<C>,
}

/// Durable SQLite implementation of the core workflow-state seam.
pub struct SqliteState<C> {
    inner: Mutex<SqliteInner<C>>,
}

impl<C: ClockPort> SqliteState<C> {
    /// Open (or create and migrate) one database and rebuild its behavioral
    /// cache from checked relational rows.
    pub fn open(path: impl AsRef<Path>, clock: C) -> Result<Self, SqliteStateOpenError> {
        let path = path.as_ref();
        apply_migrations(path)?;
        let connection = Connection::open_with_flags(
            path,
            OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_URI,
        )?;
        connection.busy_timeout(std::time::Duration::from_secs(5))?;
        connection.pragma_update(None, "foreign_keys", true)?;
        connection.pragma_update(None, "journal_mode", "WAL")?;
        let state = load_state(&connection)?;
        Ok(Self {
            inner: Mutex::new(SqliteInner {
                connection,
                cache: InMemoryState::from_state(clock, state),
            }),
        })
    }

    fn lock(&self) -> Result<MutexGuard<'_, SqliteInner<C>>, StateError> {
        self.inner.lock().map_err(|_| StateError::Corrupt)
    }

    fn read<T>(
        &self,
        operation: impl FnOnce(&InMemoryState<C>) -> Result<T, StateError>,
    ) -> Result<T, StateError> {
        let inner = self.lock()?;
        operation(&inner.cache)
    }

    fn mutate<T>(
        &self,
        operation: impl FnOnce(&InMemoryState<C>) -> Result<T, StateError>,
    ) -> Result<T, StateError> {
        let mut inner = self.lock()?;
        let SqliteInner { connection, cache } = &mut *inner;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|_| StateError::Corrupt)?;
        let before = cache.snapshot()?;
        let result = match operation(cache) {
            Ok(result) => result,
            Err(error) => {
                cache.restore(before)?;
                return Err(error);
            }
        };
        let after = cache.snapshot()?;
        if after.head != before.head
            && let Err(error) = persist_delta(&transaction, &before, &after)
        {
            cache.restore(before)?;
            let _ = error;
            return Err(StateError::Corrupt);
        }
        if transaction.commit().is_err() {
            cache.restore(before)?;
            return Err(StateError::Corrupt);
        }
        Ok(result)
    }
}

impl<C: ClockPort> WorkflowStatePort for SqliteState<C> {
    fn open_assignment(&self, opening: &AssignmentOpening) -> Result<StateApplied, StateError> {
        self.mutate(|state| WorkflowStatePort::open_assignment(state, opening))
    }

    fn append_attempt(&self, opening: &AttemptOpening) -> Result<StateApplied, StateError> {
        self.mutate(|state| WorkflowStatePort::append_attempt(state, opening))
    }

    fn record_decision(&self, record: &DecisionRecord) -> Result<StateApplied, StateError> {
        self.mutate(|state| WorkflowStatePort::record_decision(state, record))
    }

    fn activate_profile(&self, opening: &ActivationOpening) -> Result<StateApplied, StateError> {
        self.mutate(|state| WorkflowStatePort::activate_profile(state, opening))
    }

    fn deactivate_profile(
        &self,
        operation: &OperationId,
        actor: &ActorId,
        profile: &ProfileName,
    ) -> Result<StateApplied, StateError> {
        self.mutate(|state| WorkflowStatePort::deactivate_profile(state, operation, actor, profile))
    }

    fn append_signal(&self, draft: &SignalDraft) -> Result<(Signal, StateApplied), StateError> {
        self.mutate(|state| WorkflowStatePort::append_signal(state, draft))
    }

    fn fenced_report(
        &self,
        action: &FencedAction,
        draft: &ReportDraft,
    ) -> Result<(ReportOutcome, FencedResponse), StateError> {
        self.mutate(|state| WorkflowStatePort::fenced_report(state, action, draft))
    }

    fn fenced_evidence(
        &self,
        action: &FencedAction,
        evidence: &Evidence,
    ) -> Result<(EvidenceOutcome, FencedResponse), StateError> {
        self.mutate(|state| WorkflowStatePort::fenced_evidence(state, action, evidence))
    }

    fn fenced_submit_handoff(
        &self,
        action: &FencedAction,
        handoff: &HandoffRecord,
    ) -> Result<(SubmissionOutcome, FencedResponse), StateError> {
        self.mutate(|state| WorkflowStatePort::fenced_submit_handoff(state, action, handoff))
    }

    fn fenced_abort_attempt(&self, call: &FencedCall) -> Result<FencedResponse, StateError> {
        self.mutate(|state| WorkflowStatePort::fenced_abort_attempt(state, call))
    }

    fn renew_lease(
        &self,
        call: &FencedCall,
        until: Timestamp,
    ) -> Result<(Lease, FencedResponse), StateError> {
        self.mutate(|state| WorkflowStatePort::renew_lease(state, call, until))
    }

    fn persist_envelope(
        &self,
        operation: &OperationId,
        subject: &LaunchSubject,
        envelope: &EnvelopeSnapshot,
    ) -> Result<StateApplied, StateError> {
        self.mutate(|state| {
            WorkflowStatePort::persist_envelope(state, operation, subject, envelope)
        })
    }

    fn envelope(&self, subject: &LaunchSubject) -> Result<EnvelopeSnapshot, StateError> {
        self.read(|state| WorkflowStatePort::envelope(state, subject))
    }

    fn bind_runtime_handle(
        &self,
        operation: &OperationId,
        subject: &LaunchSubject,
        handle: &RuntimeHandle,
    ) -> Result<StateApplied, StateError> {
        self.mutate(|state| {
            WorkflowStatePort::bind_runtime_handle(state, operation, subject, handle)
        })
    }

    fn unbind_runtime_handle(
        &self,
        operation: &OperationId,
        subject: &LaunchSubject,
    ) -> Result<StateApplied, StateError> {
        self.mutate(|state| WorkflowStatePort::unbind_runtime_handle(state, operation, subject))
    }

    fn runtime_handle(&self, subject: &LaunchSubject) -> Result<Option<RuntimeHandle>, StateError> {
        self.read(|state| WorkflowStatePort::runtime_handle(state, subject))
    }

    fn record_runtime_observation(
        &self,
        operation: &OperationId,
        record: &RuntimeObservationRecord,
    ) -> Result<StateApplied, StateError> {
        self.mutate(|state| WorkflowStatePort::record_runtime_observation(state, operation, record))
    }

    fn runtime_observation(
        &self,
        operation: &OperationId,
    ) -> Result<RuntimeObservationRecord, StateError> {
        self.read(|state| WorkflowStatePort::runtime_observation(state, operation))
    }

    fn record_application_attempt(
        &self,
        attempt: &ApplicationAttempt,
    ) -> Result<StateApplied, StateError> {
        self.mutate(|state| WorkflowStatePort::record_application_attempt(state, attempt))
    }

    fn record_application_receipt(
        &self,
        receipt: &ApplicationReceipt,
    ) -> Result<StateApplied, StateError> {
        self.mutate(|state| WorkflowStatePort::record_application_receipt(state, receipt))
    }

    fn assignment(&self, id: &AssignmentId) -> Result<AssignmentView, StateError> {
        self.read(|state| WorkflowStatePort::assignment(state, id))
    }

    fn evidence_for(&self, attempt: &AttemptId) -> Result<Vec<EvidenceRecord>, StateError> {
        self.read(|state| WorkflowStatePort::evidence_for(state, attempt))
    }

    fn signals_for(&self, attempt: &AttemptId) -> Result<Vec<Signal>, StateError> {
        self.read(|state| WorkflowStatePort::signals_for(state, attempt))
    }

    fn handoff(&self, id: &HandoffId) -> Result<HandoffRecord, StateError> {
        self.read(|state| WorkflowStatePort::handoff(state, id))
    }

    fn decision(&self, operation: &OperationId) -> Result<DecisionRecord, StateError> {
        self.read(|state| WorkflowStatePort::decision(state, operation))
    }

    fn active_occupants(&self, profile: &ProfileName) -> Result<Vec<ActorId>, StateError> {
        self.read(|state| WorkflowStatePort::active_occupants(state, profile))
    }

    fn pending_applications(&self) -> Result<Vec<PendingApplication>, StateError> {
        self.read(WorkflowStatePort::pending_applications)
    }

    fn superseded_applications(&self) -> Result<Vec<SupersededApplication>, StateError> {
        self.read(WorkflowStatePort::superseded_applications)
    }

    fn unresolved_signals(&self, recipient: Option<&ActorId>) -> Result<Vec<Signal>, StateError> {
        self.read(|state| WorkflowStatePort::unresolved_signals(state, recipient))
    }

    fn audit_events(&self, query: &AuditQuery) -> Result<Vec<AuditEvent>, StateError> {
        self.read(|state| WorkflowStatePort::audit_events(state, query))
    }
}
