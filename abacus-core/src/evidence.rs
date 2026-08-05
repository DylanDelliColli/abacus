//! Evidence values and red-green evidence-pair evaluation.
//!
//! Evidence records outcomes bound to artifacts (I4): the structured
//! verification command, its honest exit code, the normalized outcome,
//! the commit it ran against, and before/after workspace digests
//! (ADR-0001 §9.2). Collections are validated canonical values —
//! bounded, duplicate-free, deterministically ordered — so payload
//! bounds and identity are enforceable at the protocol boundary, not
//! aspirational.
//!
//! The red-green pair (ADR-0001 §10, CONTEXT I4) is an acceptance-policy
//! *form* over two ordinary Evidence values; there is no separate red or
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
    /// Did not run to completion. Never satisfies red.
    ExecutionError,
}

/// Validated-collection failures.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CollectionError {
    Empty,
    TooMany,
    Duplicate(String),
    ItemInvalid(String),
}

/// Canonical nonempty set of repository paths: duplicate-free, sorted,
/// at most 64 entries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PathSet(Vec<WorkPath>);

impl PathSet {
    pub fn new(mut paths: Vec<WorkPath>) -> Result<Self, CollectionError> {
        if paths.is_empty() {
            return Err(CollectionError::Empty);
        }
        if paths.len() > 64 {
            return Err(CollectionError::TooMany);
        }
        paths.sort();
        for pair in paths.windows(2) {
            if pair[0] == pair[1] {
                return Err(CollectionError::Duplicate(pair[0].as_str().to_owned()));
            }
        }
        Ok(Self(paths))
    }

    pub fn iter(&self) -> impl Iterator<Item = &WorkPath> {
        self.0.iter()
    }

    pub fn contains(&self, path: &WorkPath) -> bool {
        self.0.binary_search(path).is_ok()
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

/// Structured command shape: argv, never a shell string. Nonempty, at
/// most 32 items, each 1..=200 bytes without control characters.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Argv(Vec<String>);

impl Argv {
    pub fn new(items: Vec<String>) -> Result<Self, CollectionError> {
        if items.is_empty() {
            return Err(CollectionError::Empty);
        }
        if items.len() > 32 {
            return Err(CollectionError::TooMany);
        }
        for item in &items {
            if item.is_empty() || item.len() > 200 || item.bytes().any(|b| b.is_ascii_control()) {
                return Err(CollectionError::ItemInvalid(item.clone()));
            }
        }
        Ok(Self(items))
    }

    pub fn iter(&self) -> impl Iterator<Item = &str> {
        self.0.iter().map(String::as_str)
    }
}

/// One file with its content digest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OverlayFile {
    pub path: WorkPath,
    pub digest: ContentHash,
}

/// Canonical file-digest set: duplicate-free by path, sorted by path,
/// at most 64 entries; may be empty (an evidence run can produce no
/// artifacts).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FileDigestSet(Vec<OverlayFile>);

impl FileDigestSet {
    pub fn new(mut files: Vec<OverlayFile>) -> Result<Self, CollectionError> {
        if files.len() > 64 {
            return Err(CollectionError::TooMany);
        }
        files.sort_by(|a, b| a.path.cmp(&b.path));
        for pair in files.windows(2) {
            if pair[0].path == pair[1].path {
                return Err(CollectionError::Duplicate(pair[0].path.as_str().to_owned()));
            }
        }
        Ok(Self(files))
    }

    pub fn iter(&self) -> impl Iterator<Item = &OverlayFile> {
        self.0.iter()
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

/// The normalized verification-set identity the contracts bind: the
/// policy-named commands AND paths together. Path equality alone is
/// insufficient — a vacuous red (`false`) and green (`true`) sharing a
/// path set must not pair. Commands are bounded (1..=8) and ordered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerificationSet {
    commands: Vec<Argv>,
    paths: PathSet,
}

impl VerificationSet {
    pub fn new(commands: Vec<Argv>, paths: PathSet) -> Result<Self, CollectionError> {
        if commands.is_empty() {
            return Err(CollectionError::Empty);
        }
        if commands.len() > 8 {
            return Err(CollectionError::TooMany);
        }
        for (i, command) in commands.iter().enumerate() {
            if commands[i + 1..].contains(command) {
                return Err(CollectionError::Duplicate(
                    command.iter().collect::<Vec<_>>().join(" "),
                ));
            }
        }
        Ok(Self { commands, paths })
    }

