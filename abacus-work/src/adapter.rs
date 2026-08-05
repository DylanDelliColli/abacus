//! The internal provider seam.
//!
//! `WorkProvider` is what a pinned `br` process adapter implements
//! (ABACUS-omw.2); `AdviceProvider` is what a pinned `bv` robot-mode
//! adapter implements (ABACUS-omw.5). Neither is public API of ABACUS:
//! callers outside this crate see only the core ports, which the facade
//! in [`crate::facade`] implements on top of these traits.
//!
//! The split exists so that provider-command sequencing, ambiguity
//! reconciliation, and precondition checking live in ONE place (the
//! facade) instead of being re-derived by every provider adapter. An
//! adapter reports what it observed; it does not decide what that means.

use abacus_core::ports::{CloseReason, WorkError, WorkRevision, WorkStatus};
use abacus_core::{BeadId, ContentHash, OperationId};

/// Provider facts before ABACUS applies scope-label normalization.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawBeadSnapshot {
    pub id: BeadId,
    pub content_hash: ContentHash,
    pub raw_labels: Vec<String>,
    pub priority: u8,
}

/// Provider inspection facts before the facade normalizes the embedded
/// snapshot. An adapter cannot manufacture the core-facing scope map or
/// priority because neither exists in this type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawBeadStatusView {
    pub snapshot: RawBeadSnapshot,
    pub status: WorkStatus,
    pub revision: WorkRevision,
}

/// The state a decision-gated mutation drives a bead toward.
///
/// Closed carries the curated [`CloseReason`] rather than a provider
/// string: module contract "Owns" line 73 — arbitrary provider text
/// never enters the mutation path, and the adapter renders the reason.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TargetStatus {
    InProgress,
    Closed(CloseReason),
}

/// What a provider adapter observed about its own mutation.
///
/// Deliberately smaller than [`abacus_core::ports::MutationOutcome`]:
/// an adapter cannot report `EffectAlreadyPresent`, because deciding
/// that an effect was already present requires a read-before-write
/// comparison the facade owns. An adapter that cannot tell whether its
/// command took effect returns [`ProviderMutation::Ambiguous`] and the
/// facade reconciles — never the adapter, and never by retrying blind.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProviderMutation {
    Applied {
        before: WorkRevision,
        after: WorkRevision,
        /// Audit-safe normalized summary; the facade bounds it.
        summary: String,
    },
    /// Outcome unknown: the command may or may not have applied.
    Ambiguous,
}

/// Raw reads and normalized mutations over one work graph.
///
/// Implementors do NOT enforce expected-revision preconditions and do
/// NOT interpret ambiguity; see [`ProviderMutation`]. Snapshot facts stay
/// raw until the facade so scope-label and priority normalization cannot
/// be bypassed by an adapter.
pub trait WorkProvider {
    fn ready(&self) -> Result<(WorkRevision, Vec<RawBeadSnapshot>), WorkError>;

    fn inspect(&self, id: &BeadId) -> Result<RawBeadStatusView, WorkError>;

    /// Drive `id` to `target`, carrying the authorizing operation
    /// identity for the provider's own audit trail.
    ///
    /// # Contract on `Err` (load-bearing — read before writing an adapter)
    ///
    /// Returning `Err` asserts that the mutation **definitively did not
    /// take effect**. The facade trusts that assertion: an `Err` skips
    /// reconciliation entirely, so the caller may safely re-drive the
    /// operation after addressing the cause.
    ///
    /// If the adapter cannot establish that — the process was killed,
    /// the command ran but its output could not be read, a timeout
    /// elapsed with the write possibly in flight — it MUST return
    /// [`ProviderMutation::Ambiguous`] instead. Reporting such a case as
    /// `Err(Busy)` or `Err(ProviderUnavailable)` makes a retry look safe
    /// when the mutation may already have landed, which is the
    /// double-apply this seam exists to prevent.
    ///
    /// When in doubt, `Ambiguous` is always the safe answer: it costs one
    /// extra read and is never wrong.
    fn set_status(
        &self,
        id: &BeadId,
        target: TargetStatus,
        operation: &OperationId,
    ) -> Result<ProviderMutation, WorkError>;
}

/// One graph-revision-bound advisory analysis.
///
/// The advisor is optional infrastructure: it returns its own ordering
/// and the revision it analyzed, and the facade decides whether that
/// binding is still honest. An advisor never mutates.
pub trait AdviceProvider {
    /// `Err` reports the advisor degraded; the facade maps it to a
    /// noted outcome rather than a work error, because advice is never
    /// required (core invariant I8).
    fn analyze(
        &self,
        revision: &WorkRevision,
        ready: &[BeadId],
    ) -> Result<AdviceAnalysis, abacus_core::ports::AdviceDegradation>;
}

/// A raw advisor response, before the facade validates its binding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdviceAnalysis {
    pub order: Vec<BeadId>,
    /// The revision the advisor claims it analyzed. The facade rejects
    /// this response outright when it does not match the revision the
    /// caller asked about.
    pub analyzed: WorkRevision,
}
