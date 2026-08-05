//! `WorkProvider` over the pinned `br` process (`omw.2`, read paths).
//!
//! Raw facts only: labels cross untouched, statuses are mapped from
//! the provider-faithful parse layer, and normalization stays in
//! [`crate::facade::WorkFacade`]. The graph revision is read BEFORE the
//! bead facts on purpose: if the graph moves between the two reads, the
//! caller's expected-revision precondition fails loudly
//! (`RevisionConflict`) instead of pairing a fresh revision with stale
//! status facts — the pairing that could make read-before-write
//! idempotency checks mis-assess and re-issue a landed mutation.
//!
//! The bead content hash is produced by an injected digest over
//! preimage v0 — the compact re-serialization of the provider's issue
//! object. Injection keeps the digest primitive out of this crate:
//! `abacus-omw.8` decides between a vetted SHA-256 dependency and a
//! composition-root digest port, and either satisfies this seam.

use std::cell::Cell;

use abacus_core::ContentHash;
use abacus_core::ports::WorkError;
use abacus_core::ports::WorkStatus;
use serde_json::Value;

use crate::adapter::{ProviderMutation, RawBeadSnapshot, RawBeadStatusView, TargetStatus};
use crate::br_parse::{
    BrIssueDto, BrIssueStatus, RevisionReading, classify_error, issue_status, parse_error_document,
    parse_sync_status, revision_reading,
};
use crate::br_process::{BrObservation, BrRequest, BrRunner};
use crate::id_seam::{from_provider, to_provider};
use abacus_core::BeadId;
use abacus_core::ports::WorkRevision;

/// Read-path provider facade over one pinned `br` binary.
pub struct BrWorkProvider<R, D> {
    runner: R,
    pinned_version_line: String,
    digest: D,
    pin_verified: Cell<bool>,
}