    pub fn commands(&self) -> &[Argv] {
        &self.commands
    }

    pub fn paths(&self) -> &PathSet {
        &self.paths
    }
}

/// Overlay metadata on a red capture: the declared-base checkout plus
/// the exact files composed onto it (nonempty by policy check).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OverlayCapture {
    pub declared_base: CommitId,
    pub files: FileDigestSet,
}

/// Evidence shape defects refused at construction.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EvidenceShapeError {
    /// The executed argv is not a member of the claimed verification
    /// set's commands: the claim would be dishonest by construction.
    CommandNotInVerificationSet,
}

/// One outcome record produced by the standard verification wrapper.
/// `argv` and `verification` are private and constructor-validated:
/// an Evidence whose executed command is not a member of its claimed
/// verification set is unrepresentable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Evidence {
    argv: Argv,
    verification: VerificationSet,
    pub exit_code: i32,
    pub outcome: VerificationOutcome,
    /// The commit identity the run was bound to: the Handoff commit for
    /// ordinary/green runs, the declared base for overlay (red) runs.
    pub commit: CommitId,
    pub workspace_before: WorkspaceDigest,
    pub workspace_after: WorkspaceDigest,
    /// Present exactly when the run was an overlay (red) capture.
    pub overlay: Option<OverlayCapture>,
    /// Relevant produced artifacts with content digests.
    pub artifacts: FileDigestSet,
    /// Fingerprint of the execution environment facts, when captured.
    pub environment_fingerprint: Option<ContentHash>,
}

impl Evidence {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        argv: Argv,
        verification: VerificationSet,
        exit_code: i32,
        outcome: VerificationOutcome,
        commit: CommitId,
        workspace_before: WorkspaceDigest,
        workspace_after: WorkspaceDigest,
        overlay: Option<OverlayCapture>,
        artifacts: FileDigestSet,
        environment_fingerprint: Option<ContentHash>,
    ) -> Result<Self, EvidenceShapeError> {
        if !verification.commands().contains(&argv) {
            return Err(EvidenceShapeError::CommandNotInVerificationSet);
        }
        Ok(Self {
            argv,
            verification,
            exit_code,
            outcome,
            commit,
            workspace_before,
            workspace_after,
            overlay,
            artifacts,
            environment_fingerprint,
        })
    }

    /// The honest executed command for THIS run.
    pub fn argv(&self) -> &Argv {
        &self.argv
    }

    /// The claimed normalized verification-set identity.
    pub fn verification(&self) -> &VerificationSet {
        &self.verification
    }
}

/// The acceptance-policy *form* (I5; ADR-0001 §10): every Assignment
/// binds its verification policy; red-green is an optional form of it,
/// never the presence of one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PolicyForm {
    Standard,
    /// Red runs against the Assignment's declared base.
    RedGreen,
}

/// The always-present acceptance policy bound at Assignment creation:
/// `None` can never mean "no bound evidence" because there is no
/// `None` — the verification set is unconditional.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AcceptancePolicy {
    pub verification: VerificationSet,
    pub form: PolicyForm,
}

impl AcceptancePolicy {
    /// Derive the pair-evaluator input when the red-green form is
    /// selected, binding the Assignment's declared base.
    pub fn red_green(&self, declared_base: &CommitId) -> Option<RedGreenPolicy> {
        match self.form {
            PolicyForm::RedGreen => Some(RedGreenPolicy {
                verification: self.verification.clone(),
                declared_base: declared_base.clone(),
            }),
            PolicyForm::Standard => None,
        }
    }
}

/// The red-green pair-evaluation input: the policy verification set
/// plus the base implementation red must fail on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RedGreenPolicy {
    pub verification: VerificationSet,
    pub declared_base: CommitId,
}

