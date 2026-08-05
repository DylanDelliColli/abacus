//! Scribe and the Ledger: durable state for ABACUS.
//!
//! The binding contract is `README.md` in this folder. This crate owns
//! the Scribe process (`abacus-scribe`), SQLite (WAL) persistence,
//! schema and migrations, leases, immutable Signal/evidence/decision
//! records, audit events, repository identity, and the local client/
//! server transport. It implements core persistence ports and absorbs
//! no work-graph, mailbox, or runtime policy.

#![forbid(unsafe_code)]

mod migrations;

pub use migrations::{MigrationError, MigrationReport, apply_migrations, latest_schema_version};

#[cfg(test)]
mod tests {
    // Scaffold placeholder proving the per-module hermetic test target
    // (`cargo test -p abacus-state`) exists and runs. Replaced by real
    // persistence tests as ABACUS-9NH.7 through .11 land.
    #[test]
    fn hermetic_test_target_runs() {}
}
