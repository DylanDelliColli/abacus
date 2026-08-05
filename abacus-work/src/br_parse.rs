//! Parse layer for pinned `br` JSON output (`omw.2`).
//!
//! Every function here turns recorded provider output shapes — proven
//! by `abacus-work/fixtures/br-v0.1.45` — into typed raw facts. No
//! normalization happens here: scope labels stay raw strings, and
//! status mapping preserves provider distinctions (tombstone, deferred)
//! for the provider layer to interpret under the seam contract.

use abacus_core::ContentHash;
use abacus_core::ports::{ObservedCloseReason, WorkError, WorkRevision};
use serde::Deserialize;

/// The canonical provider rendering of the two curated close reasons.
/// Written by the adapter on `close`; anything else observed maps to
/// `UnrecognizedProviderReason` so a foreign close is never adopted.
pub const CLOSE_REASON_ACCEPTED: &str = "abacus:accepted-handoff";
pub const CLOSE_REASON_CANCELLED: &str = "abacus:cancelled-obsolete";

/// br's structured error envelope (`{"error": {...}}`), fixture-proven
/// across ISSUE_NOT_FOUND, DATABASE_ERROR, and CONFIG_ERROR cases.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct BrErrorEnvelope {
    pub code: String,
    pub message: String,
    pub retryable: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
struct BrErrorDocument {
    error: BrErrorEnvelope,
}

/// One issue record as br emits it in `--json` arrays. Tolerant of
/// unknown fields: the pinned binary governs the shape, and fixtures
/// prove the fields this layer reads.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct BrIssueDto {
    pub id: String,
    pub status: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub priority: Option<u8>,
    #[serde(default)]
    pub labels: Vec<String>,
    #[serde(default)]
    pub close_reason: Option<String>,
    #[serde(default)]
    pub deleted_at: Option<String>,
}

/// Provider-faithful issue status before seam normalization. Tombstone
/// and Deferred are preserved distinctly: mapping them into the closed
/// `WorkStatus` set is a provider-layer decision, not a parse fact.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BrIssueStatus {
    Open,
    InProgress,
    Closed(ObservedCloseReason),
    Tombstone,
    Deferred,
}

/// `br sync --status --json` output (revision-bracketing fixture).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct BrSyncStatus {
    pub dirty_count: u64,
    pub jsonl_content_hash: String,
    pub jsonl_newer: bool,
    pub db_newer: bool,
}

/// A revision read is only meaningful when the JSONL hash speaks for
/// the database; otherwise the caller must flush or refuse, never use a
/// stale hash as the current revision.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RevisionReading {
    Current(WorkRevision),
    Unbracketable,
}

/// Try to read stdout as br's structured error document.
pub fn parse_error_document(stdout: &str) -> Option<BrErrorEnvelope> {
    serde_json::from_str::<BrErrorDocument>(stdout)
        .ok()
        .map(|document| document.error)
}

/// Map a structured provider error onto the closed `WorkError` set.
///
/// Unrecognized codes and unrecognized `DATABASE_ERROR` subtypes map to
/// `Corrupt` deliberately: fail loud and demand attention rather than
/// invite a retry against an unknown failure.
pub fn classify_error(error: &BrErrorEnvelope) -> WorkError {
    match error.code.as_str() {
        "ISSUE_NOT_FOUND" => WorkError::NotFound,
        "DATABASE_ERROR" if error.message.contains("database is busy") => WorkError::Busy,
        _ => WorkError::Corrupt,
    }
}

/// Parse a `--json` issue array (`show`, `ready`, `update` outputs).
pub fn parse_issue_array(stdout: &str) -> Result<Vec<BrIssueDto>, WorkError> {
    serde_json::from_str(stdout).map_err(|_| WorkError::MalformedOutput)
}

/// Map the provider status string, preserving provider distinctions.
pub fn issue_status(issue: &BrIssueDto) -> Result<BrIssueStatus, WorkError> {
    match issue.status.as_str() {
        "open" => Ok(BrIssueStatus::Open),
        "in_progress" => Ok(BrIssueStatus::InProgress),
        "deferred" => Ok(BrIssueStatus::Deferred),
        "tombstone" => Ok(BrIssueStatus::Tombstone),
        "closed" => {
            let reason = match issue.close_reason.as_deref() {
                Some(CLOSE_REASON_ACCEPTED) => ObservedCloseReason::AcceptedHandoff,
                Some(CLOSE_REASON_CANCELLED) => ObservedCloseReason::CancelledObsolete,
                _ => ObservedCloseReason::UnrecognizedProviderReason,
            };
            Ok(BrIssueStatus::Closed(reason))
        }
        _ => Err(WorkError::MalformedOutput),
    }
}