/// Distinct pair refusals.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PairRefusal {
    RedMissing,
    RedNotOverlay,
    /// Red ran a different verification set than the policy names.
    RedWrongSet,
    /// Green ran a different verification set than the policy names.
    GreenWrongSet,
    RedWrongBase,
    RedActuallyPassed,
    RedErrored,
    /// An overlaid path outside the policy's verification set:
    /// malformed evidence, refused before pairing.
    OverlayOutsidePolicy(WorkPath),
    /// Overlay digest mismatch against the Handoff commit.
    RedStale(WorkPath),
    GreenMissing,
    GreenNotNative,
    GreenWrongCommit,
    GreenNotPass(VerificationOutcome),
}

/// Aggregate acceptance refusals: the policy is evaluated over the
/// complete evidence bundle, per command, set-complete.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PolicyRefusal {
    /// Evidence claiming a verification set other than the policy's.
    SetMismatch,
    /// A policy command with no (or incomplete) evidence.
    MissingCommandEvidence,
    /// More than one evidence record for one command role.
    DuplicateCommand,
    /// A per-command red-green pairing failure.
    Pair(PairRefusal),
    /// A Standard-form command that did not pass at the Handoff commit.
    NotPassing(VerificationOutcome),
    /// Evidence bound to a commit other than the Handoff commit.
    WrongCommit,
    /// A verification run mutated the workspace (before/after digests
    /// differ) without an accounting mechanism; v1 is strict (core
    /// invariant 17).
    WorkspaceMutated,
}

/// Evaluate the complete always-present [`AcceptancePolicy`] over the
/// full evidence bundle: exact command coverage (every policy command,
/// no foreign or duplicate roles), same-command red/green pairing for
/// every command under the red-green form, commit binding, pass
/// requirements, and strict workspace-digest equality. The single-pair
/// helper is the internal per-command primitive.
pub fn evaluate_acceptance(
    policy: &AcceptancePolicy,
    declared_base: &CommitId,
    evidence: &[Evidence],
    handoff_commit: &CommitId,
    handoff_digests: &BTreeMap<WorkPath, ContentHash>,
) -> Result<(), PolicyRefusal> {
    for record in evidence {
        if record.verification != policy.verification {
            return Err(PolicyRefusal::SetMismatch);
        }
        if record.workspace_before != record.workspace_after {
            return Err(PolicyRefusal::WorkspaceMutated);
        }
    }
    match policy.form {
        PolicyForm::Standard => {
            for command in policy.verification.commands() {
                let matching: Vec<&Evidence> =
                    evidence.iter().filter(|e| &e.argv == command).collect();
                match matching.as_slice() {
                    [] => return Err(PolicyRefusal::MissingCommandEvidence),
                    [one] => {
                        if one.overlay.is_some() || one.commit != *handoff_commit {
                            return Err(PolicyRefusal::WrongCommit);
                        }
                        if one.outcome != VerificationOutcome::Pass {
                            return Err(PolicyRefusal::NotPassing(one.outcome));
                        }
                    }
                    _ => return Err(PolicyRefusal::DuplicateCommand),
                }
            }
            Ok(())
        }
        PolicyForm::RedGreen => {
            let pair_policy = RedGreenPolicy {
                verification: policy.verification.clone(),
                declared_base: declared_base.clone(),
            };
            for command in policy.verification.commands() {
                let reds: Vec<&Evidence> = evidence
                    .iter()
                    .filter(|e| &e.argv == command && e.overlay.is_some())
                    .collect();
                let greens: Vec<&Evidence> = evidence
                    .iter()
                    .filter(|e| &e.argv == command && e.overlay.is_none())
                    .collect();
                if reds.len() > 1 || greens.len() > 1 {
                    return Err(PolicyRefusal::DuplicateCommand);
                }
                // Same-command pairing: a red for `false` can never be
                // paired with a green for `true`.
                let (red, green) = match (reds.first(), greens.first()) {
                    (Some(r), Some(g)) => (*r, *g),
                    _ => return Err(PolicyRefusal::MissingCommandEvidence),
                };
                evaluate_red_green_pair(
                    &pair_policy,
                    Some(red),
                    Some(green),
                    handoff_commit,
                    handoff_digests,
                )
                .map_err(PolicyRefusal::Pair)?;
            }
            Ok(())
        }
    }
}

