//! SQLite implementation of the same portable state contract as the
//! canonical in-memory fake.

use abacus_core::Timestamp;
use abacus_state::contract::{
    RestartStateContractHarness, StateContractHarness, run_workflow_state_restart_suite,
    run_workflow_state_suite,
};
use abacus_state::{ManualClock, SqliteState, SqliteStateOpenError};
use rusqlite::Connection;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_DIRECTORY: AtomicU64 = AtomicU64::new(0);

struct SqliteHarness {
    state: Option<SqliteState<ManualClock>>,
    clock: ManualClock,
    directory: PathBuf,
}

impl Drop for SqliteHarness {
    fn drop(&mut self) {
        // Close SQLite before removing its private temporary directory.
        self.state.take();
        std::fs::remove_dir_all(&self.directory).expect("remove temporary SQLite directory");
    }
}

impl StateContractHarness for SqliteHarness {
    fn port(&self) -> &dyn abacus_core::ports::WorkflowStatePort {
        self.state.as_ref().expect("SQLite state is open")
    }

    fn set_now(&self, now: Timestamp) {
        self.clock.set(now);
    }
}

impl RestartStateContractHarness for SqliteHarness {
    fn restart(&mut self) {
        self.state.take();
        self.state = Some(
            SqliteState::open(self.directory.join("state.sqlite3"), self.clock.clone())
                .expect("reopen SQLite state"),
        );
    }
}

fn build(now: Timestamp) -> SqliteHarness {
    let clock = ManualClock::new(now);
    let directory = std::env::temp_dir().join(format!(
        "abacus-sqlite-contract-{}-{}",
        std::process::id(),
        NEXT_DIRECTORY.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir(&directory).expect("create temporary SQLite directory");
    let state = SqliteState::open(directory.join("state.sqlite3"), clock.clone())
        .expect("open SQLite state");
    SqliteHarness {
        state: Some(state),
        clock,
        directory,
    }
}

#[test]
fn sqlite_state_passes_the_portable_contract() {
    run_workflow_state_suite(build);
}

#[test]
fn sqlite_state_recovers_behavior_from_relational_rows() {
    run_workflow_state_restart_suite(build);
}

#[test]
fn unsupported_row_representation_fails_open_loudly() {
    let mut harness = build(Timestamp(1));
    harness.state.take();
    let path = harness.directory.join("state.sqlite3");
    let connection = Connection::open(&path).expect("open migrated database directly");
    connection
        .execute(
            "INSERT INTO workflow_meta(singleton, head_seq, bootstrap_complete)
             VALUES (1, 0, 0)",
            [],
        )
        .expect("insert state sentinel");
    connection
        .execute(
            "INSERT INTO credential_bindings(
                 owner_key, credential_id, revoked, record_version, record_json
             ) VALUES ('attempt:att-version', 'cred-version', 0, 999, '{}')",
            [],
        )
        .expect("insert unsupported row");
    drop(connection);

    assert!(matches!(
        SqliteState::open(&path, ManualClock::new(Timestamp(1))),
        Err(SqliteStateOpenError::Stored(message))
            if message.contains("version 999 is unsupported")
    ));
}
