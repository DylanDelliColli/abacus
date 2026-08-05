//! Provider-neutral ports consumed by core use cases.
//!
//! These traits are the seam every adapter implements (`abacus-state`,
//! `abacus-work`, `abacus-runtime`, and the composition root) and the
//! only way infrastructure behavior enters the domain. They are not a
//! generic plugin framework: a port exists only for behavior a core use
//! case consumes, with a production adapter and a hermetic fake
//! (core contract, "Required ports"). No provider types — no SQLite,
//! subprocess, `br`, `bv`, or Herdr vocabulary — appears here, and
//! changes to this module are C1 or above and require cross-review.
//!
//! Clocks and identifier generation are ports too (I13, core contract
//! invariant 9): current time and new IDs are inputs, never ambient
//! calls.

use crate::content::{CommitId, ContentHash};
use crate::edit_scope::WorkPath;
use crate::id::BeadId;
use crate::lease::Timestamp;

/// Revision of the work graph observed at read time; composed reads
/// bracket advice and mutation decisions with it (stable-read binding).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct WorkRevision(pub ContentHash);

/// Normalized bead observation crossing the work seam: identity,
/// contract hash, and the scope facts ADR-0002 evaluates. Raw provider
/// labels never cross; the work facade normalizes first.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BeadSnapshot {
    pub id: BeadId,
    /// Covers the bead contract including raw declared-key scope labels
    /// (ADR-0002 §1); Acceptance rechecks it.
    pub content_hash: ContentHash,
    /// The normalized single-valued scope map, as `(key, value)` pairs
    /// in key order.
    pub scope_map: Vec<(String, String)>,
}

/// Bounded curated close reasons written to the work graph (ADR-0001
/// §9.4). Extending this set is a C1 change.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CloseReason {
    AcceptedHandoff,
    CancelledObsolete,
}

/// Normalized work-seam failures. Adapters map provider errors into
/// these; raw provider codes and strings stay behind the facade (I9).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkError {
    ProviderUnavailable,
    /// Pinned-version or schema mismatch: fail closed.
    VersionMismatch,
    /// Provider output rejected whole at the adapter boundary.
    MalformedOutput,
    NotFound,
    /// The graph moved between bracketed reads; the caller re-reads.
    RevisionConflict,
    /// Scope-label normalization refusals (ADR-0002 §1).
    ScopeLabelMalformed { label: String },
    ScopeLabelConflict { key: String },
}

/// Normalized reads and decision-gated mutations of the work graph.
/// Only `abacus-work` implements this against a live provider; the
/// worker role never sees a closing verb (its facade surface simply
/// lacks one — enforcement is structural, not advisory).
pub trait WorkGraphPort {
    /// Ready-to-work beads with the revision they were read at.
    fn ready(&self) -> Result<(WorkRevision, Vec<BeadSnapshot>), WorkError>;

    /// One bead's current snapshot.
    fn show(&self, id: &BeadId) -> Result<(WorkRevision, BeadSnapshot), WorkError>;

    /// Apply the status projection of a committed Acceptance decision
    /// (ADR-0001 §3 saga step 2): close with a curated reason, bound to
    /// the revision the decision validated. A revision mismatch is a
    /// `RevisionConflict`, surfaced for explicit reconciliation.
    fn close(
        &self,
        id: &BeadId,
        reason: CloseReason,
        expected: &WorkRevision,
    ) -> Result<WorkRevision, WorkError>;
}

/// Optional ordering advice (I8): never authoritative, never required.
pub trait WorkAdvicePort {
    /// Advise an ordering over the given ready set. `Unavailable` is a
    /// normal outcome; the caller falls back deterministically and the
    /// degradation is noted, never an error.
    fn advise(&self, revision: &WorkRevision, ready: &[BeadId]) -> AdviceOutcome;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AdviceOutcome {
    /// A validated recommendation bound to the revision it was computed
    /// against; stale or unbound advice must be discarded by the caller.
    Advice { order: Vec<BeadId>, bound_to: WorkRevision },
    Unavailable,
}

/// Deterministic fallback ordering (core-owned policy, I8): stable and
/// independent of any advisor — lexicographic by bead ID.
pub fn fallback_order(ready: &[BeadSnapshot]) -> Vec<BeadId> {
    let mut ids: Vec<BeadId> = ready.iter().map(|b| b.id.clone()).collect();
    ids.sort_by(|a, b| a.as_str().cmp(b.as_str()));
    ids
}

/// Opaque, generation-fenced runtime handle (ADR-0001 §3; Herdr spike):
/// session namespace plus pane plus terminal/process generation. Never
/// a durable ABACUS identity; a changed generation makes the handle
/// stale until explicitly re-associated.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct RuntimeHandle {
    pub session: String,
    pub pane: String,
    pub generation: String,
}

