//! Evidence values and red-green evidence-pair evaluation.
//!
//! Evidence records outcomes bound to artifacts (I4): the verification
//! command, its honest exit code, the normalized outcome, the commit it
//! ran against, and before/after workspace digests (ADR-0001 §9.2).
//! Outcomes are the closed set `pass` / `assert-fail` /
//! `execution-error`; an expectation flag can never rewrite them.
//!
//! The red-green pair (ADR-0001 §10, CONTEXT I4) is an acceptance-policy
//! *form* over two ordinary Evidence values: red is an assertion-level
//! failure of the policy-named verification files overlaid onto the
//! declared-base implementation, digest-bound so acceptance can prove
//! the same files ship in the Handoff commit; green is the same set
//! passing natively at the Handoff commit. There is no separate red or
//! green record type, and no coverage machinery.

use std::collections::BTreeMap;

use crate::content::{CommitId, ContentHash, WorkspaceDigest};
use crate::edit_scope::WorkPath;

/// Normalized verification outcome (closed set, wrapper-normalized).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum VerificationOutcome {
    Pass,
    /// Ran to completion and asserted failure.
    AssertFail,
    /// Did not run to completion: collection failure, missing file,
    /// usage or infrastructure error. Never satisfies red.
    ExecutionError,
}

/// One overlaid verification file with its content digest at capture.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OverlayFile {
    pub path: WorkPath,
    pub digest: ContentHash,
}

/// Overlay metadata on a red capture: the declared-base checkout plus
/// the exact files composed onto it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OverlayCapture {
    pub declared_base: CommitId,
    pub files: Vec<OverlayFile>,
}

/// One outcome record produced by the standard verification wrapper.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Evidence {
    pub command: String,
    pub exit_code: i32,
    pub outcome: VerificationOutcome,
    /// The commit identity the run was bound to: the Handoff commit for
    /// ordinary/green runs, the declared base for overlay (red) runs.
    pub commit: CommitId,
    pub workspace_before: WorkspaceDigest,
    pub workspace_after: WorkspaceDigest,
    /// Present exactly when the run was an overlay (red) capture.
    pub overlay: Option<OverlayCapture>,
}

/// The Assignment-selected red-green policy: which verification files
/// the form applies to, and the base implementation red must fail on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RedGreenPolicy {
    pub verification_paths: Vec<WorkPath>,
    pub declared_base: CommitId,
}

/// Distinct pair refusals (contract: each failure mode has its own
/// reason; `red-errored` and `red-stale` are the named novel ones).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PairRefusal {
    RedMissing,
    /// The claimed red run was not an overlay capture at all.
    RedNotOverlay,
    /// Red bound to a commit other than the policy's declared base.
    RedWrongBase,
    /// The claimed red run actually passed.
    RedActuallyPassed,
    /// Red recorded `execution-error`; errors never satisfy red.
    RedErrored,
    /// An overlaid path outside the policy's verification set:
    /// malformed evidence, refused before pairing.
    OverlayOutsidePolicy(WorkPath),
    /// An overlay digest that does not match the same file in the
    /// Handoff commit: verification edited after red capture.
    RedStale(WorkPath),
    GreenMissing,
    /// Green must run natively at the Handoff commit, never overlaid.
    GreenNotNative,
    /// Green bound to a commit other than the Handoff commit.
    GreenWrongCommit,
    /// Green did not record `pass`.
    GreenNotPass(VerificationOutcome),
}