/// Parse `sync --status --json` output.
pub fn parse_sync_status(stdout: &str) -> Result<BrSyncStatus, WorkError> {
    serde_json::from_str(stdout).map_err(|_| WorkError::MalformedOutput)
}

/// Read the current graph revision from a sync status, refusing stale
/// hashes. A malformed hash is `MalformedOutput`: the 64-hex domain
/// type is the contract, not a suggestion.
pub fn revision_reading(status: &BrSyncStatus) -> Result<RevisionReading, WorkError> {
    if status.dirty_count > 0 || status.db_newer {
        return Ok(RevisionReading::Unbracketable);
    }
    let hash =
        ContentHash::new(&status.jsonl_content_hash).map_err(|_| WorkError::MalformedOutput)?;
    Ok(RevisionReading::Current(WorkRevision(hash)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    const DELETION_FIXTURE: &str = include_str!("../fixtures/br-v0.1.45/deletion-tombstone.json");
    const MUTATIONS_FIXTURE: &str = include_str!("../fixtures/br-v0.1.45/status-mutations.json");
    const REVISION_FIXTURE: &str = include_str!("../fixtures/br-v0.1.45/revision-bracketing.json");
    const BUSY_FIXTURE: &str = include_str!("../fixtures/br-v0.1.45/database-busy.json");
    const CORRUPT_FIXTURE: &str = include_str!("../fixtures/br-v0.1.45/database-corrupt.json");
    const CONFLICT_FIXTURE: &str = include_str!("../fixtures/br-v0.1.45/sync-conflict.json");

    /// Re-serialize a recorded stdout object from a fixture file, so
    /// parsers face exactly the recorded provider facts.
    fn recorded(fixture: &str, pointer: &str) -> String {
        let document: Value = serde_json::from_str(fixture).expect("fixture file is valid JSON");
        document
            .pointer(pointer)
            .unwrap_or_else(|| panic!("fixture pointer {pointer} exists"))
            .to_string()
    }

    #[test]
    fn recorded_not_found_error_maps_to_not_found() {
        let stdout = recorded(DELETION_FIXTURE, "/commands/2/stdout");
        let error = parse_error_document(&stdout).expect("recorded error envelope parses");
        assert_eq!(error.code, "ISSUE_NOT_FOUND");
        assert!(!error.retryable);
        assert_eq!(classify_error(&error), WorkError::NotFound);
    }

    #[test]
    fn recorded_busy_and_corrupt_database_errors_map_distinctly() {
        let busy = recorded(BUSY_FIXTURE, "/process/stdout");
        let busy_error = parse_error_document(&busy).expect("busy envelope parses");
        assert_eq!(classify_error(&busy_error), WorkError::Busy);

        let corrupt = recorded(CORRUPT_FIXTURE, "/without_recovery_source/process/stdout");
        let corrupt_error = parse_error_document(&corrupt).expect("corrupt envelope parses");
        assert_eq!(corrupt_error.code, "DATABASE_ERROR");
        assert_eq!(classify_error(&corrupt_error), WorkError::Corrupt);
    }

    #[test]
    fn recorded_sync_guard_error_fails_loud_as_corrupt() {
        let stdout = recorded(CONFLICT_FIXTURE, "/process/stdout");
        let error = parse_error_document(&stdout).expect("config-error envelope parses");
        assert_eq!(error.code, "CONFIG_ERROR");
        assert_eq!(classify_error(&error), WorkError::Corrupt);
    }

    #[test]
    fn unknown_structured_codes_fail_loud_as_corrupt() {
        let error = BrErrorEnvelope {
            code: "FUTURE_ERROR".to_owned(),
            message: "something new".to_owned(),
            retryable: true,
        };
        assert_eq!(classify_error(&error), WorkError::Corrupt);
    }

    #[test]
    fn recorded_update_output_parses_with_raw_status() {
        let stdout = recorded(MUTATIONS_FIXTURE, "/commands/0/stdout");
        let issues = parse_issue_array(&stdout).expect("recorded update array parses");
        assert_eq!(issues.len(), 1);
        assert_eq!(issues[0].id, "br-fixture-capture-98m");
        assert_eq!(issue_status(&issues[0]), Ok(BrIssueStatus::InProgress));
    }

    #[test]
    fn recorded_free_text_close_reason_is_unrecognized_not_adopted() {
        let stdout = recorded(MUTATIONS_FIXTURE, "/commands/1/stdout");
        let issues = parse_issue_array(&stdout).expect("recorded close array parses");
        assert_eq!(
            issue_status(&issues[0]),
            Ok(BrIssueStatus::Closed(
                ObservedCloseReason::UnrecognizedProviderReason
            ))
        );
    }

    #[test]
    fn canonical_close_reasons_round_trip_to_curated_variants() {
        // The canonical strings are the adapter's own rendering; no
        // fixture carries them until the adapter writes its first
        // close. The mapping is exact-match by contract.
        for (rendered, expected) in [
            (CLOSE_REASON_ACCEPTED, ObservedCloseReason::AcceptedHandoff),
            (
                CLOSE_REASON_CANCELLED,
                ObservedCloseReason::CancelledObsolete,
            ),
        ] {
            let issue = BrIssueDto {
                id: "abacus-x".to_owned(),
                status: "closed".to_owned(),
                title: String::new(),
                priority: None,
                labels: Vec::new(),
                close_reason: Some(rendered.to_owned()),
                deleted_at: None,
            };
            assert_eq!(issue_status(&issue), Ok(BrIssueStatus::Closed(expected)));
        }
    }

    #[test]
    fn tombstone_and_deferred_stay_provider_faithful() {
        let mut issue = BrIssueDto {
            id: "abacus-x".to_owned(),
            status: "tombstone".to_owned(),
            title: String::new(),
            priority: None,
            labels: Vec::new(),
            close_reason: None,
            deleted_at: Some("2026-08-05T13:48:59.521881719Z".to_owned()),
        };
        assert_eq!(issue_status(&issue), Ok(BrIssueStatus::Tombstone));
        issue.status = "deferred".to_owned();
        assert_eq!(issue_status(&issue), Ok(BrIssueStatus::Deferred));
        issue.status = "surprising".to_owned();
        assert_eq!(issue_status(&issue), Err(WorkError::MalformedOutput));
    }

    #[test]
    fn non_json_stdout_is_malformed_output() {
        assert_eq!(
            parse_issue_array("panic: something exploded"),
            Err(WorkError::MalformedOutput)
        );
        assert_eq!(parse_error_document("not json"), None);
        assert_eq!(
            parse_sync_status("<html>proxy error</html>"),
            Err(WorkError::MalformedOutput)
        );
    }

    #[test]
    fn recorded_sync_status_yields_the_recorded_revision() {
        let stdout = recorded(REVISION_FIXTURE, "/commands/0/stdout");
        let status = parse_sync_status(&stdout).expect("recorded sync status parses");
        assert_eq!(status.dirty_count, 0);
        assert!(!status.db_newer);
        let reading = revision_reading(&status).expect("recorded hash is 64-hex");
        let RevisionReading::Current(revision) = reading else {
            panic!("clean recorded status must yield a current revision");
        };
        assert_eq!(
            revision.0.as_str(),
            "1faf9ae20cc759d02fface7b63bc9bbb412bd28af99f7d604ea9c6ab303eaa48"
        );
    }

    #[test]
    fn dirty_or_db_newer_sync_status_is_unbracketable_never_stale() {
        let clean = BrSyncStatus {
            dirty_count: 0,
            jsonl_content_hash: "0d68cacaedf73f96d6eef77c164c0b00d1891e703c1da60591aaee1d6f29249e"
                .to_owned(),
            jsonl_newer: false,
            db_newer: false,
        };
        let dirty = BrSyncStatus {
            dirty_count: 3,
            ..clean.clone()
        };
        assert_eq!(revision_reading(&dirty), Ok(RevisionReading::Unbracketable));
        let db_newer = BrSyncStatus {
            db_newer: true,
            ..clean.clone()
        };
        assert_eq!(
            revision_reading(&db_newer),
            Ok(RevisionReading::Unbracketable)
        );
        let bad_hash = BrSyncStatus {
            jsonl_content_hash: "not-a-hash".to_owned(),
            ..clean
        };
        assert_eq!(revision_reading(&bad_hash), Err(WorkError::MalformedOutput));
    }
}
