//! Hermetic in-memory providers.
//!
//! These are the default test substrate for this crate and the harness
//! the `br`/`bv` adapter beads (ABACUS-omw.2/.5) reuse: a contract test
//! written against `FakeWorkProvider` states an expectation about the
//! FACADE, so the same expectation holds for any conforming adapter.
//!
//! Nothing here touches a process, a socket, `.beads`, or Git — module
//! contract, "No default test invokes installed `br`/`bv`".

use std::cell::RefCell;
use std::collections::{BTreeMap, VecDeque};

use abacus_core::ports::{
    AdviceDegradation, BeadSnapshot, BeadStatusView, Priority, WorkError, WorkRevision, WorkStatus,
};
use abacus_core::{BeadId, ContentHash, OperationId, ScopeMap};

use crate::adapter::{
    AdviceAnalysis, AdviceProvider, ProviderMutation, TargetStatus, WorkProvider,
};
use crate::contract::{Behavior, Scenario};

/// Build a deterministic 64-hex revision from a small seed, so tests can
/// name revisions as `rev(1)`, `rev(2)` instead of pasting hashes.
pub fn rev(seed: u32) -> WorkRevision {
    WorkRevision(hash(seed))
}

/// Build a deterministic 64-hex content hash from a small seed.
pub fn hash(seed: u32) -> ContentHash {
    ContentHash::new(&format!("{seed:064x}")).expect("seeded hash is 64 lowercase hex")
}

/// Build a normalized snapshot with an empty scope map.
pub fn snapshot(id: &BeadId, content: u32, priority: u8) -> BeadSnapshot {
    BeadSnapshot {
        id: id.clone(),
        content_hash: hash(content),
        scope_map: ScopeMap::new(Vec::new()).expect("empty scope map is valid"),
        priority: Priority::new(priority).expect("seeded priority within 0..=4"),
    }
}

/// One recorded provider interaction, so a test can assert the facade
/// issued exactly the calls it should — notably that reconciliation
/// re-inspects rather than blindly retrying a mutation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Call {
    Ready,
    Inspect(BeadId),
    SetStatus {
        id: BeadId,
        target: TargetStatus,
        operation: OperationId,
    },
}

/// Queued provider behavior. Each `set_status` consumes one entry; when
/// the queue empties the provider falls back to applying the mutation
/// normally, so a test only scripts the steps it cares about.
#[derive(Debug, Default, Clone)]
pub struct Script {
    responses: VecDeque<ScriptStep>,
}

#[derive(Debug, Clone)]
enum ScriptStep {
    /// Report ambiguity WITHOUT applying the effect.
    AmbiguousLost,
    /// Report ambiguity but apply the effect anyway — the dangerous
    /// case: the command landed and only the answer was lost.
    AmbiguousApplied,
    Fail(WorkError),
}

impl Script {
    pub fn new() -> Self {
        Self::default()
    }

    /// The mutation did not take effect, and the provider knows only
    /// that it cannot tell.
    pub fn ambiguous_lost(mut self) -> Self {
        self.responses.push_back(ScriptStep::AmbiguousLost);
        self
    }

    /// The mutation DID take effect but the acknowledgement was lost.
    pub fn ambiguous_applied(mut self) -> Self {
        self.responses.push_back(ScriptStep::AmbiguousApplied);
        self
    }

    pub fn fail(mut self, error: WorkError) -> Self {
        self.responses.push_back(ScriptStep::Fail(error));
        self
    }
}

struct State {
    beads: BTreeMap<String, BeadStatusView>,
    tick: u32,
    script: Script,
    ready_error: Option<WorkError>,
    inspect_error_after_mutation: Option<WorkError>,
    mutation_attempted: bool,
    calls: Vec<Call>,
}

/// In-memory [`WorkProvider`].
pub struct FakeWorkProvider {
    inner: RefCell<State>,
}

impl FakeWorkProvider {
    /// Start with one open bead at revision `rev(tick)`.
    pub fn with_bead(id: &BeadId, status: WorkStatus, tick: u32) -> Self {
        let mut beads = BTreeMap::new();
        beads.insert(
            id.as_str().to_owned(),
            BeadStatusView {
                snapshot: snapshot(id, tick, 2),
                status,
                revision: rev(tick),
            },
        );
        Self {
            inner: RefCell::new(State {
                beads,
                tick,
                script: Script::new(),
                ready_error: None,
                inspect_error_after_mutation: None,
                mutation_attempted: false,
                calls: Vec::new(),
            }),
        }
    }

    /// Materialize a [`Scenario`] from the portable contract suite.
    ///
    /// This is the fake's conformance entry point: `omw.2`'s `br`
    /// adapter provides its own equivalent over checked-in fixtures, and
    /// both then satisfy the same suite.
    pub fn from_scenario(scenario: &Scenario) -> Self {
        let provider = Self::with_bead(&scenario.bead, scenario.status, scenario.tick);
        match &scenario.behavior {
            Behavior::Normal => provider,
            Behavior::AmbiguousApplied => provider.scripted(Script::new().ambiguous_applied()),
            Behavior::AmbiguousLost => provider.scripted(Script::new().ambiguous_lost()),
            Behavior::Fails(error) => provider.scripted(Script::new().fail(error.clone())),
        }
    }

