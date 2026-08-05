//! SQLite schema ownership and ordered migrations.
//!
//! Migrations are deliberately kept as explicit, immutable SQL batches. The
//! caller owns database-path selection and backup policy; this module only
//! configures a connection, applies ordered schema changes transactionally,
//! and reports a typed failure without hiding the prior readable database.

use rusqlite::{Connection, Error as SqliteError, OpenFlags, TransactionBehavior};

const LATEST_SCHEMA_VERSION: u32 = 2;

const MIGRATION_1: &str = r#"
CREATE TABLE IF NOT EXISTS repository_meta (
    key TEXT PRIMARY KEY NOT NULL,
    value TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS actors (
    actor_id TEXT PRIMARY KEY NOT NULL,
    authority_class TEXT NOT NULL,
    profile_name TEXT NOT NULL,
    profile_hash TEXT NOT NULL,
    active INTEGER NOT NULL DEFAULT 1 CHECK (active IN (0, 1))
);

CREATE TABLE IF NOT EXISTS assignments (
    assignment_id TEXT PRIMARY KEY NOT NULL,
    bead_id TEXT NOT NULL,
    bead_content_hash TEXT NOT NULL,
    scope_map_json TEXT NOT NULL,
    worker_json TEXT NOT NULL,
    decision_actor_json TEXT NOT NULL,
    edit_scope_json TEXT NOT NULL,
    acceptance_json TEXT NOT NULL,
    attempt_policy_json TEXT NOT NULL,
    declared_base TEXT NOT NULL,
    created_seq INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS attempts (
    attempt_id TEXT PRIMARY KEY NOT NULL,
    assignment_id TEXT NOT NULL REFERENCES assignments(assignment_id),
    fencing_token INTEGER NOT NULL,
    lease_expires_at INTEGER NOT NULL,
    state TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS signals (
    signal_id TEXT PRIMARY KEY NOT NULL,
    attempt_id TEXT,
    sender_json TEXT NOT NULL,
    subject_json TEXT NOT NULL,
    body_json TEXT NOT NULL,
    committed_seq INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS evidence (
    operation_id TEXT PRIMARY KEY NOT NULL,
    attempt_id TEXT NOT NULL,
    evidence_json TEXT NOT NULL,
    committed_seq INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS handoffs (
    handoff_id TEXT PRIMARY KEY NOT NULL,
    attempt_id TEXT NOT NULL,
    handoff_json TEXT NOT NULL,
    committed_seq INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS decisions (
    operation_id TEXT PRIMARY KEY NOT NULL,
    assignment_id TEXT NOT NULL,
    decision_json TEXT NOT NULL,
    committed_seq INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS application_attempts (
    operation_id TEXT PRIMARY KEY NOT NULL,
    target_operation_id TEXT NOT NULL,
    outcome_json TEXT NOT NULL,
    committed_seq INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS application_receipts (
    target_operation_id TEXT PRIMARY KEY NOT NULL,
    attempt_operation_id TEXT NOT NULL,
    after_revision TEXT NOT NULL,
    committed_seq INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS envelopes (
    launch_subject_key TEXT PRIMARY KEY NOT NULL,
    envelope_json TEXT NOT NULL,
    committed_seq INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS runtime_handles (
    launch_subject_key TEXT PRIMARY KEY NOT NULL,
    handle TEXT NOT NULL,
    committed_seq INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS idempotency (
    operation_key TEXT PRIMARY KEY NOT NULL,
    request_hash TEXT NOT NULL,
    result_json TEXT NOT NULL,
    committed_seq INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS audit_events (
    event_seq INTEGER PRIMARY KEY NOT NULL,
    operation_id TEXT,
    event_kind TEXT NOT NULL,
    event_json TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_signals_attempt_seq
    ON signals(attempt_id, committed_seq);
CREATE INDEX IF NOT EXISTS idx_audit_operation_seq
    ON audit_events(operation_id, event_seq);
"#;

// These objects were split into a second migration so databases that already
// applied v1 receive the identity, profile, lease, and credential tables too.
const MIGRATION_2: &str = r#"
CREATE TABLE IF NOT EXISTS repository_identity (
    repository_id TEXT PRIMARY KEY NOT NULL,
    created_at INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS profile_snapshots (
    profile_name TEXT PRIMARY KEY NOT NULL,
    authority_class TEXT NOT NULL,
    profile_hash TEXT NOT NULL,
    grants_json TEXT NOT NULL,
    active_actor_id TEXT
);

CREATE TABLE IF NOT EXISTS leases (
    attempt_id TEXT PRIMARY KEY NOT NULL REFERENCES attempts(attempt_id),
    fencing_token INTEGER NOT NULL,
    expires_at INTEGER NOT NULL,
    updated_seq INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS credentials (
    credential_id TEXT PRIMARY KEY NOT NULL,
    digest TEXT NOT NULL,
    actor_id TEXT NOT NULL,
    authority_class TEXT NOT NULL,
    profile_name TEXT NOT NULL,
    profile_hash TEXT NOT NULL,
    launch_subject_key TEXT NOT NULL UNIQUE,
    revoked INTEGER NOT NULL DEFAULT 0 CHECK (revoked IN (0, 1)),
    created_seq INTEGER NOT NULL
);
"#;

#[derive(Debug, thiserror::Error)]
pub enum MigrationError {
    #[error("sqlite error: {0}")]
    Sqlite(#[from] SqliteError),
    #[error("database schema version {found} is newer than supported {supported}")]
    IncompatibleVersion { found: u32, supported: u32 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MigrationReport {
    pub from_version: u32,
    pub to_version: u32,
}

pub const fn latest_schema_version() -> u32 {
    LATEST_SCHEMA_VERSION
}

/// Open a private SQLite connection with WAL and a bounded busy timeout, then
/// apply every missing migration in one transaction per version.
pub fn apply_migrations(
    path: impl AsRef<std::path::Path>,
) -> Result<MigrationReport, MigrationError> {
    let mut connection = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_WRITE
            | OpenFlags::SQLITE_OPEN_CREATE
            | OpenFlags::SQLITE_OPEN_URI,
    )?;
    connection.busy_timeout(std::time::Duration::from_secs(5))?;
    connection.pragma_update(None, "foreign_keys", true)?;
    let from_version =
        connection.query_row("PRAGMA user_version", [], |row| row.get::<_, u32>(0))?;
    if from_version > LATEST_SCHEMA_VERSION {
        return Err(MigrationError::IncompatibleVersion {
            found: from_version,
            supported: LATEST_SCHEMA_VERSION,
        });
    }
    connection.pragma_update(None, "journal_mode", "WAL")?;
    if from_version < 1 {
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        transaction.execute_batch(MIGRATION_1)?;
        transaction.pragma_update(None, "user_version", 1_u32)?;
        transaction.commit()?;
    }
    if from_version < 2 {
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        transaction.execute_batch(MIGRATION_2)?;
        transaction.pragma_update(None, "user_version", 2_u32)?;
        transaction.commit()?;
    }
    Ok(MigrationReport {
        from_version,
        to_version: LATEST_SCHEMA_VERSION,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temporary_db() -> PathBuf {
        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock before epoch")
            .as_nanos();
        std::env::temp_dir().join(format!("abacus-state-migration-{suffix}.sqlite3"))
    }

    #[test]
    fn fresh_database_migrates_once_and_is_idempotent() {
        let path = temporary_db();
        let first = apply_migrations(&path).expect("first migration");
        assert_eq!(first.from_version, 0);
        assert_eq!(first.to_version, 2);
        let second = apply_migrations(&path).expect("second migration");
        assert_eq!(second.from_version, 2);
        assert_eq!(second.to_version, 2);
        let connection = Connection::open(&path).expect("open migrated database");
        let journal: String = connection
            .query_row("PRAGMA journal_mode", [], |row| row.get(0))
            .expect("journal mode");
        assert_eq!(journal.to_ascii_lowercase(), "wal");
        let foreign_keys: i64 = connection
            .query_row("PRAGMA foreign_keys", [], |row| row.get(0))
            .expect("foreign key mode");
        assert_eq!(foreign_keys, 1);
        for table in [
            "repository_meta",
            "repository_identity",
            "profile_snapshots",
            "actors",
            "assignments",
            "attempts",
            "leases",
            "credentials",
            "signals",
            "evidence",
            "handoffs",
            "decisions",
            "application_attempts",
            "application_receipts",
            "envelopes",
            "runtime_handles",
            "idempotency",
            "audit_events",
        ] {
            let present: i64 = connection
                .query_row(
                    "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = ?1",
                    [table],
                    |row| row.get(0),
                )
                .expect("table lookup");
            assert_eq!(present, 1, "missing migrated table {table}");
        }
        drop(connection);
        std::fs::remove_file(path).expect("remove temporary database");
    }

    #[test]
    fn newer_schema_fails_closed_without_mutating_it() {
        let path = temporary_db();
        let connection = Connection::open(&path).expect("create database");
        connection
            .pragma_update(None, "journal_mode", "DELETE")
            .expect("set baseline journal mode");
        connection
            .pragma_update(None, "user_version", LATEST_SCHEMA_VERSION + 1)
            .expect("set future version");
        drop(connection);
        assert!(matches!(
            apply_migrations(&path),
            Err(MigrationError::IncompatibleVersion {
                found: 3,
                supported: 2
            })
        ));
        let connection = Connection::open(&path).expect("reopen database");
        let version: u32 = connection
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .expect("read version");
        assert_eq!(version, 3);
        let journal: String = connection
            .query_row("PRAGMA journal_mode", [], |row| row.get(0))
            .expect("read journal mode");
        assert_eq!(journal.to_ascii_lowercase(), "delete");
        drop(connection);
        std::fs::remove_file(path).expect("remove temporary database");
    }

    #[test]
    fn v1_database_receives_the_incremental_v2_objects() {
        let path = temporary_db();
        let connection = Connection::open(&path).expect("create database");
        connection.execute_batch(MIGRATION_1).expect("apply v1");
        connection
            .pragma_update(None, "user_version", 1_u32)
            .expect("mark v1");
        drop(connection);

        let report = apply_migrations(&path).expect("upgrade v1");
        assert_eq!(report.from_version, 1);
        assert_eq!(report.to_version, 2);
        let connection = Connection::open(&path).expect("reopen upgraded database");
        let credential_table: String = connection
            .query_row(
                "SELECT name FROM sqlite_master WHERE type = 'table' AND name = 'credentials'",
                [],
                |row| row.get(0),
            )
            .expect("v2 credential table");
        assert_eq!(credential_table, "credentials");
        drop(connection);
        std::fs::remove_file(path).expect("remove temporary database");
    }
}