/// Non-authoritative liveness observation (CONTEXT §5): advisory input
/// to state composition, never a completion or an idle verdict.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LivenessObservation {
    pub observed_at: Timestamp,
    pub kind: LivenessKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LivenessKind {
    Running,
    Exited,
    NotFound,
    Unreachable,
    /// The handle's generation no longer matches the live terminal:
    /// stale until an explicit re-association decision.
    StaleGeneration,
}

/// Normalized runtime-seam failures.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RuntimeError {
    ProviderUnavailable,
    VersionMismatch,
    /// Host approval for the runtime facade is absent (fail closed;
    /// no fallback path exists by design).
    NotPermitted,
    HandleStale,
    MalformedOutput,
}

/// What to launch: explicit environment, never ambient (I13).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaunchSpec {
    /// Provider-neutral agent kind tag, e.g. "claude" or "codex";
    /// validated against configuration by the composer.
    pub agent_kind: String,
    pub working_directory: WorkPath,
    /// Environment variables set explicitly for the session.
    pub environment: Vec<(String, String)>,
}

/// Best-effort delivery report for a live prompt or doorbell (I6):
/// informational only — correctness never depends on it, and no retry
/// machinery consumes it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DeliveryReport {
    Submitted,
    /// Delivery attempted; submission unconfirmed within the bound.
    Ambiguous,
}

/// Agent/session lifecycle mechanics. Only `abacus-runtime` implements
/// this against a live provider.
pub trait RuntimePort {
    fn launch(&self, spec: &LaunchSpec) -> Result<RuntimeHandle, RuntimeError>;
    fn observe(&self, handle: &RuntimeHandle) -> Result<LivenessObservation, RuntimeError>;
    /// Deliver a content-free doorbell or bounded live prompt. The
    /// durable Signal is already committed before this is called.
    fn ring(&self, handle: &RuntimeHandle, prompt: &str) -> Result<DeliveryReport, RuntimeError>;
    fn stop(&self, handle: &RuntimeHandle) -> Result<(), RuntimeError>;
}

/// Commit facts for evidence validation (core contract: the Git
/// implementation lives privately in the composition module behind
/// this port).
pub trait CommitVerifierPort {
    fn commit_exists(&self, commit: &CommitId) -> bool;
    /// Normalized changed paths of `commit` relative to `base`.
    fn changed_paths(&self, base: &CommitId, commit: &CommitId) -> Result<Vec<WorkPath>, VerifyError>;
    /// Content digest of one file at one commit; `None` when absent.
    fn file_digest(&self, commit: &CommitId, path: &WorkPath) -> Result<Option<ContentHash>, VerifyError>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum VerifyError {
    UnknownCommit,
    Unavailable,
}

/// Time as an input (I13).
pub trait ClockPort {
    fn now(&self) -> Timestamp;
}

/// Identifier generation as an input (core contract invariant 9). The
/// composer wraps raw tokens into typed IDs via their validating
/// constructors.
pub trait IdGeneratorPort {
    fn generate(&self) -> String;
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Hermetic fake proving the work port is implementable and
    /// object-safe, with revision-conflict behavior.
    struct FakeWork {
        revision: WorkRevision,
        beads: Vec<BeadSnapshot>,
    }

    fn rev(fill: char) -> WorkRevision {
        WorkRevision(ContentHash::new(&fill.to_string().repeat(64)).unwrap())
    }

    fn snapshot(id: &str, fill: char) -> BeadSnapshot {
        BeadSnapshot {
            id: BeadId::new(id).unwrap(),
            content_hash: ContentHash::new(&fill.to_string().repeat(64)).unwrap(),
            scope_map: vec![("area".into(), "core".into())],
        }
    }

    impl WorkGraphPort for FakeWork {
        fn ready(&self) -> Result<(WorkRevision, Vec<BeadSnapshot>), WorkError> {
            Ok((self.revision.clone(), self.beads.clone()))
        }

        fn show(&self, id: &BeadId) -> Result<(WorkRevision, BeadSnapshot), WorkError> {
            self.beads
                .iter()
                .find(|b| &b.id == id)
                .cloned()
                .map(|b| (self.revision.clone(), b))
                .ok_or(WorkError::NotFound)
        }

        fn close(
            &self,
            id: &BeadId,
            _reason: CloseReason,
            expected: &WorkRevision,
        ) -> Result<WorkRevision, WorkError> {
            if *expected != self.revision {
                return Err(WorkError::RevisionConflict);
            }
            self.show(id)?;
            Ok(rev('f'))
        }
    }