impl<R, D> BrWorkProvider<R, D>
where
    R: BrRunner,
    D: Fn(&str) -> ContentHash,
{
    pub fn new(runner: R, pinned_version_line: impl Into<String>, digest: D) -> Self {
        Self {
            runner,
            pinned_version_line: pinned_version_line.into(),
            digest,
            pin_verified: Cell::new(false),
        }
    }

    /// One cached identity probe per provider instance, before the
    /// first graph command of any kind. Version drift fails closed as
    /// `Incompatible` and no graph command is ever issued.
    fn ensure_pinned(&self) -> Result<(), WorkError> {
        if self.pin_verified.get() {
            return Ok(());
        }
        crate::br_process::verify_pinned_identity(&self.runner, &self.pinned_version_line)?;
        self.pin_verified.set(true);
        Ok(())
    }

    /// Run one read-path invocation. Transport failure on a read is
    /// `ProviderUnavailable`: nothing was mutated, retry later.
    fn read(&self, args: &[&str]) -> Result<BrObservation, WorkError> {
        self.runner
            .run(&BrRequest::new(args.iter().copied()))
            .map_err(|_| WorkError::ProviderUnavailable)
    }

    /// Current graph revision from `sync --status`. An unbracketable
    /// hash (unexported mutations) is `Busy`: the read path never
    /// flushes implicitly and never serves a stale hash.
    fn current_revision(&self) -> Result<WorkRevision, WorkError> {
        let observation = self.read(&["sync", "--status", "--json"])?;
        if observation.exit_code != 0 {
            return Err(match parse_error_document(&observation.stdout) {
                Some(error) => classify_error(&error),
                None => WorkError::MalformedOutput,
            });
        }
        let status = parse_sync_status(&observation.stdout)?;
        match revision_reading(&status)? {
            RevisionReading::Current(revision) => Ok(revision),
            RevisionReading::Unbracketable => Err(WorkError::Busy),
        }
    }

    /// One `show` invocation mapped to raw facts. `expected` pins the
    /// answer to the asked-about bead; a foreign or mismatched answer
    /// is `MalformedOutput`.
    fn show_facts(
        &self,
        provider_id: &str,
        expected: &BeadId,
    ) -> Result<(RawBeadSnapshot, BrIssueStatus), WorkError> {
        let observation = self.read(&["show", provider_id, "--json"])?;
        if observation.exit_code != 0 {
            return Err(match parse_error_document(&observation.stdout) {
                Some(error) => classify_error(&error),
                None => WorkError::MalformedOutput,
            });
        }
        let elements: Vec<Value> =
            serde_json::from_str(&observation.stdout).map_err(|_| WorkError::MalformedOutput)?;
        let [element] = elements.as_slice() else {
            return Err(WorkError::MalformedOutput);
        };
        // Preimage v0: the compact re-serialization of the provider's
        // issue object. Any bead-field change changes it; the pinned
        // emitter keeps field order stable.
        let preimage = element.to_string();
        let issue: BrIssueDto =
            serde_json::from_value(element.clone()).map_err(|_| WorkError::MalformedOutput)?;
        let answered = from_provider(&issue.id).map_err(|_| WorkError::MalformedOutput)?;
        if &answered != expected {
            return Err(WorkError::MalformedOutput);
        }
        let status = issue_status(&issue)?;
        let priority = issue.priority.ok_or(WorkError::MalformedOutput)?;
        Ok((
            RawBeadSnapshot {
                id: answered,
                content_hash: (self.digest)(&preimage),
                raw_labels: issue.labels.clone(),
                priority,
            },
            status,
        ))
    }

    /// Inspect one bead: graph revision first (see module doc), then
    /// the bead facts, mapped without normalization.
    pub fn inspect_raw(&self, id: &BeadId) -> Result<RawBeadStatusView, WorkError> {
        self.ensure_pinned()?;
        let revision = self.current_revision()?;
        let provider_id = to_provider(id);
        let (snapshot, provider_status) = self.show_facts(provider_id.as_str(), id)?;
        let status = match provider_status {
            // A deleted bead no longer exists as work; the typed
            // `Missing` anomaly is derived from this by the port.
            BrIssueStatus::Tombstone => return Err(WorkError::NotFound),
            // Deferral is provider scheduling, not a distinct work
            // status: a deferred bead is open work that `ready`
            // excludes. Flagged for cross-review at the omw.2 gate.
            BrIssueStatus::Deferred | BrIssueStatus::Open => WorkStatus::Open,
            BrIssueStatus::InProgress => WorkStatus::InProgress,
            BrIssueStatus::Closed(observed_reason) => WorkStatus::Closed { observed_reason },
        };
        Ok(RawBeadStatusView {
            snapshot,
            status,
            revision,
        })
    }

    /// Ready work, revision-bracketed: hash → `ready --limit 0` → one
    /// `show` per id (ready output carries no labels) → hash again. A
    /// mismatched closing hash, or a bead that vanishes mid-batch, is
    /// `RevisionConflict` — never a silently mixed-generation batch.
    /// `--limit 0` is load-bearing: the provider default silently
    /// truncates at 20.
    pub fn ready_raw(&self) -> Result<(WorkRevision, Vec<RawBeadSnapshot>), WorkError> {
        self.ensure_pinned()?;
        let opening = self.current_revision()?;
        let observation = self.read(&["ready", "--limit", "0", "--json"])?;
        if observation.exit_code != 0 {
            return Err(match parse_error_document(&observation.stdout) {
                Some(error) => classify_error(&error),
                None => WorkError::MalformedOutput,
            });
        }
        let listed: Vec<BrIssueDto> =
            serde_json::from_str(&observation.stdout).map_err(|_| WorkError::MalformedOutput)?;
        let mut snapshots = Vec::with_capacity(listed.len());
        for issue in &listed {
            let expected = from_provider(&issue.id).map_err(|_| WorkError::MalformedOutput)?;
            let (snapshot, _) = match self.show_facts(&issue.id, &expected) {
                // Listed as ready moments ago, unknown now: the graph
                // moved under the batch. Report the conflict; the
                // caller re-reads a coherent generation.
                Err(WorkError::NotFound) => return Err(WorkError::RevisionConflict),
                other => other?,
            };
            snapshots.push(snapshot);
        }
        let closing = self.current_revision()?;
        if closing != opening {
            return Err(WorkError::RevisionConflict);
        }
        Ok((opening, snapshots))
    }

    fn mutation_args(
        provider_id: &crate::id_seam::ProviderBeadId,
        target: &TargetStatus,
        operation: &abacus_core::OperationId,
    ) -> Vec<String> {
        let actor = format!("--actor={}", operation.as_str());
        match target {
            TargetStatus::InProgress => vec![
                "update".to_owned(),
                provider_id.as_str().to_owned(),
                "--status=in_progress".to_owned(),
                actor,
                "--json".to_owned(),
            ],
            TargetStatus::Closed(reason) => {
                let rendered = match reason {
                    abacus_core::ports::CloseReason::AcceptedHandoff => {
                        crate::br_parse::CLOSE_REASON_ACCEPTED
                    }
                    abacus_core::ports::CloseReason::CancelledObsolete => {
                        crate::br_parse::CLOSE_REASON_CANCELLED
                    }
                };
                vec![
                    "close".to_owned(),
                    provider_id.as_str().to_owned(),
                    format!("--reason={rendered}"),
                    actor,
                    "--json".to_owned(),
                ]
            }
        }
    }

    /// Issue one decision-gated mutation. The `Err` contract is
    /// load-bearing: `Err` asserts the mutation DEFINITIVELY did not
    /// take effect, so it is returned only before the command is issued
    /// (pin gate, unbracketable revision, spawn failure) or on a
    /// structured provider refusal whose fixture verified
    /// `mutation: not_applied`. Every uncertain shape after issuing —
    /// deadline, output bound, unreadable output, a failed after-hash
    /// read — is `Ok(Ambiguous)` so the facade reconciles by
    /// re-inspection instead of inviting a blind retry.
    fn set_status_raw(
        &self,
        id: &BeadId,
        target: TargetStatus,
        operation: &abacus_core::OperationId,
    ) -> Result<ProviderMutation, WorkError> {
        self.ensure_pinned()?;
        let before = self.current_revision()?;
        let provider_id = to_provider(id);
        let args = Self::mutation_args(&provider_id, &target, operation);
        let request = BrRequest { args };
        let observation = match self.runner.run(&request) {
            Ok(observation) => observation,
            // The process never started: nothing can have taken effect.
            Err(crate::br_process::BrRunError::Spawn) => {
                return Err(WorkError::ProviderUnavailable);
            }
            // The write may be in flight: outcome unknown.
            Err(_) => return Ok(ProviderMutation::Ambiguous),
        };
        if observation.exit_code != 0 {
            return match parse_error_document(&observation.stdout) {
                // Structured refusals are fixture-verified not-applied.
                Some(error) => Err(classify_error(&error)),
                // Unreadable failure AFTER issuing: never claim
                // definitively-not-applied on faith.
                None => Ok(ProviderMutation::Ambiguous),
            };
        }
        let confirmed = crate::br_parse::parse_issue_array(&observation.stdout)
            .ok()
            .and_then(|issues| match issues.as_slice() {
                [only] if from_provider(&only.id).as_ref() == Ok(id) => {
                    Some(format!("{}: {}", only.id, only.status))
                }
                _ => None,
            });
        let Some(summary) = confirmed else {
            // Exit 0 with unconfirmable output: the mutation very
            // likely landed; report unknown and let reconciliation
            // observe the facts.
            return Ok(ProviderMutation::Ambiguous);
        };
        let Ok(after) = self.current_revision() else {
            // The mutation landed but its after-revision is unreadable;
            // Applied without honest before/after would be a lie.
            return Ok(ProviderMutation::Ambiguous);
        };
        Ok(ProviderMutation::Applied {
            before,
            after,
            summary,
        })
    }
}