    pub fn scripted(self, script: Script) -> Self {
        self.inner.borrow_mut().script = script;
        self
    }

    pub fn failing_ready(self, error: WorkError) -> Self {
        self.inner.borrow_mut().ready_error = Some(error);
        self
    }

    /// Make `inspect` fail only AFTER a mutation has been attempted, so a
    /// test can drive the case where reconciliation of an ambiguous
    /// outcome is itself impossible.
    pub fn failing_inspect_after_mutation(self, error: WorkError) -> Self {
        self.inner.borrow_mut().inspect_error_after_mutation = Some(error);
        self
    }

    pub fn calls(&self) -> Vec<Call> {
        self.inner.borrow().calls.clone()
    }

    /// Current normalized status, for asserting real provider effect
    /// rather than only the facade's reported outcome.
    pub fn status_of(&self, id: &BeadId) -> Option<WorkStatus> {
        self.inner
            .borrow()
            .beads
            .get(id.as_str())
            .map(|view| view.status)
    }
}

/// Apply `target` to the stored view and advance the revision.
fn apply(state: &mut State, id: &BeadId) -> Option<(WorkRevision, WorkRevision)> {
    let before = state.beads.get(id.as_str())?.revision.clone();
    state.tick += 1;
    let after = rev(state.tick);
    let view = state.beads.get_mut(id.as_str())?;
    view.revision = after.clone();
    Some((before, after))
}

fn set_stored_status(state: &mut State, id: &BeadId, target: TargetStatus) {
    if let Some(view) = state.beads.get_mut(id.as_str()) {
        view.status = match target {
            TargetStatus::InProgress => WorkStatus::InProgress,
            TargetStatus::Closed(reason) => WorkStatus::Closed {
                observed_reason: match reason {
                    abacus_core::ports::CloseReason::AcceptedHandoff => {
                        abacus_core::ports::ObservedCloseReason::AcceptedHandoff
                    }
                    abacus_core::ports::CloseReason::CancelledObsolete => {
                        abacus_core::ports::ObservedCloseReason::CancelledObsolete
                    }
                },
            },
        };
    }
}

impl WorkProvider for FakeWorkProvider {
    fn ready(&self) -> Result<(WorkRevision, Vec<BeadSnapshot>), WorkError> {
        let mut state = self.inner.borrow_mut();
        state.calls.push(Call::Ready);
        if let Some(error) = state.ready_error.clone() {
            return Err(error);
        }
        let revision = rev(state.tick);
        let open = state
            .beads
            .values()
            .filter(|view| view.status == WorkStatus::Open)
            .map(|view| view.snapshot.clone())
            .collect();
        Ok((revision, open))
    }

    fn inspect(&self, id: &BeadId) -> Result<BeadStatusView, WorkError> {
        let mut state = self.inner.borrow_mut();
        state.calls.push(Call::Inspect(id.clone()));
        if state.mutation_attempted
            && let Some(error) = state.inspect_error_after_mutation.clone()
        {
            return Err(error);
        }
        state
            .beads
            .get(id.as_str())
            .cloned()
            .ok_or(WorkError::NotFound)
    }

    fn set_status(
        &self,
        id: &BeadId,
        target: TargetStatus,
        operation: &OperationId,
    ) -> Result<ProviderMutation, WorkError> {
        let mut state = self.inner.borrow_mut();
        state.calls.push(Call::SetStatus {
            id: id.clone(),
            target,
            operation: operation.clone(),
        });
        state.mutation_attempted = true;

        match state.script.responses.pop_front() {
            Some(ScriptStep::Fail(error)) => Err(error),
            Some(ScriptStep::AmbiguousLost) => Ok(ProviderMutation::Ambiguous),
            Some(ScriptStep::AmbiguousApplied) => {
                set_stored_status(&mut state, id, target);
                apply(&mut state, id);
                Ok(ProviderMutation::Ambiguous)
            }
            None => {
                if !state.beads.contains_key(id.as_str()) {
                    return Err(WorkError::NotFound);
                }
                set_stored_status(&mut state, id, target);
                let (before, after) = apply(&mut state, id).ok_or(WorkError::NotFound)?;
                Ok(ProviderMutation::Applied {
                    before,
                    after,
                    summary: format!("{} -> {:?}", id.as_str(), target),
                })
            }
        }
    }
}

/// In-memory [`AdviceProvider`].
pub struct FakeAdvisor {
    outcome: Result<AdviceAnalysis, AdviceDegradation>,
}

impl FakeAdvisor {
    pub fn advising(order: Vec<BeadId>, analyzed: WorkRevision) -> Self {
        Self {
            outcome: Ok(AdviceAnalysis { order, analyzed }),
        }
    }

    pub fn degraded(reason: AdviceDegradation) -> Self {
        Self {
            outcome: Err(reason),
        }
    }
}

impl AdviceProvider for FakeAdvisor {
    fn analyze(
        &self,
        _revision: &WorkRevision,
        _ready: &[BeadId],
    ) -> Result<AdviceAnalysis, AdviceDegradation> {
        self.outcome.clone()
    }
}