    #[test]
    fn work_port_round_trip_and_revision_conflict() {
        let port: &dyn WorkGraphPort = &FakeWork {
            revision: rev('a'),
            beads: vec![snapshot("ABACUS-x", 'b'), snapshot("ABACUS-a", 'c')],
        };
        let (revision, ready) = port.ready().unwrap();
        assert_eq!(ready.len(), 2);
        let closed = port.close(&ready[0].id, CloseReason::AcceptedHandoff, &revision);
        assert!(closed.is_ok());
        assert_eq!(
            port.close(&ready[0].id, CloseReason::AcceptedHandoff, &rev('9')),
            Err(WorkError::RevisionConflict)
        );
        assert_eq!(
            port.show(&BeadId::new("ABACUS-zzz").unwrap()),
            Err(WorkError::NotFound)
        );
    }

    #[test]
    fn fallback_order_is_deterministic_and_advice_free() {
        let ready = vec![snapshot("ABACUS-x", 'b'), snapshot("ABACUS-a", 'c')];
        let order = fallback_order(&ready);
        assert_eq!(
            order.iter().map(|id| id.as_str()).collect::<Vec<_>>(),
            vec!["ABACUS-a", "ABACUS-x"]
        );
        assert_eq!(fallback_order(&ready), order);
    }

    /// An advisor that is absent changes nothing: the caller composes
    /// the fallback (I8).
    struct AbsentAdvisor;
    impl WorkAdvicePort for AbsentAdvisor {
        fn advise(&self, _revision: &WorkRevision, _ready: &[BeadId]) -> AdviceOutcome {
            AdviceOutcome::Unavailable
        }
    }

    #[test]
    fn absent_advice_degrades_to_fallback() {
        let ready = vec![snapshot("ABACUS-b", 'b'), snapshot("ABACUS-a", 'c')];
        let ids: Vec<BeadId> = ready.iter().map(|b| b.id.clone()).collect();
        let advisor: &dyn WorkAdvicePort = &AbsentAdvisor;
        let order = match advisor.advise(&rev('a'), &ids) {
            AdviceOutcome::Advice { order, bound_to } if bound_to == rev('a') => order,
            _ => fallback_order(&ready),
        };
        assert_eq!(order.first().unwrap().as_str(), "ABACUS-a");
    }

    /// Hermetic fake runtime proving handle fencing shape: a stale
    /// generation is observable, never silently reattached.
    struct FakeRuntime {
        live_generation: String,
    }

    impl RuntimePort for FakeRuntime {
        fn launch(&self, _spec: &LaunchSpec) -> Result<RuntimeHandle, RuntimeError> {
            Ok(RuntimeHandle {
                session: "s1".into(),
                pane: "w1:p1".into(),
                generation: self.live_generation.clone(),
            })
        }

        fn observe(&self, handle: &RuntimeHandle) -> Result<LivenessObservation, RuntimeError> {
            let kind = if handle.generation == self.live_generation {
                LivenessKind::Running
            } else {
                LivenessKind::StaleGeneration
            };
            Ok(LivenessObservation { observed_at: Timestamp(1), kind })
        }

        fn ring(&self, handle: &RuntimeHandle, _prompt: &str) -> Result<DeliveryReport, RuntimeError> {
            if handle.generation != self.live_generation {
                return Err(RuntimeError::HandleStale);
            }
            Ok(DeliveryReport::Ambiguous)
        }

        fn stop(&self, _handle: &RuntimeHandle) -> Result<(), RuntimeError> {
            Ok(())
        }
    }

    #[test]
    fn runtime_handles_are_generation_fenced() {
        let runtime = FakeRuntime { live_generation: "gen-2".into() };
        let port: &dyn RuntimePort = &runtime;
        let spec = LaunchSpec {
            agent_kind: "claude".into(),
            working_directory: WorkPath::new("worktrees/abacus-x").unwrap(),
            environment: vec![("ABACUS_SOCKET".into(), "path".into())],
        };
        let live = port.launch(&spec).unwrap();
        assert_eq!(port.observe(&live).unwrap().kind, LivenessKind::Running);

        let stale = RuntimeHandle { generation: "gen-1".into(), ..live.clone() };
        assert_eq!(port.observe(&stale).unwrap().kind, LivenessKind::StaleGeneration);
        assert_eq!(port.ring(&stale, "doorbell"), Err(RuntimeError::HandleStale));
        // An ambiguous delivery is a normal, non-error outcome (I6).
        assert_eq!(port.ring(&live, "doorbell"), Ok(DeliveryReport::Ambiguous));
    }
}