/// Evaluate a required red-green pair against the Handoff commit.
///
/// `handoff_digests` maps each policy verification path to its content
/// digest in the Handoff commit (absent = file not in that commit).
/// Callers obtain digests through the commit-verification port; this
/// evaluation is pure.
pub fn evaluate_red_green_pair(
    policy: &RedGreenPolicy,
    red: Option<&Evidence>,
    green: Option<&Evidence>,
    handoff_commit: &CommitId,
    handoff_digests: &BTreeMap<WorkPath, ContentHash>,
) -> Result<(), PairRefusal> {
    let red = red.ok_or(PairRefusal::RedMissing)?;
    let overlay = red.overlay.as_ref().ok_or(PairRefusal::RedNotOverlay)?;

    // Malformed evidence is refused before pairing semantics.
    for file in &overlay.files {
        if !policy.verification_paths.contains(&file.path) {
            return Err(PairRefusal::OverlayOutsidePolicy(file.path.clone()));
        }
    }

    if red.commit != policy.declared_base || overlay.declared_base != policy.declared_base {
        return Err(PairRefusal::RedWrongBase);
    }
    match red.outcome {
        VerificationOutcome::AssertFail => {}
        VerificationOutcome::Pass => return Err(PairRefusal::RedActuallyPassed),
        VerificationOutcome::ExecutionError => return Err(PairRefusal::RedErrored),
    }

    // Every overlaid file must ship unchanged in the Handoff commit.
    for file in &overlay.files {
        match handoff_digests.get(&file.path) {
            Some(digest) if *digest == file.digest => {}
            _ => return Err(PairRefusal::RedStale(file.path.clone())),
        }
    }

    let green = green.ok_or(PairRefusal::GreenMissing)?;
    if green.overlay.is_some() {
        return Err(PairRefusal::GreenNotNative);
    }
    if green.commit != *handoff_commit {
        return Err(PairRefusal::GreenWrongCommit);
    }
    match green.outcome {
        VerificationOutcome::Pass => Ok(()),
        other => Err(PairRefusal::GreenNotPass(other)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn path(raw: &str) -> WorkPath {
        WorkPath::new(raw).unwrap()
    }

    fn hash(fill: char) -> ContentHash {
        ContentHash::new(&fill.to_string().repeat(64)).unwrap()
    }

    fn digest(fill: char) -> WorkspaceDigest {
        WorkspaceDigest::new(&fill.to_string().repeat(64)).unwrap()
    }

    fn commit(fill: char) -> CommitId {
        CommitId::new(&fill.to_string().repeat(40)).unwrap()
    }

    fn policy() -> RedGreenPolicy {
        RedGreenPolicy {
            verification_paths: vec![path("abacus-core/tests/contract.rs")],
            declared_base: commit('b'),
        }
    }

    fn red_evidence() -> Evidence {
        Evidence {
            command: "cargo test -p abacus-core".into(),
            exit_code: 101,
            outcome: VerificationOutcome::AssertFail,
            commit: commit('b'),
            workspace_before: digest('1'),
            workspace_after: digest('1'),
            overlay: Some(OverlayCapture {
                declared_base: commit('b'),
                files: vec![OverlayFile {
                    path: path("abacus-core/tests/contract.rs"),
                    digest: hash('d'),
                }],
            }),
        }
    }

    fn green_evidence() -> Evidence {
        Evidence {
            command: "cargo test -p abacus-core".into(),
            exit_code: 0,
            outcome: VerificationOutcome::Pass,
            commit: commit('c'),
            workspace_before: digest('2'),
            workspace_after: digest('2'),
            overlay: None,
        }
    }

    fn handoff_digests() -> BTreeMap<WorkPath, ContentHash> {
        BTreeMap::from([(path("abacus-core/tests/contract.rs"), hash('d'))])
    }

    #[test]
    fn valid_pair_passes() {
        assert_eq!(
            evaluate_red_green_pair(
                &policy(),
                Some(&red_evidence()),
                Some(&green_evidence()),
                &commit('c'),
                &handoff_digests(),
            ),
            Ok(())
        );
    }

    #[test]
    fn every_refusal_is_distinct() {
        let p = policy();
        let handoff = commit('c');
        let digests = handoff_digests();
        let red = red_evidence();
        let green = green_evidence();

        assert_eq!(
            evaluate_red_green_pair(&p, None, Some(&green), &handoff, &digests),
            Err(PairRefusal::RedMissing)
        );

        let mut not_overlay = red.clone();
        not_overlay.overlay = None;
        assert_eq!(
            evaluate_red_green_pair(&p, Some(&not_overlay), Some(&green), &handoff, &digests),
            Err(PairRefusal::RedNotOverlay)
        );

        let mut wrong_base = red.clone();
        wrong_base.commit = commit('a');
        assert_eq!(
            evaluate_red_green_pair(&p, Some(&wrong_base), Some(&green), &handoff, &digests),
            Err(PairRefusal::RedWrongBase)
        );

        let mut passed = red.clone();
        passed.outcome = VerificationOutcome::Pass;
        assert_eq!(
            evaluate_red_green_pair(&p, Some(&passed), Some(&green), &handoff, &digests),
            Err(PairRefusal::RedActuallyPassed)
        );

        let mut errored = red.clone();
        errored.outcome = VerificationOutcome::ExecutionError;
        assert_eq!(
            evaluate_red_green_pair(&p, Some(&errored), Some(&green), &handoff, &digests),
            Err(PairRefusal::RedErrored)
        );

        let mut outside = red.clone();
        outside.overlay.as_mut().unwrap().files[0].path = path("src/other.rs");
        assert_eq!(
            evaluate_red_green_pair(&p, Some(&outside), Some(&green), &handoff, &digests),
            Err(PairRefusal::OverlayOutsidePolicy(path("src/other.rs")))
        );

        let mut stale = red.clone();
        stale.overlay.as_mut().unwrap().files[0].digest = hash('e');
        assert_eq!(
            evaluate_red_green_pair(&p, Some(&stale), Some(&green), &handoff, &digests),
            Err(PairRefusal::RedStale(path("abacus-core/tests/contract.rs")))
        );

        // A policy file absent from the handoff commit is also stale.
        assert_eq!(
            evaluate_red_green_pair(&p, Some(&red), Some(&green), &handoff, &BTreeMap::new()),
            Err(PairRefusal::RedStale(path("abacus-core/tests/contract.rs")))
        );

        assert_eq!(
            evaluate_red_green_pair(&p, Some(&red), None, &handoff, &digests),
            Err(PairRefusal::GreenMissing)
        );

        let mut green_overlay = green.clone();
        green_overlay.overlay = red.overlay.clone();
        assert_eq!(
            evaluate_red_green_pair(&p, Some(&red), Some(&green_overlay), &handoff, &digests),
            Err(PairRefusal::GreenNotNative)
        );

        let mut wrong_commit = green.clone();
        wrong_commit.commit = commit('a');
        assert_eq!(
            evaluate_red_green_pair(&p, Some(&red), Some(&wrong_commit), &handoff, &digests),
            Err(PairRefusal::GreenWrongCommit)
        );

        let mut green_failed = green.clone();
        green_failed.outcome = VerificationOutcome::AssertFail;
        assert_eq!(
            evaluate_red_green_pair(&p, Some(&red), Some(&green_failed), &handoff, &digests),
            Err(PairRefusal::GreenNotPass(VerificationOutcome::AssertFail))
        );
    }

    /// The structural point of the pair: a vacuous verification that
    /// cannot fail cannot produce a valid red half even though its
    /// green half passes.
    #[test]
    fn vacuous_verification_cannot_satisfy_the_pair() {
        let mut vacuous_red = red_evidence();
        vacuous_red.outcome = VerificationOutcome::Pass; // it "ran" and passed on base too
        assert_eq!(
            evaluate_red_green_pair(
                &policy(),
                Some(&vacuous_red),
                Some(&green_evidence()),
                &commit('c'),
                &handoff_digests(),
            ),
            Err(PairRefusal::RedActuallyPassed)
        );
    }
}
