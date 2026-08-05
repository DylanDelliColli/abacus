//! Scribe and the Ledger: durable state for ABACUS.
//!
//! The binding contract is `README.md` in this folder. This crate owns
//! the Scribe process (`abacus-scribe`), SQLite (WAL) persistence,
//! schema and migrations, leases, immutable Signal/evidence/decision
//! records, audit events, repository identity, and the local client/
//! server transport. It implements core persistence ports and absorbs
//! no work-graph, mailbox, or runtime policy.

#![forbid(unsafe_code)]

pub mod contract;
mod memory;
mod migrations;

pub use memory::{InMemoryState, ManualClock};
pub use migrations::{MigrationError, MigrationReport, apply_migrations, latest_schema_version};