/// Evaluate a required red-green pair against the Handoff commit.
pub fn evaluate_red_green_pair(
    policy: &RedGreenPolicy,
    red: Option<&Evidence>,
    green: Option<&Evidence>,
    handoff_commit: &CommitId,
    handoff_digests: &BTreeMap<WorkPath, ContentHash>,
) -> Result<(), PairRefusal> {
    let red = red.ok_or(PairRefusal::RedMissing)?;
    let overlay = red.overlay.as_ref().ok_or(PairRefusal::RedNotOverlay)?;
    if red.verification != policy.verification {
        return Err(PairRefusal::RedWrongSet);
    }

    for file in overlay.files.iter() {
        if !policy.verification.paths().contains(&file.path) {
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

    for file in overlay.files.iter() {
        match handoff_digests.get(&file.path) {
            Some(digest) if *digest == file.digest => {}
            _ => return Err(PairRefusal::RedStale(file.path.clone())),
        }
    }

    let green = green.ok_or(PairRefusal::GreenMissing)?;
    if green.verification != policy.verification {
        return Err(PairRefusal::GreenWrongSet);
    }
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

    fn argv() -> Argv {
        Argv::new(vec![
            "cargo".into(),
            "test".into(),
            "-p".into(),
            "abacus-core".into(),
        ])
        .unwrap()
    }

    fn verification() -> VerificationSet {
        VerificationSet::new(
            vec![argv()],
            PathSet::new(vec![path("abacus-core/tests/contract.rs")]).unwrap(),
        )
        .unwrap()
    }

    fn overlay_files(digest_fill: char) -> FileDigestSet {
        FileDigestSet::new(vec![OverlayFile {
            path: path("abacus-core/tests/contract.rs"),
            digest: hash(digest_fill),
        }])
        .unwrap()
    }

    fn policy() -> RedGreenPolicy {
        RedGreenPolicy {
            verification: verification(),
            declared_base: commit('b'),
        }
    }

    fn red_evidence() -> Evidence {
        Evidence::new(
            argv(),
            verification(),
            101,
            VerificationOutcome::AssertFail,
            commit('b'),
            digest('1'),
            digest('1'),
            Some(OverlayCapture {
                declared_base: commit('b'),
                files: overlay_files('d'),
            }),
            FileDigestSet::default(),
            None,
        )
        .unwrap()
    }

    fn green_evidence() -> Evidence {
        Evidence::new(
            argv(),
            verification(),
            0,
            VerificationOutcome::Pass,
            commit('c'),
            digest('2'),
            digest('2'),
            None,
            FileDigestSet::default(),
            None,
        )
        .unwrap()
    }

    fn handoff_digests() -> BTreeMap<WorkPath, ContentHash> {
        BTreeMap::from([(path("abacus-core/tests/contract.rs"), hash('d'))])
    }

    #[test]
    fn canonical_collections_are_validated() {
        assert_eq!(Argv::new(vec![]), Err(CollectionError::Empty));
        assert!(matches!(
            Argv::new(vec!["a\nb".into()]),
            Err(CollectionError::ItemInvalid(_))
        ));
        assert_eq!(PathSet::new(vec![]), Err(CollectionError::Empty));
        assert!(matches!(
            PathSet::new(vec![path("a"), path("a")]),
            Err(CollectionError::Duplicate(_))
        ));
        // Deterministic order: inputs are canonicalized by sorting.
        let set = PathSet::new(vec![path("z"), path("a")]).unwrap();
        assert_eq!(
            set.iter().map(|p| p.as_str()).collect::<Vec<_>>(),
            vec!["a", "z"]
        );
        assert!(matches!(
            FileDigestSet::new(vec![
                OverlayFile {
                    path: path("a"),
                    digest: hash('1')
                },
                OverlayFile {
                    path: path("a"),
                    digest: hash('2')
                },
            ]),
            Err(CollectionError::Duplicate(_))
        ));
        // Artifacts may be empty; verification sets may not.
        assert!(FileDigestSet::new(vec![]).is_ok());
        // A VerificationSet is a canonical set: duplicate commands are
        // rejected so one Evidence can never satisfy a policy command
        // twice.
        let cmd = Argv::new(vec!["cargo".into(), "test".into()]).unwrap();
        assert!(matches!(
            VerificationSet::new(
                vec![cmd.clone(), cmd],
                PathSet::new(vec![path("a")]).unwrap()
            ),
            Err(CollectionError::Duplicate(_))
        ));
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
        outside.overlay = Some(OverlayCapture {
            declared_base: commit('b'),
            files: FileDigestSet::new(vec![OverlayFile {
                path: path("src/other.rs"),
                digest: hash('d'),
            }])
            .unwrap(),
        });
        assert_eq!(
            evaluate_red_green_pair(&p, Some(&outside), Some(&green), &handoff, &digests),
            Err(PairRefusal::OverlayOutsidePolicy(path("src/other.rs")))
        );

        let mut stale = red.clone();
        stale.overlay = Some(OverlayCapture {
            declared_base: commit('b'),
            files: overlay_files('e'),
        });
        assert_eq!(
            evaluate_red_green_pair(&p, Some(&stale), Some(&green), &handoff, &digests),
            Err(PairRefusal::RedStale(path("abacus-core/tests/contract.rs")))
        );

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

        let other_set = VerificationSet::new(
            vec![argv()],
            PathSet::new(vec![path("other/test.rs")]).unwrap(),
        )
        .unwrap();
        let red_wrong_set = Evidence::new(
            argv(),
            other_set.clone(),
            101,
            VerificationOutcome::AssertFail,
            commit('b'),
            digest('1'),
            digest('1'),
            red.overlay.clone(),
            FileDigestSet::default(),
            None,
        )
        .unwrap();
        assert_eq!(
            evaluate_red_green_pair(&p, Some(&red_wrong_set), Some(&green), &handoff, &digests),
            Err(PairRefusal::RedWrongSet)
        );

        let green_wrong_set = Evidence::new(
            argv(),
            other_set,
            0,
            VerificationOutcome::Pass,
            commit('c'),
            digest('2'),
            digest('2'),
            None,
            FileDigestSet::default(),
            None,
        )
        .unwrap();
        assert_eq!(
            evaluate_red_green_pair(&p, Some(&red), Some(&green_wrong_set), &handoff, &digests),
            Err(PairRefusal::GreenWrongSet)
        );

        let mut green_failed = green.clone();
        green_failed.outcome = VerificationOutcome::AssertFail;
        assert_eq!(
            evaluate_red_green_pair(&p, Some(&red), Some(&green_failed), &handoff, &digests),
            Err(PairRefusal::GreenNotPass(VerificationOutcome::AssertFail))
        );
    }

    /// Same path set, different command sets: the pair must refuse —
    /// otherwise red argv [false] and green argv [true] over one path
    /// set would launder a vacuous pair.
    #[test]
    fn command_identity_is_part_of_the_verification_set() {
        // Claiming the policy VerificationSet while actually executing
        // a different command is unconstructible: argv membership is
        // validated at Evidence construction (the false/true attack).
        assert_eq!(
            Evidence::new(
                Argv::new(vec!["false".into()]).unwrap(),
                verification(),
                1,
                VerificationOutcome::AssertFail,
                commit('b'),
                digest('1'),
                digest('1'),
                None,
                FileDigestSet::default(),
                None,
            ),
            Err(EvidenceShapeError::CommandNotInVerificationSet)
        );
        assert_eq!(
            Evidence::new(
                Argv::new(vec!["true".into()]).unwrap(),
                verification(),
                0,
                VerificationOutcome::Pass,
                commit('c'),
                digest('2'),
                digest('2'),
                None,
                FileDigestSet::default(),
                None,
            ),
            Err(EvidenceShapeError::CommandNotInVerificationSet)
        );
        // And a coherent-but-different command set still refuses at
        // pairing (RedWrongSet), so both layers hold.
        let false_set = VerificationSet::new(
            vec![Argv::new(vec!["false".into()]).unwrap()],
            PathSet::new(vec![path("abacus-core/tests/contract.rs")]).unwrap(),
        )
        .unwrap();
        let red = Evidence::new(
            Argv::new(vec!["false".into()]).unwrap(),
            false_set,
            1,
            VerificationOutcome::AssertFail,
            commit('b'),
            digest('1'),
            digest('1'),
            red_evidence().overlay.clone(),
            FileDigestSet::default(),
            None,
        )
        .unwrap();
        assert_eq!(
            evaluate_red_green_pair(
                &policy(),
                Some(&red),
                Some(&green_evidence()),
                &commit('c'),
                &handoff_digests(),
            ),
            Err(PairRefusal::RedWrongSet)
        );
    }

    /// The [false,true] laundering attack at the aggregate level: a
    /// policy with two commands cannot pair red=false with green=true —
    /// every command needs its own same-command red AND green.
    #[test]
    fn aggregate_acceptance_requires_same_command_pairs() {
        let false_cmd = Argv::new(vec!["false".into()]).unwrap();
        let true_cmd = Argv::new(vec!["true".into()]).unwrap();
        let paths = PathSet::new(vec![path("abacus-core/tests/contract.rs")]).unwrap();
        let set = VerificationSet::new(vec![false_cmd.clone(), true_cmd.clone()], paths).unwrap();
        let policy = AcceptancePolicy {
            verification: set.clone(),
            form: PolicyForm::RedGreen,
        };
        let red = Evidence::new(
            false_cmd,
            set.clone(),
            1,
            VerificationOutcome::AssertFail,
            commit('b'),
            digest('1'),
            digest('1'),
            Some(OverlayCapture {
                declared_base: commit('b'),
                files: overlay_files('d'),
            }),
            FileDigestSet::default(),
            None,
        )
        .unwrap();
        let green = Evidence::new(
            true_cmd,
            set,
            0,
            VerificationOutcome::Pass,
            commit('c'),
            digest('2'),
            digest('2'),
            None,
            FileDigestSet::default(),
            None,
        )
        .unwrap();
        assert_eq!(
            evaluate_acceptance(
                &policy,
                &commit('b'),
                &[red, green],
                &commit('c'),
                &handoff_digests(),
            ),
            Err(PolicyRefusal::MissingCommandEvidence)
        );
    }

    #[test]
    fn aggregate_acceptance_standard_and_redgreen_forms() {
        // Red-green form: the classic single-command pair passes whole.
        let rg = AcceptancePolicy {
            verification: verification(),
            form: PolicyForm::RedGreen,
        };
        assert_eq!(
            evaluate_acceptance(
                &rg,
                &commit('b'),
                &[red_evidence(), green_evidence()],
                &commit('c'),
                &handoff_digests(),
            ),
            Ok(())
        );
        // Duplicate green for one command is refused.
        assert_eq!(
            evaluate_acceptance(
                &rg,
                &commit('b'),
                &[red_evidence(), green_evidence(), green_evidence()],
                &commit('c'),
                &handoff_digests(),
            ),
            Err(PolicyRefusal::DuplicateCommand)
        );
        // Standard form: one passing native run per command suffices;
        // a mutated workspace is a distinct refusal.
        let standard = AcceptancePolicy {
            verification: verification(),
            form: PolicyForm::Standard,
        };
        assert_eq!(
            evaluate_acceptance(
                &standard,
                &commit('b'),
                &[green_evidence()],
                &commit('c'),
                &handoff_digests(),
            ),
            Ok(())
        );
        assert_eq!(
            evaluate_acceptance(
                &standard,
                &commit('b'),
                &[],
                &commit('c'),
                &handoff_digests()
            ),
            Err(PolicyRefusal::MissingCommandEvidence)
        );
        let mutated = Evidence::new(
            argv(),
            verification(),
            0,
            VerificationOutcome::Pass,
            commit('c'),
            digest('2'),
            digest('3'),
            None,
            FileDigestSet::default(),
            None,
        )
        .unwrap();
        assert_eq!(
            evaluate_acceptance(
                &standard,
                &commit('b'),
                &[mutated],
                &commit('c'),
                &handoff_digests(),
            ),
            Err(PolicyRefusal::WorkspaceMutated)
        );
    }

    #[test]
    fn vacuous_verification_cannot_satisfy_the_pair() {
        let mut vacuous_red = red_evidence();
        vacuous_red.outcome = VerificationOutcome::Pass;
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