impl<R, D> crate::adapter::WorkProvider for BrWorkProvider<R, D>
where
    R: BrRunner,
    D: Fn(&str) -> ContentHash,
{
    fn ready(&self) -> Result<(WorkRevision, Vec<RawBeadSnapshot>), WorkError> {
        self.ready_raw()
    }

    fn inspect(&self, id: &BeadId) -> Result<RawBeadStatusView, WorkError> {
        self.inspect_raw(id)
    }

    fn set_status(
        &self,
        id: &BeadId,
        target: TargetStatus,
        operation: &abacus_core::OperationId,
    ) -> Result<ProviderMutation, WorkError> {
        self.set_status_raw(id, target, operation)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::br_process::{BrRunError, ScriptedBrRunner};

    const PINNED: &str = "br 0.1.45";
    const HASH: &str = "1faf9ae20cc759d02fface7b63bc9bbb412bd28af99f7d604ea9c6ab303eaa48";

    /// Deterministic test stand-in — NOT a cryptographic digest. The
    /// production primitive arrives via `abacus-omw.8`.
    fn test_digest(preimage: &str) -> ContentHash {
        let mut acc: u64 = 0xcbf2_9ce4_8422_2325;
        for byte in preimage.bytes() {
            acc = acc.wrapping_mul(0x0000_0100_0000_01b3) ^ u64::from(byte);
        }
        ContentHash::new(&format!("{acc:016x}").repeat(4)).expect("64 hex")
    }

    fn ok(stdout: &str) -> Result<BrObservation, BrRunError> {
        Ok(BrObservation {
            exit_code: 0,
            stdout: stdout.to_owned(),
            stderr: String::new(),
        })
    }

    fn exit(code: i32, stdout: &str) -> Result<BrObservation, BrRunError> {
        Ok(BrObservation {
            exit_code: code,
            stdout: stdout.to_owned(),
            stderr: String::new(),
        })
    }

    fn version_step() -> (BrRequest, Result<BrObservation, BrRunError>) {
        (BrRequest::new(["--version"]), ok("br 0.1.45\n"))
    }

    fn sync_step(dirty: u64) -> (BrRequest, Result<BrObservation, BrRunError>) {
        let stdout = format!(
            "{{\"dirty_count\":{dirty},\"jsonl_content_hash\":\"{HASH}\",\"jsonl_newer\":false,\"db_newer\":false}}"
        );
        (BrRequest::new(["sync", "--status", "--json"]), ok(&stdout))
    }

    fn show_request(provider_id: &str) -> BrRequest {
        BrRequest::new(["show", provider_id, "--json"])
    }

    fn provider(
        steps: Vec<(BrRequest, Result<BrObservation, BrRunError>)>,
    ) -> BrWorkProvider<ScriptedBrRunner, fn(&str) -> ContentHash> {
        let mut runner = ScriptedBrRunner::new();
        for (request, result) in steps {
            runner = runner.expect(request, result);
        }
        BrWorkProvider::new(runner, PINNED, test_digest as fn(&str) -> ContentHash)
    }

    fn bead(suffix: &str) -> BeadId {
        BeadId::new(&format!("ABACUS-{suffix}")).expect("valid bead id")
    }

    #[test]
    fn inspect_maps_open_bead_with_raw_labels_and_graph_revision() {
        let element = "{\"id\":\"abacus-x1\",\"status\":\"open\",\"title\":\"T\",\"priority\":1,\"labels\":[\"area:auth\",\"area:billing\"]}";
        let p = provider(vec![
            version_step(),
            sync_step(0),
            (show_request("abacus-x1"), ok(&format!("[{element}]"))),
        ]);
        let view = p.inspect_raw(&bead("x1")).expect("inspect succeeds");
        assert_eq!(view.status, WorkStatus::Open);
        assert_eq!(view.revision.0.as_str(), HASH);
        assert_eq!(
            view.snapshot.raw_labels,
            vec!["area:auth".to_owned(), "area:billing".to_owned()]
        );
        assert_eq!(view.snapshot.priority, 1);
        // Preimage v0 is the compact re-serialization of the element.
        let expected_preimage: Value = serde_json::from_str(element).unwrap();
        assert_eq!(
            view.snapshot.content_hash,
            test_digest(&expected_preimage.to_string())
        );
        p.runner.assert_exhausted();
    }

    #[test]
    fn tombstone_and_never_existing_both_read_as_not_found() {
        let tombstone =
            "[{\"id\":\"abacus-x2\",\"status\":\"tombstone\",\"priority\":2,\"labels\":[]}]";
        let p = provider(vec![
            version_step(),
            sync_step(0),
            (show_request("abacus-x2"), ok(tombstone)),
        ]);
        assert_eq!(p.inspect_raw(&bead("x2")), Err(WorkError::NotFound));

        let missing = "{\"error\":{\"code\":\"ISSUE_NOT_FOUND\",\"message\":\"Issue not found: abacus-x3\",\"retryable\":false}}";
        let p = provider(vec![
            version_step(),
            sync_step(0),
            (show_request("abacus-x3"), exit(3, missing)),
        ]);
        assert_eq!(p.inspect_raw(&bead("x3")), Err(WorkError::NotFound));
    }

    #[test]
    fn deferred_bead_reads_as_open_work() {
        let deferred =
            "[{\"id\":\"abacus-x4\",\"status\":\"deferred\",\"priority\":3,\"labels\":[]}]";
        let p = provider(vec![
            version_step(),
            sync_step(0),
            (show_request("abacus-x4"), ok(deferred)),
        ]);
        let view = p.inspect_raw(&bead("x4")).expect("deferred bead inspects");
        assert_eq!(view.status, WorkStatus::Open);
    }

    #[test]
    fn unbracketable_sync_status_is_busy_and_no_bead_read_is_issued() {
        // The script ends after sync-status: reaching `show` would
        // panic as an unexpected invocation.
        let p = provider(vec![version_step(), sync_step(3)]);
        assert_eq!(p.inspect_raw(&bead("x5")), Err(WorkError::Busy));
        p.runner.assert_exhausted();
    }

    fn op_id(raw: &str) -> abacus_core::OperationId {
        abacus_core::OperationId::new(raw).expect("valid operation id")
    }

    fn update_request(provider_id: &str, operation: &str) -> BrRequest {
        BrRequest::new([
            "update",
            provider_id,
            "--status=in_progress",
            &format!("--actor={operation}"),
            "--json",
        ])
    }

    fn second_sync_step() -> (BrRequest, Result<BrObservation, BrRunError>) {
        let other = "0d68cacaedf73f96d6eef77c164c0b00d1891e703c1da60591aaee1d6f29249e";
        let stdout = format!(
            "{{\"dirty_count\":0,\"jsonl_content_hash\":\"{other}\",\"jsonl_newer\":false,\"db_newer\":false}}"
        );
        (BrRequest::new(["sync", "--status", "--json"]), ok(&stdout))
    }

    #[test]
    fn applied_mutation_brackets_before_and_after_revisions() {
        let updated = "[{\"id\":\"abacus-m1\",\"title\":\"T\",\"status\":\"in_progress\",\"priority\":1,\"updated_at\":\"2026-08-05T13:48:35.608039553Z\"}]";
        let p = provider(vec![
            version_step(),
            sync_step(0),
            (update_request("abacus-m1", "op-m1"), ok(updated)),
            second_sync_step(),
        ]);
        let outcome = p
            .set_status_raw(&bead("m1"), TargetStatus::InProgress, &op_id("op-m1"))
            .expect("mutation reports");
        let ProviderMutation::Applied {
            before,
            after,
            summary,
        } = outcome
        else {
            panic!("expected applied, got {outcome:?}");
        };
        assert_eq!(before.0.as_str(), HASH);
        assert_eq!(
            after.0.as_str(),
            "0d68cacaedf73f96d6eef77c164c0b00d1891e703c1da60591aaee1d6f29249e"
        );
        assert_eq!(summary, "abacus-m1: in_progress");
        p.runner.assert_exhausted();
    }

    #[test]
    fn close_renders_exactly_the_canonical_curated_reason() {
        let closed = "[{\"id\":\"abacus-m2\",\"status\":\"closed\",\"priority\":1,\"close_reason\":\"abacus:accepted-handoff\"}]";
        let close_request = BrRequest::new([
            "close",
            "abacus-m2",
            "--reason=abacus:accepted-handoff",
            "--actor=op-m2",
            "--json",
        ]);
        let p = provider(vec![
            version_step(),
            sync_step(0),
            (close_request, ok(closed)),
            second_sync_step(),
        ]);
        let outcome = p
            .set_status_raw(
                &bead("m2"),
                TargetStatus::Closed(abacus_core::ports::CloseReason::AcceptedHandoff),
                &op_id("op-m2"),
            )
            .expect("close reports");
        assert!(matches!(outcome, ProviderMutation::Applied { .. }));
        p.runner.assert_exhausted();
    }

    #[test]
    fn uncertain_transport_after_issuing_is_ambiguous_never_err() {
        for error in [
            BrRunError::DeadlineExceeded,
            BrRunError::OutputBoundExceeded,
        ] {
            let p = provider(vec![
                version_step(),
                sync_step(0),
                (update_request("abacus-m3", "op-m3"), Err(error)),
            ]);
            assert_eq!(
                p.set_status_raw(&bead("m3"), TargetStatus::InProgress, &op_id("op-m3")),
                Ok(ProviderMutation::Ambiguous),
                "{error:?} may have landed; Err would invite a double-apply"
            );
            p.runner.assert_exhausted();
        }
    }

    #[test]
    fn spawn_failure_is_definitively_not_applied() {
        let p = provider(vec![
            version_step(),
            sync_step(0),
            (update_request("abacus-m4", "op-m4"), Err(BrRunError::Spawn)),
        ]);
        assert_eq!(
            p.set_status_raw(&bead("m4"), TargetStatus::InProgress, &op_id("op-m4")),
            Err(WorkError::ProviderUnavailable)
        );
    }

    #[test]
    fn structured_refusals_map_and_unreadable_failure_is_ambiguous() {
        let missing = "{\"error\":{\"code\":\"ISSUE_NOT_FOUND\",\"message\":\"Issue not found: abacus-m5\",\"retryable\":false}}";
        let p = provider(vec![
            version_step(),
            sync_step(0),
            (update_request("abacus-m5", "op-m5"), exit(3, missing)),
        ]);
        assert_eq!(
            p.set_status_raw(&bead("m5"), TargetStatus::InProgress, &op_id("op-m5")),
            Err(WorkError::NotFound)
        );

        let garbled = provider(vec![
            version_step(),
            sync_step(0),
            (
                update_request("abacus-m6", "op-m6"),
                exit(1, "panic: poisoned lock"),
            ),
        ]);
        assert_eq!(
            garbled.set_status_raw(&bead("m6"), TargetStatus::InProgress, &op_id("op-m6")),
            Ok(ProviderMutation::Ambiguous),
            "an unreadable failure after issuing must never claim not-applied"
        );
    }

    #[test]
    fn unconfirmable_success_output_and_failed_after_read_are_ambiguous() {
        let p = provider(vec![
            version_step(),
            sync_step(0),
            (update_request("abacus-m7", "op-m7"), ok("not json at all")),
        ]);
        assert_eq!(
            p.set_status_raw(&bead("m7"), TargetStatus::InProgress, &op_id("op-m7")),
            Ok(ProviderMutation::Ambiguous)
        );

        let updated = "[{\"id\":\"abacus-m8\",\"status\":\"in_progress\",\"priority\":1}]";
        let after_read_fails = provider(vec![
            version_step(),
            sync_step(0),
            (update_request("abacus-m8", "op-m8"), ok(updated)),
            (
                BrRequest::new(["sync", "--status", "--json"]),
                Err(BrRunError::DeadlineExceeded),
            ),
        ]);
        assert_eq!(
            after_read_fails.set_status_raw(&bead("m8"), TargetStatus::InProgress, &op_id("op-m8")),
            Ok(ProviderMutation::Ambiguous),
            "a landed mutation without an honest after-revision reports unknown"
        );
    }

    #[test]
    fn unbracketable_before_read_refuses_with_no_mutation_issued() {
        // Script ends after the dirty sync-status: issuing the update
        // would panic as an unexpected invocation.
        let p = provider(vec![version_step(), sync_step(4)]);
        assert_eq!(
            p.set_status_raw(&bead("m9"), TargetStatus::InProgress, &op_id("op-m9")),
            Err(WorkError::Busy)
        );
        p.runner.assert_exhausted();
    }

    #[test]
    fn version_drift_refuses_before_any_graph_command() {
        let p = provider(vec![(BrRequest::new(["--version"]), ok("br 0.1.99\n"))]);
        assert_eq!(p.inspect_raw(&bead("x6")), Err(WorkError::Incompatible));
        p.runner.assert_exhausted();
    }

    #[test]
    fn pin_probe_runs_once_per_provider_instance() {
        let first = "[{\"id\":\"abacus-x7\",\"status\":\"open\",\"priority\":1,\"labels\":[]}]";
        let second =
            "[{\"id\":\"abacus-x7\",\"status\":\"in_progress\",\"priority\":1,\"labels\":[]}]";
        let p = provider(vec![
            version_step(),
            sync_step(0),
            (show_request("abacus-x7"), ok(first)),
            sync_step(0),
            (show_request("abacus-x7"), ok(second)),
        ]);
        assert_eq!(
            p.inspect_raw(&bead("x7")).expect("first inspect").status,
            WorkStatus::Open
        );
        assert_eq!(
            p.inspect_raw(&bead("x7")).expect("second inspect").status,
            WorkStatus::InProgress
        );
        p.runner.assert_exhausted();
    }

    #[test]
    fn a_foreign_or_mismatched_answer_is_malformed_output() {
        let wrong = "[{\"id\":\"abacus-other\",\"status\":\"open\",\"priority\":1,\"labels\":[]}]";
        let p = provider(vec![
            version_step(),
            sync_step(0),
            (show_request("abacus-x8"), ok(wrong)),
        ]);
        assert_eq!(p.inspect_raw(&bead("x8")), Err(WorkError::MalformedOutput));
    }

    fn ready_request() -> BrRequest {
        BrRequest::new(["ready", "--limit", "0", "--json"])
    }

    #[test]
    fn ready_brackets_the_batch_and_fills_labels_from_shows() {
        let listing = "[{\"id\":\"abacus-r1\",\"status\":\"open\",\"priority\":1},{\"id\":\"abacus-r2\",\"status\":\"open\",\"priority\":2}]";
        let first = "[{\"id\":\"abacus-r1\",\"status\":\"open\",\"priority\":1,\"labels\":[\"area:auth\"]}]";
        let second = "[{\"id\":\"abacus-r2\",\"status\":\"open\",\"priority\":2,\"labels\":[\"module:pay\"]}]";
        let p = provider(vec![
            version_step(),
            sync_step(0),
            (ready_request(), ok(listing)),
            (show_request("abacus-r1"), ok(first)),
            (show_request("abacus-r2"), ok(second)),
            sync_step(0),
        ]);
        let (revision, snapshots) = p.ready_raw().expect("bracketed ready succeeds");
        assert_eq!(revision.0.as_str(), HASH);
        assert_eq!(snapshots.len(), 2);
        assert_eq!(snapshots[0].raw_labels, vec!["area:auth".to_owned()]);
        assert_eq!(snapshots[1].raw_labels, vec!["module:pay".to_owned()]);
        p.runner.assert_exhausted();
    }

    #[test]
    fn a_moved_graph_during_the_batch_is_a_revision_conflict() {
        let listing = "[{\"id\":\"abacus-r3\",\"status\":\"open\",\"priority\":1}]";
        let shown = "[{\"id\":\"abacus-r3\",\"status\":\"open\",\"priority\":1,\"labels\":[]}]";
        let other_hash = "0d68cacaedf73f96d6eef77c164c0b00d1891e703c1da60591aaee1d6f29249e";
        let moved = format!(
            "{{\"dirty_count\":0,\"jsonl_content_hash\":\"{other_hash}\",\"jsonl_newer\":false,\"db_newer\":false}}"
        );
        let p = provider(vec![
            version_step(),
            sync_step(0),
            (ready_request(), ok(listing)),
            (show_request("abacus-r3"), ok(shown)),
            (BrRequest::new(["sync", "--status", "--json"]), ok(&moved)),
        ]);
        assert_eq!(p.ready_raw(), Err(WorkError::RevisionConflict));
        p.runner.assert_exhausted();
    }

    #[test]
    fn a_bead_vanishing_mid_batch_is_a_revision_conflict_not_not_found() {
        let listing = "[{\"id\":\"abacus-r4\",\"status\":\"open\",\"priority\":1}]";
        let missing = "{\"error\":{\"code\":\"ISSUE_NOT_FOUND\",\"message\":\"Issue not found: abacus-r4\",\"retryable\":false}}";
        let p = provider(vec![
            version_step(),
            sync_step(0),
            (ready_request(), ok(listing)),
            (show_request("abacus-r4"), exit(3, missing)),
        ]);
        assert_eq!(p.ready_raw(), Err(WorkError::RevisionConflict));
        p.runner.assert_exhausted();
    }

    #[test]
    fn an_empty_ready_queue_still_closes_the_bracket() {
        let p = provider(vec![
            version_step(),
            sync_step(0),
            (ready_request(), ok("[]")),
            sync_step(0),
        ]);
        let (revision, snapshots) = p.ready_raw().expect("empty ready succeeds");
        assert_eq!(revision.0.as_str(), HASH);
        assert!(snapshots.is_empty());
        p.runner.assert_exhausted();
    }
}
