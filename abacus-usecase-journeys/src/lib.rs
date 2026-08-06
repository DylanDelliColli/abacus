//! The hermetic vertical journey (ABACUS-3ju).
//!
//! This crate is deliberately EMPTY of production code. It exists to
//! hold exactly four journey tests that drive the production use-case
//! composition in `abacus_core::usecase` over the canonical fakes, so
//! the modules are proven to compose as a machine rather than only to
//! satisfy their individual contracts.
//!
//! Binding constraints (operator-ratified, both reviewer lineages):
//!
//! - The driver may supply inputs, clocks, scripts, and assertions. It
//!   may NOT own transition policy, transaction choreography,
//!   compensation rules, receipt choice, or direct state shortcuts.
//!   Anything the journey needs that production wiring lacks is a
//!   missing production seam, not test scaffolding.
//! - Exactly four paths, pinned. New combinatorics belong to the owning
//!   module's suite unless an existing journey's end-to-end semantic
//!   changes.
//! - Helpers stay private and literal: no reusable orchestration DSL,
//!   no shared fixture package, no "integration common" crate. That
//!   growth is what this leaf exists to avoid.
