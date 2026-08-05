//! The work-graph seam: the only ABACUS module that speaks to `br`/`bv`.
//!
//! The binding contract is `README.md` in this folder. This crate
//! implements the core work ports over an internal provider seam, so
//! that provider command sequences, schemas, version quirks, and
//! failure interpretation stay inside this module. Callers reason about
//! beads and outcomes, never subprocesses.
//!
//! Layering (ADR-0001 §3): depends on `abacus-core` only. It cannot
//! import state, runtime, or CLI, and it never opens the Ledger.

#![forbid(unsafe_code)]

pub mod adapter;
pub mod contract;
pub mod facade;
pub mod fake;
pub mod id_seam;
pub mod scope_labels;

pub use adapter::{
    AdviceAnalysis, AdviceProvider, ProviderMutation, RawBeadSnapshot, RawBeadStatusView,
    TargetStatus, WorkProvider,
};
pub use facade::{AdviceFacade, MAX_SUMMARY_LEN, WorkFacade};
pub use fake::{FakeAdvisor, FakeWorkProvider, Script};
pub use id_seam::{
    IdSeamError, NormalizedBead, PROVIDER_PREFIX, ProviderBeadId, RawProviderBead,
    assert_single_namespace, from_provider, normalize, to_provider,
};
