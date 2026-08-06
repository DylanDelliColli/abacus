//! Private, versioned SQLite representations.
//!
//! These DTOs deliberately live below the state boundary. Core domain values
//! own meaning; this module owns storage representation and reconstructs every
//! value through its checked public constructors. No DTO escapes this crate or
//! participates in domain decisions.

use std::collections::{BTreeMap, BTreeSet};

use abacus_core::evidence::{AcceptancePolicy, PolicyForm};
use abacus_core::ports::*;
use abacus_core::*;
use rusqlite::{Connection, OptionalExtension, Transaction, params};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::memory::{AssignmentEntry, AttemptEntry, CredentialBinding, State};

pub(crate) const SCHEMA_VERSION: u32 = 3;
const IDENTITY_VERSION: u32 = 1;

#[derive(Debug, thiserror::Error)]
pub(crate) enum StoredError {
    #[error("stored representation version {found} is unsupported; expected {supported}")]
    UnsupportedVersion { found: u32, supported: u32 },
    #[error("stored JSON is malformed: {0}")]
    Json(#[from] serde_json::Error),
    #[error("sqlite storage error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("stored domain value is invalid: {0}")]
    Invalid(String),
}

#[derive(Debug, Serialize, Deserialize)]
struct Versioned<T> {
    schema_version: u32,
    value: T,
}

fn encode_row<T: Serialize>(value: &T) -> Result<String, StoredError> {
    Ok(serde_json::to_string(&Versioned {
        schema_version: SCHEMA_VERSION,
        value,
    })?)
}

fn encode_identity<T: Serialize>(value: &T) -> Result<String, StoredError> {
    Ok(serde_json::to_string(&Versioned {
        schema_version: IDENTITY_VERSION,
        value,
    })?)
}

fn decode_row<T: DeserializeOwned>(column_version: i64, json: &str) -> Result<T, StoredError> {
    let found = u32::try_from(column_version)
        .map_err(|_| invalid("stored schema version", column_version))?;
    if found != SCHEMA_VERSION {
        return Err(StoredError::UnsupportedVersion {
            found,
            supported: SCHEMA_VERSION,
        });
    }
    let envelope: Versioned<T> = serde_json::from_str(json)?;
    if envelope.schema_version != found {
        return Err(StoredError::Invalid(format!(
            "row version {found} disagrees with JSON version {}",
            envelope.schema_version
        )));
    }
    Ok(envelope.value)
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum StoredAuthorityClass {
    Orchestrator,
    Worker,
}

#[derive(Debug, Serialize, Deserialize)]
struct StoredCredentialBinding {
    credential: String,
    digest: String,
    actor: String,
    profile: String,
    assignment: Option<String>,
    revoked: bool,
}

#[derive(Debug, Serialize)]
struct StoredCredentialProvisioning {
    id: String,
    digest: String,
}

#[derive(Debug, Serialize)]
struct StoredAssignDecision {
    operation: String,
    assignment: String,
    first_attempt: String,
    authority: StoredAuthoritySnapshot,
}

#[derive(Debug, Serialize)]
struct StoredAssignmentOpening {
    assignment: StoredAssignmentRecord,
    first_attempt: StoredAttemptRecord,
    authorizing: StoredAssignDecision,
    bead_revision: String,
    worker_credential: StoredCredentialProvisioning,
}

#[derive(Debug, Serialize)]
struct StoredRetryDecision {
    operation: String,
    assignment: String,
    authority: StoredAuthoritySnapshot,
    reason: String,
}

#[derive(Debug, Serialize)]
struct StoredAttemptOpening {
    authorizing: StoredRetryDecision,
    attempt: StoredAttemptRecord,
    worker_credential: StoredCredentialProvisioning,
}

#[derive(Debug, Serialize)]
struct StoredGrant {
    capability: String,
    scope: StoredScopeExpr,
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
enum StoredOccupancyClass {
    Singleton,
    Shared,
}

#[derive(Debug, Serialize)]
struct StoredProfileActivation {
    operation: String,
    actor: String,
    profile: String,
    profile_hash: String,
    class: StoredAuthorityClass,
    occupancy: StoredOccupancyClass,
    grants: Vec<StoredGrant>,
}

#[derive(Debug, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum StoredActivationCase {
    OperatorBootstrap,
    ActorAuthorizedRotation { authority: StoredAuthoritySnapshot },
    OperatorRecovery,
    OperatorOrchestratorEnrolment,
}

#[derive(Debug, Serialize)]
struct StoredActivationOpening {
    activation: StoredProfileActivation,
    case: StoredActivationCase,
    credential: StoredCredentialProvisioning,
}

#[derive(Debug, Serialize)]
struct StoredSignalDraft {
    id: String,
    sender: StoredAuthoritySnapshot,
    subject: StoredSubjectRef,
    body: StoredSignalBody,
}

#[derive(Debug, Serialize)]
struct StoredFencedPayload<T> {
    fenced_identity: String,
    payload: T,
}

#[derive(Debug, Serialize)]
struct StoredRenewalIdentity {
    fenced_identity: String,
    until: u64,
}

#[derive(Debug, Serialize)]
struct StoredAssociationPayload<T> {
    association_key: String,
    payload: T,
}

#[derive(Debug, Serialize, Deserialize)]
struct StoredAssignmentRecord {
    id: String,
    bead: String,
    bead_content_hash: String,
    scope_map: Vec<(String, String)>,
    worker: StoredDecisionActor,
    decision_actor: StoredDecisionActor,
    edit_scope: Vec<String>,
    acceptance: StoredAcceptancePolicy,
    attempt_cap: Option<u8>,
    declared_base: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct StoredAttemptRecord {
    id: String,
    assignment: String,
    token: u64,
    expires_at: u64,
}

#[derive(Debug, Serialize, Deserialize)]
struct StoredDecisionActor {
    actor: String,
    class: StoredAuthorityClass,
    profile: String,
    profile_hash: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct StoredAuthoritySnapshot {
    actor: StoredDecisionActor,
    capability: String,
    scope: StoredScopeExpr,
}

#[derive(Debug, Serialize, Deserialize)]
struct StoredScopeExpr {
    canonical: String,
    declared_keys: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize)]
struct StoredAcceptancePolicy {
    verification: StoredVerificationSet,
    form: StoredPolicyForm,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum StoredPolicyForm {
    Standard,
    RedGreen,
}

#[derive(Debug, Serialize, Deserialize)]
struct StoredVerificationSet {
    commands: Vec<Vec<String>>,
    paths: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize)]
struct StoredSignal {
    id: String,
    seq: u64,
    sender: StoredAuthoritySnapshot,
    subject: StoredSubjectRef,
    body: StoredSignalBody,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum StoredSubjectRef {
    Bead { id: String },
    Assignment { id: String },
    Attempt { id: String },
    Scope { scope: StoredScopeExpr },
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum StoredSignalBody {
    Directive {
        assignment: String,
        attempt: String,
        directive: StoredDirectiveKind,
    },
    Report {
        attempt: String,
        report: StoredReportKind,
    },
    Request {
        recipient: String,
        request: StoredRequestKind,
        ask: String,
    },
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum StoredDirectiveKind {
    Amend { instruction: String },
    Pause { reason: String },
    Abort { reason: String },
    Answer { report: String, answer: String },
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum StoredReportKind {
    Progress {
        phase: StoredSemanticPhase,
        summary: Option<String>,
    },
    BlockedWithReason {
        reason: String,
    },
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum StoredSemanticPhase {
    Claimed,
    Verifying,
    HandingOff,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum StoredRequestKind {
    Arbitration,
    AuthorityTransfer,
    Reconciliation,
}

#[derive(Debug, Serialize, Deserialize)]
struct StoredResponseAction {
    seq: u64,
    kind: StoredResponseKind,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum StoredResponseKind {
    WorkerAction {
        attempt: String,
        responds_to: Option<String>,
    },
    DirectiveCommitted {
        attempt: String,
        directive: String,
    },
    FencedDecision {
        responds_to: Option<String>,
    },
    TerminalAttemptAction {
        attempt: String,
        abort_consistent: bool,
    },
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum StoredReportOutcome {
    Recorded { signal: Box<StoredSignal> },
    Refused { reason: StoredDirectiveGateRefusal },
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum StoredEvidenceOutcome {
    Recorded,
    Refused { reason: StoredDirectiveGateRefusal },
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum StoredDirectiveGateRefusal {
    AmendUndischarged,
    PauseInForce,
    AbortInForce,
}

#[derive(Debug, Serialize, Deserialize)]
struct StoredEvidenceRecord {
    operation: String,
    attempt: String,
    evidence: StoredEvidence,
}

#[derive(Debug, Serialize, Deserialize)]
struct StoredEvidence {
    argv: Vec<String>,
    verification: StoredVerificationSet,
    exit_code: i32,
    outcome: StoredVerificationOutcome,
    commit: String,
    workspace_before: String,
    workspace_after: String,
    overlay: Option<StoredOverlayCapture>,
    artifacts: Vec<StoredOverlayFile>,
    environment_fingerprint: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum StoredVerificationOutcome {
    Pass,
    AssertFail,
    ExecutionError,
}

#[derive(Debug, Serialize, Deserialize)]
struct StoredOverlayCapture {
    declared_base: String,
    files: Vec<StoredOverlayFile>,
}

#[derive(Debug, Serialize, Deserialize)]
struct StoredOverlayFile {
    path: String,
    digest: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct StoredHandoffRecord {
    id: String,
    attempt: String,
    commit: String,
    expected_base: String,
    clean_tree: String,
    changed_paths: Vec<String>,
    evidence_operations: Vec<String>,
    attestation: String,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum StoredSubmissionOutcome {
    Recorded { handoff: String },
    Refused { reason: StoredSubmissionRefusal },
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum StoredSubmissionRefusal {
    DirtyWorktree { paths: Vec<String> },
    MissingEvidence,
    EvidenceWrongCommit,
    FailingOutcome,
    EditScopeViolation { paths: Vec<String> },
    Directive { reason: StoredDirectiveGateRefusal },
    RedGreen { reason: StoredPairRefusal },
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum StoredPairRefusal {
    RedMissing,
    RedNotOverlay,
    RedWrongSet,
    GreenWrongSet,
    RedWrongBase,
    RedActuallyPassed,
    RedErrored,
    OverlayOutsidePolicy { path: String },
    RedStale { path: String },
    GreenMissing,
    GreenNotNative,
    GreenWrongCommit,
    GreenNotPass { outcome: StoredVerificationOutcome },
}

#[derive(Debug, Serialize, Deserialize)]
struct StoredDecisionRecord {
    operation: String,
    assignment: String,
    authority: StoredAuthoritySnapshot,
    kind: StoredDecisionKind,
    resolves: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum StoredDecisionKind {
    Accept {
        handoff: String,
        reason: String,
    },
    Reject {
        handoff: String,
        reason: String,
    },
    Cancel {
        reason: String,
    },
    Revoke {
        attempt: String,
        reason: String,
    },
    Reclaim {
        attempt: String,
        reason: String,
    },
    TransferAuthority {
        to: StoredDecisionActor,
        reason: String,
    },
}

#[derive(Debug, Serialize, Deserialize)]
struct StoredEnvelope {
    content: String,
    content_hash: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct StoredPendingApplication {
    operation: String,
    assignment: String,
    bead: String,
    projection: StoredWorkProjection,
    committed_at: u64,
    authorized_revision: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum StoredWorkProjection {
    MarkInProgress,
    Close { reason: StoredCloseReason },
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum StoredCloseReason {
    AcceptedHandoff,
    CancelledObsolete,
}

#[derive(Debug, Serialize, Deserialize)]
struct StoredApplicationAttempt {
    id: String,
    target: String,
    outcome: StoredApplicationOutcome,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum StoredApplicationOutcome {
    Applied {
        before: String,
        after: String,
    },
    #[serde(alias = "effect_already_present")]
    FoundPresent {
        status: StoredWorkStatus,
        revision: String,
    },
    ObservedAfterAmbiguous {
        status: StoredWorkStatus,
        revision: String,
    },
    Failed {
        error: StoredWorkError,
    },
    Ambiguous,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum StoredWorkStatus {
    Open,
    InProgress,
    Closed { reason: StoredObservedCloseReason },
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum StoredObservedCloseReason {
    AcceptedHandoff,
    CancelledObsolete,
    UnrecognizedProviderReason,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum StoredWorkError {
    ProviderUnavailable,
    Incompatible,
    Corrupt,
    Busy,
    MalformedOutput,
    NotFound,
    RevisionConflict,
    AmbiguousOutcome,
    BeadParked,
    ScopeLabelMalformed { label: String },
    ScopeLabelConflict { key: String },
}

#[derive(Debug, Serialize, Deserialize)]
struct StoredApplicationReceipt {
    target: String,
    attempt: String,
    after: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct StoredAuditEvent {
    seq: u64,
    at: u64,
    initiator: StoredAuditInitiator,
    operation: StoredAuditOperation,
    subject: StoredAuditSubject,
    kind: StoredAuditKind,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum StoredAuditOperation {
    Operation { id: String },
    Signal { id: String },
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum StoredAuditInitiator {
    Authority {
        authority: StoredAuthoritySnapshot,
    },
    WorkerBinding {
        actor: StoredDecisionActor,
        assignment: String,
        attempt: String,
    },
    OperatorChannel,
    SystemProjection {
        authorizing: String,
    },
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum StoredAuditSubject {
    Workflow { subject: StoredSubjectRef },
    ActorProfile { actor: String, profile: String },
    Launch { subject: StoredLaunchSubject },
    Projection { operation: String },
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
enum StoredAuditKind {
    AssignmentOpened,
    AttemptOpened,
    DecisionRecorded {
        kind: StoredAuditDecisionKind,
    },
    ProfileActivated {
        case: StoredAuditActivationCase,
    },
    ProfileDeactivated,
    DirectiveAppended {
        signal: String,
    },
    RequestAppended {
        signal: String,
    },
    ReportRecorded {
        signal: String,
    },
    ReportRefused {
        reason: StoredDirectiveGateRefusal,
    },
    EvidenceRecorded,
    EvidenceRefused {
        reason: StoredDirectiveGateRefusal,
    },
    HandoffRecorded {
        handoff: String,
    },
    HandoffRefused {
        reason: StoredAuditSubmissionRefusal,
    },
    LeaseRenewed,
    AttemptAborted,
    EnvelopePersisted,
    RuntimeHandleBound,
    RuntimeHandleUnbound,
    RuntimeObservationRecorded,
    ApplicationAttemptRecorded {
        outcome: StoredAuditApplicationOutcome,
    },
    ApplicationReceiptRecorded,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum StoredAuditDecisionKind {
    Accept,
    Reject,
    Cancel,
    Revoke,
    Reclaim,
    TransferAuthority,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum StoredAuditActivationCase {
    OperatorBootstrap,
    ActorAuthorizedRotation,
    OperatorRecovery,
    OperatorOrchestratorEnrolment,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum StoredAuditSubmissionRefusal {
    DirtyWorktree,
    MissingEvidence,
    EvidenceWrongCommit,
    FailingOutcome,
    EditScopeViolation,
    Directive { reason: StoredDirectiveGateRefusal },
    RedGreen,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum StoredAuditApplicationOutcome {
    Applied,
    #[serde(alias = "effect_already_present")]
    FoundPresent,
    ObservedAfterAmbiguous,
    Failed,
    Ambiguous,
}

#[derive(Debug, Serialize, Deserialize)]
struct StoredRuntimeObservation {
    reporter: StoredAuthoritySnapshot,
    subject: StoredLaunchSubject,
    observed_at: u64,
    liveness: StoredLivenessKind,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum StoredLaunchSubject {
    WorkerAttempt {
        attempt: String,
        credential: String,
    },
    ActorActivation {
        actor: String,
        profile: String,
        generation: String,
        credential: String,
    },
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum StoredLivenessKind {
    Starting,
    Running,
    Idle,
    Blocked,
    Exited,
    NotFound,
    Unavailable,
    Unknown,
    StaleGeneration,
}

fn invalid(kind: &str, value: impl core::fmt::Display) -> StoredError {
    StoredError::Invalid(format!("invalid {kind}: {value}"))
}

macro_rules! parse_id {
    ($ty:ty, $value:expr, $kind:literal) => {
        <$ty>::new(&$value).map_err(|_| invalid($kind, &$value))
    };
}

fn content_hash(value: String) -> Result<ContentHash, StoredError> {
    ContentHash::new(&value).map_err(|_| invalid("content hash", value))
}

fn commit_id(value: String) -> Result<CommitId, StoredError> {
    CommitId::new(&value).map_err(|_| invalid("commit id", value))
}

fn workspace_digest(value: String) -> Result<WorkspaceDigest, StoredError> {
    WorkspaceDigest::new(&value).map_err(|_| invalid("workspace digest", value))
}

fn work_revision(value: String) -> Result<WorkRevision, StoredError> {
    Ok(WorkRevision(content_hash(value)?))
}

fn work_path(value: String) -> Result<WorkPath, StoredError> {
    WorkPath::new(&value).map_err(|_| invalid("work path", value))
}

fn bounded_text(value: String) -> Result<BoundedText, StoredError> {
    BoundedText::new(&value).map_err(|_| invalid("bounded text", value))
}

fn decision_reason(value: String) -> Result<DecisionReason, StoredError> {
    DecisionReason::new(&value).map_err(|_| invalid("decision reason", value))
}

impl From<AuthorityClass> for StoredAuthorityClass {
    fn from(value: AuthorityClass) -> Self {
        match value {
            AuthorityClass::Orchestrator => Self::Orchestrator,
            AuthorityClass::Worker => Self::Worker,
        }
    }
}

impl From<StoredAuthorityClass> for AuthorityClass {
    fn from(value: StoredAuthorityClass) -> Self {
        match value {
            StoredAuthorityClass::Orchestrator => Self::Orchestrator,
            StoredAuthorityClass::Worker => Self::Worker,
        }
    }
}

impl From<&CredentialBinding> for StoredCredentialBinding {
    fn from(value: &CredentialBinding) -> Self {
        Self {
            credential: value.credential.as_str().to_owned(),
            digest: value.digest.as_str().to_owned(),
            actor: value.actor.as_str().to_owned(),
            profile: value.profile.as_str().to_owned(),
            assignment: value.assignment.as_ref().map(|id| id.as_str().to_owned()),
            revoked: value.revoked,
        }
    }
}

impl TryFrom<StoredCredentialBinding> for CredentialBinding {
    type Error = StoredError;

    fn try_from(value: StoredCredentialBinding) -> Result<Self, Self::Error> {
        Ok(Self {
            credential: parse_id!(CredentialId, value.credential, "credential id")?,
            digest: content_hash(value.digest)?,
            actor: parse_id!(ActorId, value.actor, "actor id")?,
            profile: parse_id!(ProfileName, value.profile, "profile name")?,
            assignment: value
                .assignment
                .map(|id| parse_id!(AssignmentId, id, "assignment id"))
                .transpose()?,
            revoked: value.revoked,
        })
    }
}

impl From<&DecisionActor> for StoredDecisionActor {
    fn from(value: &DecisionActor) -> Self {
        Self {
            actor: value.actor.as_str().to_owned(),
            class: value.class.into(),
            profile: value.profile.as_str().to_owned(),
            profile_hash: value.profile_hash.as_str().to_owned(),
        }
    }
}

impl TryFrom<StoredDecisionActor> for DecisionActor {
    type Error = StoredError;

    fn try_from(value: StoredDecisionActor) -> Result<Self, Self::Error> {
        Ok(Self {
            actor: parse_id!(ActorId, value.actor, "actor id")?,
            class: value.class.into(),
            profile: parse_id!(ProfileName, value.profile, "profile name")?,
            profile_hash: content_hash(value.profile_hash)?,
        })
    }
}

fn scope_keys(canonical: &str) -> Vec<String> {
    if canonical == "*" {
        return Vec::new();
    }
    let mut keys = BTreeSet::new();
    for selector in canonical.split('|') {
        for atom in selector.split('&') {
            let atom = atom.trim();
            let key = atom
                .split_once("!=")
                .map(|(key, _)| key)
                .or_else(|| atom.split_once('=').map(|(key, _)| key))
                .unwrap_or(atom)
                .trim();
            keys.insert(key.to_owned());
        }
    }
    keys.into_iter().collect()
}

impl From<&ScopeExpr> for StoredScopeExpr {
    fn from(value: &ScopeExpr) -> Self {
        let canonical = value.canonical();
        Self {
            declared_keys: scope_keys(&canonical),
            canonical,
        }
    }
}

impl TryFrom<StoredScopeExpr> for ScopeExpr {
    type Error = StoredError;

    fn try_from(value: StoredScopeExpr) -> Result<Self, Self::Error> {
        if value.declared_keys != scope_keys(&value.canonical) {
            return Err(invalid(
                "scope key set",
                "does not match the canonical expression",
            ));
        }
        let declared: Vec<ScopeKey> = value
            .declared_keys
            .into_iter()
            .map(|key| ScopeKey::new(&key).map_err(|_| invalid("scope key", key)))
            .collect::<Result<_, _>>()?;
        ScopeExpr::parse(&value.canonical, &declared)
            .map_err(|_| invalid("scope expression", value.canonical))
    }
}

impl From<&AuthoritySnapshot> for StoredAuthoritySnapshot {
    fn from(value: &AuthoritySnapshot) -> Self {
        Self {
            actor: (&value.actor).into(),
            capability: value.capability.as_str().to_owned(),
            scope: (&value.scope).into(),
        }
    }
}

impl TryFrom<StoredAuthoritySnapshot> for AuthoritySnapshot {
    type Error = StoredError;

    fn try_from(value: StoredAuthoritySnapshot) -> Result<Self, Self::Error> {
        Ok(Self {
            actor: value.actor.try_into()?,
            capability: parse_id!(CapabilityId, value.capability, "capability id")?,
            scope: value.scope.try_into()?,
        })
    }
}

impl From<&VerificationSet> for StoredVerificationSet {
    fn from(value: &VerificationSet) -> Self {
        Self {
            commands: value
                .commands()
                .iter()
                .map(|argv| argv.iter().map(str::to_owned).collect())
                .collect(),
            paths: value
                .paths()
                .iter()
                .map(|path| path.as_str().to_owned())
                .collect(),
        }
    }
}

impl TryFrom<StoredVerificationSet> for VerificationSet {
    type Error = StoredError;

    fn try_from(value: StoredVerificationSet) -> Result<Self, Self::Error> {
        let commands = value
            .commands
            .into_iter()
            .map(|items| Argv::new(items).map_err(|_| invalid("verification argv", "shape")))
            .collect::<Result<_, _>>()?;
        let paths = PathSet::new(
            value
                .paths
                .into_iter()
                .map(work_path)
                .collect::<Result<_, _>>()?,
        )
        .map_err(|_| invalid("verification paths", "shape"))?;
        VerificationSet::new(commands, paths).map_err(|_| invalid("verification set", "shape"))
    }
}

impl From<&AcceptancePolicy> for StoredAcceptancePolicy {
    fn from(value: &AcceptancePolicy) -> Self {
        Self {
            verification: (&value.verification).into(),
            form: match value.form {
                PolicyForm::Standard => StoredPolicyForm::Standard,
                PolicyForm::RedGreen => StoredPolicyForm::RedGreen,
            },
        }
    }
}

impl TryFrom<StoredAcceptancePolicy> for AcceptancePolicy {
    type Error = StoredError;

    fn try_from(value: StoredAcceptancePolicy) -> Result<Self, Self::Error> {
        Ok(Self {
            verification: value.verification.try_into()?,
            form: match value.form {
                StoredPolicyForm::Standard => PolicyForm::Standard,
                StoredPolicyForm::RedGreen => PolicyForm::RedGreen,
            },
        })
    }
}

impl From<&AssignmentRecord> for StoredAssignmentRecord {
    fn from(value: &AssignmentRecord) -> Self {
        Self {
            id: value.id.as_str().to_owned(),
            bead: value.bead.as_str().to_owned(),
            bead_content_hash: value.bead_content_hash.as_str().to_owned(),
            scope_map: value
                .scope_map
                .pairs()
                .map(|(key, value)| (key.as_str().to_owned(), value.as_str().to_owned()))
                .collect(),
            worker: (&value.worker).into(),
            decision_actor: (&value.decision_actor).into(),
            edit_scope: value
                .edit_scope
                .entries()
                .iter()
                .map(|path| path.as_str().to_owned())
                .collect(),
            acceptance: (&value.acceptance).into(),
            attempt_cap: value.attempt_policy.cap.map(AttemptCap::value),
            declared_base: value.declared_base.as_str().to_owned(),
        }
    }
}

impl TryFrom<StoredAssignmentRecord> for AssignmentRecord {
    type Error = StoredError;

    fn try_from(value: StoredAssignmentRecord) -> Result<Self, Self::Error> {
        let scope_map = ScopeMap::new(
            value
                .scope_map
                .into_iter()
                .map(|(key, value)| {
                    Ok((
                        ScopeKey::new(&key).map_err(|_| invalid("scope key", key))?,
                        ScopeValue::new(&value).map_err(|_| invalid("scope value", value))?,
                    ))
                })
                .collect::<Result<_, StoredError>>()?,
        )
        .map_err(|_| invalid("scope map", "shape"))?;
        let edit_scope = EditScope::new(
            value
                .edit_scope
                .into_iter()
                .map(work_path)
                .collect::<Result<_, _>>()?,
        )
        .map_err(|_| invalid("edit scope", "shape"))?;
        let cap = value
            .attempt_cap
            .map(|cap| AttemptCap::new(cap).map_err(|_| invalid("attempt cap", cap)))
            .transpose()?;
        Ok(Self {
            id: parse_id!(AssignmentId, value.id, "assignment id")?,
            bead: parse_id!(BeadId, value.bead, "bead id")?,
            bead_content_hash: content_hash(value.bead_content_hash)?,
            scope_map,
            worker: value.worker.try_into()?,
            decision_actor: value.decision_actor.try_into()?,
            edit_scope,
            acceptance: value.acceptance.try_into()?,
            attempt_policy: AttemptPolicy { cap },
            declared_base: commit_id(value.declared_base)?,
        })
    }
}

impl From<&AttemptRecord> for StoredAttemptRecord {
    fn from(value: &AttemptRecord) -> Self {
        Self {
            id: value.id.as_str().to_owned(),
            assignment: value.assignment.as_str().to_owned(),
            token: value.lease.token.0,
            expires_at: value.lease.expires_at.0,
        }
    }
}

impl TryFrom<StoredAttemptRecord> for AttemptRecord {
    type Error = StoredError;

    fn try_from(value: StoredAttemptRecord) -> Result<Self, Self::Error> {
        Ok(Self {
            id: parse_id!(AttemptId, value.id, "attempt id")?,
            assignment: parse_id!(AssignmentId, value.assignment, "assignment id")?,
            lease: Lease {
                token: FencingToken(value.token),
                expires_at: Timestamp(value.expires_at),
            },
        })
    }
}

impl From<&SubjectRef> for StoredSubjectRef {
    fn from(value: &SubjectRef) -> Self {
        match value {
            SubjectRef::Bead(id) => Self::Bead {
                id: id.as_str().to_owned(),
            },
            SubjectRef::Assignment(id) => Self::Assignment {
                id: id.as_str().to_owned(),
            },
            SubjectRef::Attempt(id) => Self::Attempt {
                id: id.as_str().to_owned(),
            },
            SubjectRef::Scope(scope) => Self::Scope {
                scope: scope.into(),
            },
        }
    }
}

impl TryFrom<StoredSubjectRef> for SubjectRef {
    type Error = StoredError;

    fn try_from(value: StoredSubjectRef) -> Result<Self, Self::Error> {
        Ok(match value {
            StoredSubjectRef::Bead { id } => Self::Bead(parse_id!(BeadId, id, "bead id")?),
            StoredSubjectRef::Assignment { id } => {
                Self::Assignment(parse_id!(AssignmentId, id, "assignment id")?)
            }
            StoredSubjectRef::Attempt { id } => {
                Self::Attempt(parse_id!(AttemptId, id, "attempt id")?)
            }
            StoredSubjectRef::Scope { scope } => Self::Scope(scope.try_into()?),
        })
    }
}

impl From<&DirectiveKind> for StoredDirectiveKind {
    fn from(value: &DirectiveKind) -> Self {
        match value {
            DirectiveKind::Amend { instruction } => Self::Amend {
                instruction: instruction.as_str().to_owned(),
            },
            DirectiveKind::Pause { reason } => Self::Pause {
                reason: reason.as_str().to_owned(),
            },
            DirectiveKind::Abort { reason } => Self::Abort {
                reason: reason.as_str().to_owned(),
            },
            DirectiveKind::Answer { report, answer } => Self::Answer {
                report: report.as_str().to_owned(),
                answer: answer.as_str().to_owned(),
            },
        }
    }
}

impl TryFrom<StoredDirectiveKind> for DirectiveKind {
    type Error = StoredError;

    fn try_from(value: StoredDirectiveKind) -> Result<Self, Self::Error> {
        Ok(match value {
            StoredDirectiveKind::Amend { instruction } => Self::Amend {
                instruction: bounded_text(instruction)?,
            },
            StoredDirectiveKind::Pause { reason } => Self::Pause {
                reason: bounded_text(reason)?,
            },
            StoredDirectiveKind::Abort { reason } => Self::Abort {
                reason: bounded_text(reason)?,
            },
            StoredDirectiveKind::Answer { report, answer } => Self::Answer {
                report: parse_id!(SignalId, report, "signal id")?,
                answer: bounded_text(answer)?,
            },
        })
    }
}

impl From<SemanticPhase> for StoredSemanticPhase {
    fn from(value: SemanticPhase) -> Self {
        match value {
            SemanticPhase::Claimed => Self::Claimed,
            SemanticPhase::Verifying => Self::Verifying,
            SemanticPhase::HandingOff => Self::HandingOff,
        }
    }
}

impl From<StoredSemanticPhase> for SemanticPhase {
    fn from(value: StoredSemanticPhase) -> Self {
        match value {
            StoredSemanticPhase::Claimed => Self::Claimed,
            StoredSemanticPhase::Verifying => Self::Verifying,
            StoredSemanticPhase::HandingOff => Self::HandingOff,
        }
    }
}

impl From<&ReportKind> for StoredReportKind {
    fn from(value: &ReportKind) -> Self {
        match value {
            ReportKind::Progress { phase, summary } => Self::Progress {
                phase: (*phase).into(),
                summary: summary.as_ref().map(|text| text.as_str().to_owned()),
            },
            ReportKind::BlockedWithReason { reason } => Self::BlockedWithReason {
                reason: reason.as_str().to_owned(),
            },
        }
    }
}

impl TryFrom<StoredReportKind> for ReportKind {
    type Error = StoredError;

    fn try_from(value: StoredReportKind) -> Result<Self, Self::Error> {
        Ok(match value {
            StoredReportKind::Progress { phase, summary } => Self::Progress {
                phase: phase.into(),
                summary: summary.map(bounded_text).transpose()?,
            },
            StoredReportKind::BlockedWithReason { reason } => Self::BlockedWithReason {
                reason: bounded_text(reason)?,
            },
        })
    }
}

impl From<RequestKind> for StoredRequestKind {
    fn from(value: RequestKind) -> Self {
        match value {
            RequestKind::Arbitration => Self::Arbitration,
            RequestKind::AuthorityTransfer => Self::AuthorityTransfer,
            RequestKind::Reconciliation => Self::Reconciliation,
        }
    }
}

impl From<StoredRequestKind> for RequestKind {
    fn from(value: StoredRequestKind) -> Self {
        match value {
            StoredRequestKind::Arbitration => Self::Arbitration,
            StoredRequestKind::AuthorityTransfer => Self::AuthorityTransfer,
            StoredRequestKind::Reconciliation => Self::Reconciliation,
        }
    }
}

impl From<&SignalBody> for StoredSignalBody {
    fn from(value: &SignalBody) -> Self {
        match value {
            SignalBody::Directive {
                assignment,
                attempt,
                kind,
            } => Self::Directive {
                assignment: assignment.as_str().to_owned(),
                attempt: attempt.as_str().to_owned(),
                directive: kind.into(),
            },
            SignalBody::Report { attempt, kind } => Self::Report {
                attempt: attempt.as_str().to_owned(),
                report: kind.into(),
            },
            SignalBody::Request {
                recipient,
                kind,
                ask,
            } => Self::Request {
                recipient: recipient.as_str().to_owned(),
                request: (*kind).into(),
                ask: ask.as_str().to_owned(),
            },
        }
    }
}

impl TryFrom<StoredSignalBody> for SignalBody {
    type Error = StoredError;

    fn try_from(value: StoredSignalBody) -> Result<Self, Self::Error> {
        Ok(match value {
            StoredSignalBody::Directive {
                assignment,
                attempt,
                directive,
            } => Self::Directive {
                assignment: parse_id!(AssignmentId, assignment, "assignment id")?,
                attempt: parse_id!(AttemptId, attempt, "attempt id")?,
                kind: directive.try_into()?,
            },
            StoredSignalBody::Report { attempt, report } => Self::Report {
                attempt: parse_id!(AttemptId, attempt, "attempt id")?,
                kind: report.try_into()?,
            },
            StoredSignalBody::Request {
                recipient,
                request,
                ask,
            } => Self::Request {
                recipient: parse_id!(ActorId, recipient, "actor id")?,
                kind: request.into(),
                ask: bounded_text(ask)?,
            },
        })
    }
}

impl From<&Signal> for StoredSignal {
    fn from(value: &Signal) -> Self {
        Self {
            id: value.id.as_str().to_owned(),
            seq: value.seq.0,
            sender: (&value.sender).into(),
            subject: (&value.subject).into(),
            body: (&value.body).into(),
        }
    }
}

impl TryFrom<StoredSignal> for Signal {
    type Error = StoredError;

    fn try_from(value: StoredSignal) -> Result<Self, Self::Error> {
        let signal = Self {
            id: parse_id!(SignalId, value.id, "signal id")?,
            seq: Seq(value.seq),
            sender: value.sender.try_into()?,
            subject: value.subject.try_into()?,
            body: value.body.try_into()?,
        };
        abacus_core::signal::validate_subject(&signal.body, &signal.subject)
            .map_err(|_| invalid("signal subject", signal.id.as_str()))?;
        Ok(signal)
    }
}

impl From<&ResponseAction> for StoredResponseAction {
    fn from(value: &ResponseAction) -> Self {
        Self {
            seq: value.seq.0,
            kind: match &value.kind {
                ResponseKind::WorkerAction {
                    attempt,
                    responds_to,
                } => StoredResponseKind::WorkerAction {
                    attempt: attempt.as_str().to_owned(),
                    responds_to: responds_to.as_ref().map(|id| id.as_str().to_owned()),
                },
                ResponseKind::DirectiveCommitted { attempt, directive } => {
                    StoredResponseKind::DirectiveCommitted {
                        attempt: attempt.as_str().to_owned(),
                        directive: directive.as_str().to_owned(),
                    }
                }
                ResponseKind::FencedDecision { responds_to } => {
                    StoredResponseKind::FencedDecision {
                        responds_to: responds_to.as_ref().map(|id| id.as_str().to_owned()),
                    }
                }
                ResponseKind::TerminalAttemptAction {
                    attempt,
                    abort_consistent,
                } => StoredResponseKind::TerminalAttemptAction {
                    attempt: attempt.as_str().to_owned(),
                    abort_consistent: *abort_consistent,
                },
            },
        }
    }
}

impl TryFrom<StoredResponseAction> for ResponseAction {
    type Error = StoredError;

    fn try_from(value: StoredResponseAction) -> Result<Self, Self::Error> {
        Ok(Self {
            seq: Seq(value.seq),
            kind: match value.kind {
                StoredResponseKind::WorkerAction {
                    attempt,
                    responds_to,
                } => ResponseKind::WorkerAction {
                    attempt: parse_id!(AttemptId, attempt, "attempt id")?,
                    responds_to: responds_to
                        .map(|id| parse_id!(SignalId, id, "signal id"))
                        .transpose()?,
                },
                StoredResponseKind::DirectiveCommitted { attempt, directive } => {
                    ResponseKind::DirectiveCommitted {
                        attempt: parse_id!(AttemptId, attempt, "attempt id")?,
                        directive: parse_id!(SignalId, directive, "signal id")?,
                    }
                }
                StoredResponseKind::FencedDecision { responds_to } => {
                    ResponseKind::FencedDecision {
                        responds_to: responds_to
                            .map(|id| parse_id!(SignalId, id, "signal id"))
                            .transpose()?,
                    }
                }
                StoredResponseKind::TerminalAttemptAction {
                    attempt,
                    abort_consistent,
                } => ResponseKind::TerminalAttemptAction {
                    attempt: parse_id!(AttemptId, attempt, "attempt id")?,
                    abort_consistent,
                },
            },
        })
    }
}

impl From<DirectiveGateRefusal> for StoredDirectiveGateRefusal {
    fn from(value: DirectiveGateRefusal) -> Self {
        match value {
            DirectiveGateRefusal::AmendUndischarged => Self::AmendUndischarged,
            DirectiveGateRefusal::PauseInForce => Self::PauseInForce,
            DirectiveGateRefusal::AbortInForce => Self::AbortInForce,
        }
    }
}

impl From<StoredDirectiveGateRefusal> for DirectiveGateRefusal {
    fn from(value: StoredDirectiveGateRefusal) -> Self {
        match value {
            StoredDirectiveGateRefusal::AmendUndischarged => Self::AmendUndischarged,
            StoredDirectiveGateRefusal::PauseInForce => Self::PauseInForce,
            StoredDirectiveGateRefusal::AbortInForce => Self::AbortInForce,
        }
    }
}

impl From<&ReportOutcome> for StoredReportOutcome {
    fn from(value: &ReportOutcome) -> Self {
        match value {
            ReportOutcome::Recorded { signal } => Self::Recorded {
                signal: Box::new(signal.as_ref().into()),
            },
            ReportOutcome::Refused { reason } => Self::Refused {
                reason: (*reason).into(),
            },
        }
    }
}

impl TryFrom<StoredReportOutcome> for ReportOutcome {
    type Error = StoredError;

    fn try_from(value: StoredReportOutcome) -> Result<Self, Self::Error> {
        Ok(match value {
            StoredReportOutcome::Recorded { signal } => Self::Recorded {
                signal: Box::new((*signal).try_into()?),
            },
            StoredReportOutcome::Refused { reason } => Self::Refused {
                reason: reason.into(),
            },
        })
    }
}

impl From<EvidenceOutcome> for StoredEvidenceOutcome {
    fn from(value: EvidenceOutcome) -> Self {
        match value {
            EvidenceOutcome::Recorded => Self::Recorded,
            EvidenceOutcome::Refused { reason } => Self::Refused {
                reason: reason.into(),
            },
        }
    }
}

impl From<StoredEvidenceOutcome> for EvidenceOutcome {
    fn from(value: StoredEvidenceOutcome) -> Self {
        match value {
            StoredEvidenceOutcome::Recorded => Self::Recorded,
            StoredEvidenceOutcome::Refused { reason } => Self::Refused {
                reason: reason.into(),
            },
        }
    }
}

impl From<VerificationOutcome> for StoredVerificationOutcome {
    fn from(value: VerificationOutcome) -> Self {
        match value {
            VerificationOutcome::Pass => Self::Pass,
            VerificationOutcome::AssertFail => Self::AssertFail,
            VerificationOutcome::ExecutionError => Self::ExecutionError,
        }
    }
}

impl From<StoredVerificationOutcome> for VerificationOutcome {
    fn from(value: StoredVerificationOutcome) -> Self {
        match value {
            StoredVerificationOutcome::Pass => Self::Pass,
            StoredVerificationOutcome::AssertFail => Self::AssertFail,
            StoredVerificationOutcome::ExecutionError => Self::ExecutionError,
        }
    }
}

impl From<&OverlayFile> for StoredOverlayFile {
    fn from(value: &OverlayFile) -> Self {
        Self {
            path: value.path.as_str().to_owned(),
            digest: value.digest.as_str().to_owned(),
        }
    }
}

impl TryFrom<StoredOverlayFile> for OverlayFile {
    type Error = StoredError;

    fn try_from(value: StoredOverlayFile) -> Result<Self, Self::Error> {
        Ok(Self {
            path: work_path(value.path)?,
            digest: content_hash(value.digest)?,
        })
    }
}

fn stored_files(value: &FileDigestSet) -> Vec<StoredOverlayFile> {
    value.iter().map(Into::into).collect()
}

fn file_set(value: Vec<StoredOverlayFile>) -> Result<FileDigestSet, StoredError> {
    FileDigestSet::new(
        value
            .into_iter()
            .map(TryInto::try_into)
            .collect::<Result<_, _>>()?,
    )
    .map_err(|_| invalid("file digest set", "shape"))
}

impl From<&Evidence> for StoredEvidence {
    fn from(value: &Evidence) -> Self {
        Self {
            argv: value.argv().iter().map(str::to_owned).collect(),
            verification: value.verification().into(),
            exit_code: value.exit_code,
            outcome: value.outcome.into(),
            commit: value.commit.as_str().to_owned(),
            workspace_before: value.workspace_before.as_str().to_owned(),
            workspace_after: value.workspace_after.as_str().to_owned(),
            overlay: value.overlay.as_ref().map(|overlay| StoredOverlayCapture {
                declared_base: overlay.declared_base.as_str().to_owned(),
                files: stored_files(&overlay.files),
            }),
            artifacts: stored_files(&value.artifacts),
            environment_fingerprint: value
                .environment_fingerprint
                .as_ref()
                .map(|hash| hash.as_str().to_owned()),
        }
    }
}

impl TryFrom<StoredEvidence> for Evidence {
    type Error = StoredError;

    fn try_from(value: StoredEvidence) -> Result<Self, Self::Error> {
        let argv = Argv::new(value.argv).map_err(|_| invalid("evidence argv", "shape"))?;
        let verification = value.verification.try_into()?;
        let overlay = value
            .overlay
            .map(|overlay| {
                Ok::<OverlayCapture, StoredError>(OverlayCapture {
                    declared_base: commit_id(overlay.declared_base)?,
                    files: file_set(overlay.files)?,
                })
            })
            .transpose()?;
        Evidence::new(
            argv,
            verification,
            value.exit_code,
            value.outcome.into(),
            commit_id(value.commit)?,
            workspace_digest(value.workspace_before)?,
            workspace_digest(value.workspace_after)?,
            overlay,
            file_set(value.artifacts)?,
            value
                .environment_fingerprint
                .map(content_hash)
                .transpose()?,
        )
        .map_err(|_| invalid("evidence", "command not in verification set"))
    }
}

impl From<&EvidenceRecord> for StoredEvidenceRecord {
    fn from(value: &EvidenceRecord) -> Self {
        Self {
            operation: value.operation.as_str().to_owned(),
            attempt: value.attempt.as_str().to_owned(),
            evidence: (&value.evidence).into(),
        }
    }
}

impl TryFrom<StoredEvidenceRecord> for EvidenceRecord {
    type Error = StoredError;

    fn try_from(value: StoredEvidenceRecord) -> Result<Self, Self::Error> {
        Ok(Self {
            operation: parse_id!(OperationId, value.operation, "operation id")?,
            attempt: parse_id!(AttemptId, value.attempt, "attempt id")?,
            evidence: value.evidence.try_into()?,
        })
    }
}

impl From<&HandoffRecord> for StoredHandoffRecord {
    fn from(value: &HandoffRecord) -> Self {
        Self {
            id: value.id.as_str().to_owned(),
            attempt: value.attempt.as_str().to_owned(),
            commit: value.commit.as_str().to_owned(),
            expected_base: value.expected_base.as_str().to_owned(),
            clean_tree: value.clean_tree.as_str().to_owned(),
            changed_paths: value
                .changed_paths
                .iter()
                .map(|path| path.as_str().to_owned())
                .collect(),
            evidence_operations: value
                .evidence_operations
                .iter()
                .map(|operation| operation.as_str().to_owned())
                .collect(),
            attestation: value.attestation.as_str().to_owned(),
        }
    }
}

impl TryFrom<StoredHandoffRecord> for HandoffRecord {
    type Error = StoredError;

    fn try_from(value: StoredHandoffRecord) -> Result<Self, Self::Error> {
        Ok(Self {
            id: parse_id!(HandoffId, value.id, "handoff id")?,
            attempt: parse_id!(AttemptId, value.attempt, "attempt id")?,
            commit: commit_id(value.commit)?,
            expected_base: commit_id(value.expected_base)?,
            clean_tree: workspace_digest(value.clean_tree)?,
            changed_paths: PathSet::new(
                value
                    .changed_paths
                    .into_iter()
                    .map(work_path)
                    .collect::<Result<_, _>>()?,
            )
            .map_err(|_| invalid("changed path set", "shape"))?,
            evidence_operations: OperationSet::new(
                value
                    .evidence_operations
                    .into_iter()
                    .map(|id| parse_id!(OperationId, id, "operation id"))
                    .collect::<Result<_, _>>()?,
            )
            .map_err(|_| invalid("operation set", "shape"))?,
            attestation: content_hash(value.attestation)?,
        })
    }
}

impl From<VerificationOutcome> for StoredPairRefusal {
    fn from(outcome: VerificationOutcome) -> Self {
        Self::GreenNotPass {
            outcome: outcome.into(),
        }
    }
}

impl From<&PairRefusal> for StoredPairRefusal {
    fn from(value: &PairRefusal) -> Self {
        match value {
            PairRefusal::RedMissing => Self::RedMissing,
            PairRefusal::RedNotOverlay => Self::RedNotOverlay,
            PairRefusal::RedWrongSet => Self::RedWrongSet,
            PairRefusal::GreenWrongSet => Self::GreenWrongSet,
            PairRefusal::RedWrongBase => Self::RedWrongBase,
            PairRefusal::RedActuallyPassed => Self::RedActuallyPassed,
            PairRefusal::RedErrored => Self::RedErrored,
            PairRefusal::OverlayOutsidePolicy(path) => Self::OverlayOutsidePolicy {
                path: path.as_str().to_owned(),
            },
            PairRefusal::RedStale(path) => Self::RedStale {
                path: path.as_str().to_owned(),
            },
            PairRefusal::GreenMissing => Self::GreenMissing,
            PairRefusal::GreenNotNative => Self::GreenNotNative,
            PairRefusal::GreenWrongCommit => Self::GreenWrongCommit,
            PairRefusal::GreenNotPass(outcome) => Self::GreenNotPass {
                outcome: (*outcome).into(),
            },
        }
    }
}

impl TryFrom<StoredPairRefusal> for PairRefusal {
    type Error = StoredError;

    fn try_from(value: StoredPairRefusal) -> Result<Self, Self::Error> {
        Ok(match value {
            StoredPairRefusal::RedMissing => Self::RedMissing,
            StoredPairRefusal::RedNotOverlay => Self::RedNotOverlay,
            StoredPairRefusal::RedWrongSet => Self::RedWrongSet,
            StoredPairRefusal::GreenWrongSet => Self::GreenWrongSet,
            StoredPairRefusal::RedWrongBase => Self::RedWrongBase,
            StoredPairRefusal::RedActuallyPassed => Self::RedActuallyPassed,
            StoredPairRefusal::RedErrored => Self::RedErrored,
            StoredPairRefusal::OverlayOutsidePolicy { path } => {
                Self::OverlayOutsidePolicy(work_path(path)?)
            }
            StoredPairRefusal::RedStale { path } => Self::RedStale(work_path(path)?),
            StoredPairRefusal::GreenMissing => Self::GreenMissing,
            StoredPairRefusal::GreenNotNative => Self::GreenNotNative,
            StoredPairRefusal::GreenWrongCommit => Self::GreenWrongCommit,
            StoredPairRefusal::GreenNotPass { outcome } => Self::GreenNotPass(outcome.into()),
        })
    }
}

impl From<&SubmissionRefusalReason> for StoredSubmissionRefusal {
    fn from(value: &SubmissionRefusalReason) -> Self {
        match value {
            SubmissionRefusalReason::DirtyWorktree { paths } => Self::DirtyWorktree {
                paths: paths.iter().map(|path| path.as_str().to_owned()).collect(),
            },
            SubmissionRefusalReason::MissingEvidence => Self::MissingEvidence,
            SubmissionRefusalReason::EvidenceWrongCommit => Self::EvidenceWrongCommit,
            SubmissionRefusalReason::FailingOutcome => Self::FailingOutcome,
            SubmissionRefusalReason::EditScopeViolation { paths } => Self::EditScopeViolation {
                paths: paths.iter().map(|path| path.as_str().to_owned()).collect(),
            },
            SubmissionRefusalReason::Directive(reason) => Self::Directive {
                reason: (*reason).into(),
            },
            SubmissionRefusalReason::RedGreen(reason) => Self::RedGreen {
                reason: reason.into(),
            },
        }
    }
}

impl TryFrom<StoredSubmissionRefusal> for SubmissionRefusalReason {
    type Error = StoredError;

    fn try_from(value: StoredSubmissionRefusal) -> Result<Self, Self::Error> {
        let paths = |values: Vec<String>| -> Result<PathSet, StoredError> {
            PathSet::new(
                values
                    .into_iter()
                    .map(work_path)
                    .collect::<Result<_, _>>()?,
            )
            .map_err(|_| invalid("path set", "shape"))
        };
        Ok(match value {
            StoredSubmissionRefusal::DirtyWorktree { paths: values } => Self::DirtyWorktree {
                paths: paths(values)?,
            },
            StoredSubmissionRefusal::MissingEvidence => Self::MissingEvidence,
            StoredSubmissionRefusal::EvidenceWrongCommit => Self::EvidenceWrongCommit,
            StoredSubmissionRefusal::FailingOutcome => Self::FailingOutcome,
            StoredSubmissionRefusal::EditScopeViolation { paths: values } => {
                Self::EditScopeViolation {
                    paths: paths(values)?,
                }
            }
            StoredSubmissionRefusal::Directive { reason } => Self::Directive(reason.into()),
            StoredSubmissionRefusal::RedGreen { reason } => Self::RedGreen(reason.try_into()?),
        })
    }
}

impl From<&SubmissionOutcome> for StoredSubmissionOutcome {
    fn from(value: &SubmissionOutcome) -> Self {
        match value {
            SubmissionOutcome::Recorded { handoff } => Self::Recorded {
                handoff: handoff.as_str().to_owned(),
            },
            SubmissionOutcome::Refused { reason } => Self::Refused {
                reason: reason.into(),
            },
        }
    }
}

impl TryFrom<StoredSubmissionOutcome> for SubmissionOutcome {
    type Error = StoredError;

    fn try_from(value: StoredSubmissionOutcome) -> Result<Self, Self::Error> {
        Ok(match value {
            StoredSubmissionOutcome::Recorded { handoff } => Self::Recorded {
                handoff: parse_id!(HandoffId, handoff, "handoff id")?,
            },
            StoredSubmissionOutcome::Refused { reason } => Self::Refused {
                reason: reason.try_into()?,
            },
        })
    }
}

impl From<&DecisionKind> for StoredDecisionKind {
    fn from(value: &DecisionKind) -> Self {
        match value {
            DecisionKind::Accept { handoff, reason } => Self::Accept {
                handoff: handoff.as_str().to_owned(),
                reason: reason.as_str().to_owned(),
            },
            DecisionKind::Reject { handoff, reason } => Self::Reject {
                handoff: handoff.as_str().to_owned(),
                reason: reason.as_str().to_owned(),
            },
            DecisionKind::Cancel { reason } => Self::Cancel {
                reason: reason.as_str().to_owned(),
            },
            DecisionKind::Revoke { attempt, reason } => Self::Revoke {
                attempt: attempt.as_str().to_owned(),
                reason: reason.as_str().to_owned(),
            },
            DecisionKind::Reclaim { attempt, reason } => Self::Reclaim {
                attempt: attempt.as_str().to_owned(),
                reason: reason.as_str().to_owned(),
            },
            DecisionKind::TransferAuthority { to, reason } => Self::TransferAuthority {
                to: to.into(),
                reason: reason.as_str().to_owned(),
            },
        }
    }
}

impl TryFrom<StoredDecisionKind> for DecisionKind {
    type Error = StoredError;

    fn try_from(value: StoredDecisionKind) -> Result<Self, Self::Error> {
        Ok(match value {
            StoredDecisionKind::Accept { handoff, reason } => Self::Accept {
                handoff: parse_id!(HandoffId, handoff, "handoff id")?,
                reason: decision_reason(reason)?,
            },
            StoredDecisionKind::Reject { handoff, reason } => Self::Reject {
                handoff: parse_id!(HandoffId, handoff, "handoff id")?,
                reason: decision_reason(reason)?,
            },
            StoredDecisionKind::Cancel { reason } => Self::Cancel {
                reason: decision_reason(reason)?,
            },
            StoredDecisionKind::Revoke { attempt, reason } => Self::Revoke {
                attempt: parse_id!(AttemptId, attempt, "attempt id")?,
                reason: decision_reason(reason)?,
            },
            StoredDecisionKind::Reclaim { attempt, reason } => Self::Reclaim {
                attempt: parse_id!(AttemptId, attempt, "attempt id")?,
                reason: decision_reason(reason)?,
            },
            StoredDecisionKind::TransferAuthority { to, reason } => Self::TransferAuthority {
                to: to.try_into()?,
                reason: decision_reason(reason)?,
            },
        })
    }
}

impl From<&DecisionRecord> for StoredDecisionRecord {
    fn from(value: &DecisionRecord) -> Self {
        Self {
            operation: value.operation.as_str().to_owned(),
            assignment: value.assignment.as_str().to_owned(),
            authority: (&value.authority).into(),
            kind: (&value.kind).into(),
            resolves: value.resolves.as_ref().map(|id| id.as_str().to_owned()),
        }
    }
}

impl TryFrom<StoredDecisionRecord> for DecisionRecord {
    type Error = StoredError;

    fn try_from(value: StoredDecisionRecord) -> Result<Self, Self::Error> {
        Ok(Self {
            operation: parse_id!(OperationId, value.operation, "operation id")?,
            assignment: parse_id!(AssignmentId, value.assignment, "assignment id")?,
            authority: value.authority.try_into()?,
            kind: value.kind.try_into()?,
            resolves: value
                .resolves
                .map(|id| parse_id!(SignalId, id, "signal id"))
                .transpose()?,
        })
    }
}

impl From<&EnvelopeSnapshot> for StoredEnvelope {
    fn from(value: &EnvelopeSnapshot) -> Self {
        Self {
            content: value.content().to_owned(),
            content_hash: value.content_hash.as_str().to_owned(),
        }
    }
}

impl TryFrom<StoredEnvelope> for EnvelopeSnapshot {
    type Error = StoredError;

    fn try_from(value: StoredEnvelope) -> Result<Self, Self::Error> {
        EnvelopeSnapshot::new(value.content, content_hash(value.content_hash)?)
            .map_err(|_| invalid("envelope", "too large"))
    }
}

impl From<CloseReason> for StoredCloseReason {
    fn from(value: CloseReason) -> Self {
        match value {
            CloseReason::AcceptedHandoff => Self::AcceptedHandoff,
            CloseReason::CancelledObsolete => Self::CancelledObsolete,
        }
    }
}

impl From<StoredCloseReason> for CloseReason {
    fn from(value: StoredCloseReason) -> Self {
        match value {
            StoredCloseReason::AcceptedHandoff => Self::AcceptedHandoff,
            StoredCloseReason::CancelledObsolete => Self::CancelledObsolete,
        }
    }
}

impl From<&WorkProjection> for StoredWorkProjection {
    fn from(value: &WorkProjection) -> Self {
        match value {
            WorkProjection::MarkInProgress => Self::MarkInProgress,
            WorkProjection::Close { reason } => Self::Close {
                reason: (*reason).into(),
            },
        }
    }
}

impl From<StoredWorkProjection> for WorkProjection {
    fn from(value: StoredWorkProjection) -> Self {
        match value {
            StoredWorkProjection::MarkInProgress => Self::MarkInProgress,
            StoredWorkProjection::Close { reason } => Self::Close {
                reason: reason.into(),
            },
        }
    }
}

impl From<&PendingApplication> for StoredPendingApplication {
    fn from(value: &PendingApplication) -> Self {
        Self {
            operation: value.operation.as_str().to_owned(),
            assignment: value.assignment.as_str().to_owned(),
            bead: value.bead.as_str().to_owned(),
            projection: (&value.projection).into(),
            committed_at: value.committed_at.0,
            authorized_revision: value
                .authorized_revision
                .as_ref()
                .map(|revision| revision.0.as_str().to_owned()),
        }
    }
}

impl TryFrom<StoredPendingApplication> for PendingApplication {
    type Error = StoredError;

    fn try_from(value: StoredPendingApplication) -> Result<Self, Self::Error> {
        Ok(Self {
            operation: parse_id!(OperationId, value.operation, "operation id")?,
            assignment: parse_id!(AssignmentId, value.assignment, "assignment id")?,
            bead: parse_id!(BeadId, value.bead, "bead id")?,
            projection: value.projection.into(),
            committed_at: Seq(value.committed_at),
            authorized_revision: value.authorized_revision.map(work_revision).transpose()?,
            receipt_candidate: None,
        })
    }
}

impl From<ObservedCloseReason> for StoredObservedCloseReason {
    fn from(value: ObservedCloseReason) -> Self {
        match value {
            ObservedCloseReason::AcceptedHandoff => Self::AcceptedHandoff,
            ObservedCloseReason::CancelledObsolete => Self::CancelledObsolete,
            ObservedCloseReason::UnrecognizedProviderReason => Self::UnrecognizedProviderReason,
        }
    }
}

impl From<StoredObservedCloseReason> for ObservedCloseReason {
    fn from(value: StoredObservedCloseReason) -> Self {
        match value {
            StoredObservedCloseReason::AcceptedHandoff => Self::AcceptedHandoff,
            StoredObservedCloseReason::CancelledObsolete => Self::CancelledObsolete,
            StoredObservedCloseReason::UnrecognizedProviderReason => {
                Self::UnrecognizedProviderReason
            }
        }
    }
}

impl From<WorkStatus> for StoredWorkStatus {
    fn from(value: WorkStatus) -> Self {
        match value {
            WorkStatus::Open => Self::Open,
            WorkStatus::InProgress => Self::InProgress,
            WorkStatus::Closed { observed_reason } => Self::Closed {
                reason: observed_reason.into(),
            },
        }
    }
}

impl From<StoredWorkStatus> for WorkStatus {
    fn from(value: StoredWorkStatus) -> Self {
        match value {
            StoredWorkStatus::Open => Self::Open,
            StoredWorkStatus::InProgress => Self::InProgress,
            StoredWorkStatus::Closed { reason } => Self::Closed {
                observed_reason: reason.into(),
            },
        }
    }
}

impl From<&WorkError> for StoredWorkError {
    fn from(value: &WorkError) -> Self {
        match value {
            WorkError::ProviderUnavailable => Self::ProviderUnavailable,
            WorkError::Incompatible => Self::Incompatible,
            WorkError::Corrupt => Self::Corrupt,
            WorkError::Busy => Self::Busy,
            WorkError::MalformedOutput => Self::MalformedOutput,
            WorkError::NotFound => Self::NotFound,
            WorkError::RevisionConflict => Self::RevisionConflict,
            WorkError::BeadParked => Self::BeadParked,
            WorkError::AmbiguousOutcome => Self::AmbiguousOutcome,
            WorkError::ScopeLabelMalformed { label } => Self::ScopeLabelMalformed {
                label: label.clone(),
            },
            WorkError::ScopeLabelConflict { key } => Self::ScopeLabelConflict { key: key.clone() },
        }
    }
}

impl From<StoredWorkError> for WorkError {
    fn from(value: StoredWorkError) -> Self {
        match value {
            StoredWorkError::ProviderUnavailable => Self::ProviderUnavailable,
            StoredWorkError::Incompatible => Self::Incompatible,
            StoredWorkError::Corrupt => Self::Corrupt,
            StoredWorkError::Busy => Self::Busy,
            StoredWorkError::MalformedOutput => Self::MalformedOutput,
            StoredWorkError::NotFound => Self::NotFound,
            StoredWorkError::RevisionConflict => Self::RevisionConflict,
            StoredWorkError::BeadParked => Self::BeadParked,
            StoredWorkError::AmbiguousOutcome => Self::AmbiguousOutcome,
            StoredWorkError::ScopeLabelMalformed { label } => Self::ScopeLabelMalformed { label },
            StoredWorkError::ScopeLabelConflict { key } => Self::ScopeLabelConflict { key },
        }
    }
}

impl From<&ApplicationOutcome> for StoredApplicationOutcome {
    fn from(value: &ApplicationOutcome) -> Self {
        match value {
            ApplicationOutcome::Applied { before, after } => Self::Applied {
                before: before.0.as_str().to_owned(),
                after: after.0.as_str().to_owned(),
            },
            ApplicationOutcome::FoundPresent { status, revision } => Self::FoundPresent {
                status: (*status).into(),
                revision: revision.0.as_str().to_owned(),
            },
            ApplicationOutcome::ObservedAfterAmbiguous { status, revision } => {
                Self::ObservedAfterAmbiguous {
                    status: (*status).into(),
                    revision: revision.0.as_str().to_owned(),
                }
            }
            ApplicationOutcome::Failed { error } => Self::Failed {
                error: error.into(),
            },
            ApplicationOutcome::Ambiguous => Self::Ambiguous,
        }
    }
}

impl TryFrom<StoredApplicationOutcome> for ApplicationOutcome {
    type Error = StoredError;

    fn try_from(value: StoredApplicationOutcome) -> Result<Self, Self::Error> {
        Ok(match value {
            StoredApplicationOutcome::Applied { before, after } => Self::Applied {
                before: work_revision(before)?,
                after: work_revision(after)?,
            },
            StoredApplicationOutcome::FoundPresent { status, revision } => Self::FoundPresent {
                status: status.into(),
                revision: work_revision(revision)?,
            },
            StoredApplicationOutcome::ObservedAfterAmbiguous { status, revision } => {
                Self::ObservedAfterAmbiguous {
                    status: status.into(),
                    revision: work_revision(revision)?,
                }
            }
            StoredApplicationOutcome::Failed { error } => Self::Failed {
                error: error.into(),
            },
            StoredApplicationOutcome::Ambiguous => Self::Ambiguous,
        })
    }
}

impl From<&ApplicationAttempt> for StoredApplicationAttempt {
    fn from(value: &ApplicationAttempt) -> Self {
        Self {
            id: value.id.as_str().to_owned(),
            target: value.target.as_str().to_owned(),
            outcome: (&value.outcome).into(),
        }
    }
}

impl TryFrom<StoredApplicationAttempt> for ApplicationAttempt {
    type Error = StoredError;

    fn try_from(value: StoredApplicationAttempt) -> Result<Self, Self::Error> {
        Ok(Self {
            id: parse_id!(OperationId, value.id, "operation id")?,
            target: parse_id!(OperationId, value.target, "operation id")?,
            outcome: value.outcome.try_into()?,
        })
    }
}

impl From<&ApplicationReceipt> for StoredApplicationReceipt {
    fn from(value: &ApplicationReceipt) -> Self {
        Self {
            target: value.target.as_str().to_owned(),
            attempt: value.attempt.as_str().to_owned(),
            after: value.after.0.as_str().to_owned(),
        }
    }
}

impl TryFrom<StoredApplicationReceipt> for ApplicationReceipt {
    type Error = StoredError;

    fn try_from(value: StoredApplicationReceipt) -> Result<Self, Self::Error> {
        Ok(Self {
            target: parse_id!(OperationId, value.target, "operation id")?,
            attempt: parse_id!(OperationId, value.attempt, "operation id")?,
            after: work_revision(value.after)?,
        })
    }
}

impl From<&LaunchSubject> for StoredLaunchSubject {
    fn from(value: &LaunchSubject) -> Self {
        match value {
            LaunchSubject::WorkerAttempt {
                attempt,
                credential,
            } => Self::WorkerAttempt {
                attempt: attempt.as_str().to_owned(),
                credential: credential.as_str().to_owned(),
            },
            LaunchSubject::ActorActivation {
                actor,
                profile,
                generation,
                credential,
            } => Self::ActorActivation {
                actor: actor.as_str().to_owned(),
                profile: profile.as_str().to_owned(),
                generation: generation.as_str().to_owned(),
                credential: credential.as_str().to_owned(),
            },
        }
    }
}

impl TryFrom<StoredLaunchSubject> for LaunchSubject {
    type Error = StoredError;

    fn try_from(value: StoredLaunchSubject) -> Result<Self, Self::Error> {
        Ok(match value {
            StoredLaunchSubject::WorkerAttempt {
                attempt,
                credential,
            } => Self::WorkerAttempt {
                attempt: parse_id!(AttemptId, attempt, "attempt id")?,
                credential: parse_id!(CredentialId, credential, "credential id")?,
            },
            StoredLaunchSubject::ActorActivation {
                actor,
                profile,
                generation,
                credential,
            } => Self::ActorActivation {
                actor: parse_id!(ActorId, actor, "actor id")?,
                profile: parse_id!(ProfileName, profile, "profile name")?,
                generation: parse_id!(OperationId, generation, "operation id")?,
                credential: parse_id!(CredentialId, credential, "credential id")?,
            },
        })
    }
}

impl From<LivenessKind> for StoredLivenessKind {
    fn from(value: LivenessKind) -> Self {
        match value {
            LivenessKind::Starting => Self::Starting,
            LivenessKind::Running => Self::Running,
            LivenessKind::Idle => Self::Idle,
            LivenessKind::Blocked => Self::Blocked,
            LivenessKind::Exited => Self::Exited,
            LivenessKind::NotFound => Self::NotFound,
            LivenessKind::Unavailable => Self::Unavailable,
            LivenessKind::Unknown => Self::Unknown,
            LivenessKind::StaleGeneration => Self::StaleGeneration,
        }
    }
}

impl From<StoredLivenessKind> for LivenessKind {
    fn from(value: StoredLivenessKind) -> Self {
        match value {
            StoredLivenessKind::Starting => Self::Starting,
            StoredLivenessKind::Running => Self::Running,
            StoredLivenessKind::Idle => Self::Idle,
            StoredLivenessKind::Blocked => Self::Blocked,
            StoredLivenessKind::Exited => Self::Exited,
            StoredLivenessKind::NotFound => Self::NotFound,
            StoredLivenessKind::Unavailable => Self::Unavailable,
            StoredLivenessKind::Unknown => Self::Unknown,
            StoredLivenessKind::StaleGeneration => Self::StaleGeneration,
        }
    }
}

impl From<&RuntimeObservationRecord> for StoredRuntimeObservation {
    fn from(value: &RuntimeObservationRecord) -> Self {
        Self {
            reporter: (&value.reporter).into(),
            subject: (&value.subject).into(),
            observed_at: value.observation.observed_at.0,
            liveness: value.observation.kind.into(),
        }
    }
}

impl TryFrom<StoredRuntimeObservation> for RuntimeObservationRecord {
    type Error = StoredError;

    fn try_from(value: StoredRuntimeObservation) -> Result<Self, Self::Error> {
        Ok(Self {
            reporter: value.reporter.try_into()?,
            subject: value.subject.try_into()?,
            observation: LivenessObservation {
                observed_at: Timestamp(value.observed_at),
                kind: value.liveness.into(),
            },
        })
    }
}

impl From<&AuditOperation> for StoredAuditOperation {
    fn from(value: &AuditOperation) -> Self {
        match value {
            AuditOperation::Operation(id) => Self::Operation {
                id: id.as_str().to_owned(),
            },
            AuditOperation::Signal(id) => Self::Signal {
                id: id.as_str().to_owned(),
            },
        }
    }
}

impl TryFrom<StoredAuditOperation> for AuditOperation {
    type Error = StoredError;

    fn try_from(value: StoredAuditOperation) -> Result<Self, Self::Error> {
        Ok(match value {
            StoredAuditOperation::Operation { id } => {
                Self::Operation(parse_id!(OperationId, id, "operation id")?)
            }
            StoredAuditOperation::Signal { id } => {
                Self::Signal(parse_id!(SignalId, id, "signal id")?)
            }
        })
    }
}

impl From<&AuditInitiator> for StoredAuditInitiator {
    fn from(value: &AuditInitiator) -> Self {
        match value {
            AuditInitiator::Authority(authority) => Self::Authority {
                authority: authority.into(),
            },
            AuditInitiator::WorkerBinding {
                actor,
                assignment,
                attempt,
            } => Self::WorkerBinding {
                actor: actor.into(),
                assignment: assignment.as_str().to_owned(),
                attempt: attempt.as_str().to_owned(),
            },
            AuditInitiator::OperatorChannel => Self::OperatorChannel,
            AuditInitiator::SystemProjection { authorizing } => Self::SystemProjection {
                authorizing: authorizing.as_str().to_owned(),
            },
        }
    }
}

impl TryFrom<StoredAuditInitiator> for AuditInitiator {
    type Error = StoredError;

    fn try_from(value: StoredAuditInitiator) -> Result<Self, Self::Error> {
        Ok(match value {
            StoredAuditInitiator::Authority { authority } => Self::Authority(authority.try_into()?),
            StoredAuditInitiator::WorkerBinding {
                actor,
                assignment,
                attempt,
            } => Self::WorkerBinding {
                actor: actor.try_into()?,
                assignment: parse_id!(AssignmentId, assignment, "assignment id")?,
                attempt: parse_id!(AttemptId, attempt, "attempt id")?,
            },
            StoredAuditInitiator::OperatorChannel => Self::OperatorChannel,
            StoredAuditInitiator::SystemProjection { authorizing } => Self::SystemProjection {
                authorizing: parse_id!(OperationId, authorizing, "operation id")?,
            },
        })
    }
}

impl From<&AuditSubject> for StoredAuditSubject {
    fn from(value: &AuditSubject) -> Self {
        match value {
            AuditSubject::Workflow(subject) => Self::Workflow {
                subject: subject.into(),
            },
            AuditSubject::ActorProfile { actor, profile } => Self::ActorProfile {
                actor: actor.as_str().to_owned(),
                profile: profile.as_str().to_owned(),
            },
            AuditSubject::Launch(subject) => Self::Launch {
                subject: subject.into(),
            },
            AuditSubject::Projection(operation) => Self::Projection {
                operation: operation.as_str().to_owned(),
            },
        }
    }
}

impl TryFrom<StoredAuditSubject> for AuditSubject {
    type Error = StoredError;

    fn try_from(value: StoredAuditSubject) -> Result<Self, Self::Error> {
        Ok(match value {
            StoredAuditSubject::Workflow { subject } => Self::Workflow(subject.try_into()?),
            StoredAuditSubject::ActorProfile { actor, profile } => Self::ActorProfile {
                actor: parse_id!(ActorId, actor, "actor id")?,
                profile: parse_id!(ProfileName, profile, "profile name")?,
            },
            StoredAuditSubject::Launch { subject } => Self::Launch(subject.try_into()?),
            StoredAuditSubject::Projection { operation } => {
                Self::Projection(parse_id!(OperationId, operation, "operation id")?)
            }
        })
    }
}

impl From<AuditDecisionKind> for StoredAuditDecisionKind {
    fn from(value: AuditDecisionKind) -> Self {
        match value {
            AuditDecisionKind::Accept => Self::Accept,
            AuditDecisionKind::Reject => Self::Reject,
            AuditDecisionKind::Cancel => Self::Cancel,
            AuditDecisionKind::Revoke => Self::Revoke,
            AuditDecisionKind::Reclaim => Self::Reclaim,
            AuditDecisionKind::TransferAuthority => Self::TransferAuthority,
        }
    }
}

impl From<StoredAuditDecisionKind> for AuditDecisionKind {
    fn from(value: StoredAuditDecisionKind) -> Self {
        match value {
            StoredAuditDecisionKind::Accept => Self::Accept,
            StoredAuditDecisionKind::Reject => Self::Reject,
            StoredAuditDecisionKind::Cancel => Self::Cancel,
            StoredAuditDecisionKind::Revoke => Self::Revoke,
            StoredAuditDecisionKind::Reclaim => Self::Reclaim,
            StoredAuditDecisionKind::TransferAuthority => Self::TransferAuthority,
        }
    }
}

impl From<AuditActivationCase> for StoredAuditActivationCase {
    fn from(value: AuditActivationCase) -> Self {
        match value {
            AuditActivationCase::OperatorBootstrap => Self::OperatorBootstrap,
            AuditActivationCase::ActorAuthorizedRotation => Self::ActorAuthorizedRotation,
            AuditActivationCase::OperatorRecovery => Self::OperatorRecovery,
            AuditActivationCase::OperatorOrchestratorEnrolment => {
                Self::OperatorOrchestratorEnrolment
            }
        }
    }
}

impl From<StoredAuditActivationCase> for AuditActivationCase {
    fn from(value: StoredAuditActivationCase) -> Self {
        match value {
            StoredAuditActivationCase::OperatorBootstrap => Self::OperatorBootstrap,
            StoredAuditActivationCase::ActorAuthorizedRotation => Self::ActorAuthorizedRotation,
            StoredAuditActivationCase::OperatorRecovery => Self::OperatorRecovery,
            StoredAuditActivationCase::OperatorOrchestratorEnrolment => {
                Self::OperatorOrchestratorEnrolment
            }
        }
    }
}

impl From<AuditSubmissionRefusal> for StoredAuditSubmissionRefusal {
    fn from(value: AuditSubmissionRefusal) -> Self {
        match value {
            AuditSubmissionRefusal::DirtyWorktree => Self::DirtyWorktree,
            AuditSubmissionRefusal::MissingEvidence => Self::MissingEvidence,
            AuditSubmissionRefusal::EvidenceWrongCommit => Self::EvidenceWrongCommit,
            AuditSubmissionRefusal::FailingOutcome => Self::FailingOutcome,
            AuditSubmissionRefusal::EditScopeViolation => Self::EditScopeViolation,
            AuditSubmissionRefusal::Directive(reason) => Self::Directive {
                reason: reason.into(),
            },
            AuditSubmissionRefusal::RedGreen => Self::RedGreen,
        }
    }
}

impl From<StoredAuditSubmissionRefusal> for AuditSubmissionRefusal {
    fn from(value: StoredAuditSubmissionRefusal) -> Self {
        match value {
            StoredAuditSubmissionRefusal::DirtyWorktree => Self::DirtyWorktree,
            StoredAuditSubmissionRefusal::MissingEvidence => Self::MissingEvidence,
            StoredAuditSubmissionRefusal::EvidenceWrongCommit => Self::EvidenceWrongCommit,
            StoredAuditSubmissionRefusal::FailingOutcome => Self::FailingOutcome,
            StoredAuditSubmissionRefusal::EditScopeViolation => Self::EditScopeViolation,
            StoredAuditSubmissionRefusal::Directive { reason } => Self::Directive(reason.into()),
            StoredAuditSubmissionRefusal::RedGreen => Self::RedGreen,
        }
    }
}

impl From<AuditApplicationOutcome> for StoredAuditApplicationOutcome {
    fn from(value: AuditApplicationOutcome) -> Self {
        match value {
            AuditApplicationOutcome::Applied => Self::Applied,
            AuditApplicationOutcome::FoundPresent => Self::FoundPresent,
            AuditApplicationOutcome::ObservedAfterAmbiguous => Self::ObservedAfterAmbiguous,
            AuditApplicationOutcome::Failed => Self::Failed,
            AuditApplicationOutcome::Ambiguous => Self::Ambiguous,
        }
    }
}

impl From<StoredAuditApplicationOutcome> for AuditApplicationOutcome {
    fn from(value: StoredAuditApplicationOutcome) -> Self {
        match value {
            StoredAuditApplicationOutcome::Applied => Self::Applied,
            StoredAuditApplicationOutcome::FoundPresent => Self::FoundPresent,
            StoredAuditApplicationOutcome::ObservedAfterAmbiguous => Self::ObservedAfterAmbiguous,
            StoredAuditApplicationOutcome::Failed => Self::Failed,
            StoredAuditApplicationOutcome::Ambiguous => Self::Ambiguous,
        }
    }
}

impl From<&AuditKind> for StoredAuditKind {
    fn from(value: &AuditKind) -> Self {
        match value {
            AuditKind::AssignmentOpened => Self::AssignmentOpened,
            AuditKind::AttemptOpened => Self::AttemptOpened,
            AuditKind::DecisionRecorded { kind } => Self::DecisionRecorded {
                kind: (*kind).into(),
            },
            AuditKind::ProfileActivated { case } => Self::ProfileActivated {
                case: (*case).into(),
            },
            AuditKind::ProfileDeactivated => Self::ProfileDeactivated,
            AuditKind::DirectiveAppended { signal } => Self::DirectiveAppended {
                signal: signal.as_str().to_owned(),
            },
            AuditKind::RequestAppended { signal } => Self::RequestAppended {
                signal: signal.as_str().to_owned(),
            },
            AuditKind::ReportRecorded { signal } => Self::ReportRecorded {
                signal: signal.as_str().to_owned(),
            },
            AuditKind::ReportRefused { reason } => Self::ReportRefused {
                reason: (*reason).into(),
            },
            AuditKind::EvidenceRecorded => Self::EvidenceRecorded,
            AuditKind::EvidenceRefused { reason } => Self::EvidenceRefused {
                reason: (*reason).into(),
            },
            AuditKind::HandoffRecorded { handoff } => Self::HandoffRecorded {
                handoff: handoff.as_str().to_owned(),
            },
            AuditKind::HandoffRefused { reason } => Self::HandoffRefused {
                reason: (*reason).into(),
            },
            AuditKind::LeaseRenewed => Self::LeaseRenewed,
            AuditKind::AttemptAborted => Self::AttemptAborted,
            AuditKind::EnvelopePersisted => Self::EnvelopePersisted,
            AuditKind::RuntimeHandleBound => Self::RuntimeHandleBound,
            AuditKind::RuntimeHandleUnbound => Self::RuntimeHandleUnbound,
            AuditKind::RuntimeObservationRecorded => Self::RuntimeObservationRecorded,
            AuditKind::ApplicationAttemptRecorded { outcome } => Self::ApplicationAttemptRecorded {
                outcome: (*outcome).into(),
            },
            AuditKind::ApplicationReceiptRecorded => Self::ApplicationReceiptRecorded,
        }
    }
}

impl TryFrom<StoredAuditKind> for AuditKind {
    type Error = StoredError;

    fn try_from(value: StoredAuditKind) -> Result<Self, Self::Error> {
        Ok(match value {
            StoredAuditKind::AssignmentOpened => Self::AssignmentOpened,
            StoredAuditKind::AttemptOpened => Self::AttemptOpened,
            StoredAuditKind::DecisionRecorded { kind } => {
                Self::DecisionRecorded { kind: kind.into() }
            }
            StoredAuditKind::ProfileActivated { case } => {
                Self::ProfileActivated { case: case.into() }
            }
            StoredAuditKind::ProfileDeactivated => Self::ProfileDeactivated,
            StoredAuditKind::DirectiveAppended { signal } => Self::DirectiveAppended {
                signal: parse_id!(SignalId, signal, "signal id")?,
            },
            StoredAuditKind::RequestAppended { signal } => Self::RequestAppended {
                signal: parse_id!(SignalId, signal, "signal id")?,
            },
            StoredAuditKind::ReportRecorded { signal } => Self::ReportRecorded {
                signal: parse_id!(SignalId, signal, "signal id")?,
            },
            StoredAuditKind::ReportRefused { reason } => Self::ReportRefused {
                reason: reason.into(),
            },
            StoredAuditKind::EvidenceRecorded => Self::EvidenceRecorded,
            StoredAuditKind::EvidenceRefused { reason } => Self::EvidenceRefused {
                reason: reason.into(),
            },
            StoredAuditKind::HandoffRecorded { handoff } => Self::HandoffRecorded {
                handoff: parse_id!(HandoffId, handoff, "handoff id")?,
            },
            StoredAuditKind::HandoffRefused { reason } => Self::HandoffRefused {
                reason: reason.into(),
            },
            StoredAuditKind::LeaseRenewed => Self::LeaseRenewed,
            StoredAuditKind::AttemptAborted => Self::AttemptAborted,
            StoredAuditKind::EnvelopePersisted => Self::EnvelopePersisted,
            StoredAuditKind::RuntimeHandleBound => Self::RuntimeHandleBound,
            StoredAuditKind::RuntimeHandleUnbound => Self::RuntimeHandleUnbound,
            StoredAuditKind::RuntimeObservationRecorded => Self::RuntimeObservationRecorded,
            StoredAuditKind::ApplicationAttemptRecorded { outcome } => {
                Self::ApplicationAttemptRecorded {
                    outcome: outcome.into(),
                }
            }
            StoredAuditKind::ApplicationReceiptRecorded => Self::ApplicationReceiptRecorded,
        })
    }
}

impl From<&AuditEvent> for StoredAuditEvent {
    fn from(value: &AuditEvent) -> Self {
        Self {
            seq: value.seq.0,
            at: value.at.0,
            initiator: (&value.initiator).into(),
            operation: (&value.operation).into(),
            subject: (&value.subject).into(),
            kind: (&value.kind).into(),
        }
    }
}

impl TryFrom<StoredAuditEvent> for AuditEvent {
    type Error = StoredError;

    fn try_from(value: StoredAuditEvent) -> Result<Self, Self::Error> {
        Ok(Self {
            seq: Seq(value.seq),
            at: Timestamp(value.at),
            initiator: value.initiator.try_into()?,
            operation: value.operation.try_into()?,
            subject: value.subject.try_into()?,
            kind: value.kind.try_into()?,
        })
    }
}

fn stored_credential(value: &CredentialProvisioning) -> StoredCredentialProvisioning {
    StoredCredentialProvisioning {
        id: value.id.as_str().to_owned(),
        digest: value.digest.as_str().to_owned(),
    }
}

pub(crate) fn assignment_opening_identity(
    value: &AssignmentOpening,
) -> Result<String, StoredError> {
    encode_identity(&StoredAssignmentOpening {
        assignment: (&value.assignment).into(),
        first_attempt: (&value.first_attempt).into(),
        authorizing: StoredAssignDecision {
            operation: value.authorizing.operation.as_str().to_owned(),
            assignment: value.authorizing.assignment.as_str().to_owned(),
            first_attempt: value.authorizing.first_attempt.as_str().to_owned(),
            authority: (&value.authorizing.authority).into(),
        },
        bead_revision: value.bead_revision.0.as_str().to_owned(),
        worker_credential: stored_credential(&value.worker_credential),
    })
}

pub(crate) fn attempt_opening_identity(value: &AttemptOpening) -> Result<String, StoredError> {
    encode_identity(&StoredAttemptOpening {
        authorizing: StoredRetryDecision {
            operation: value.authorizing.operation.as_str().to_owned(),
            assignment: value.authorizing.assignment.as_str().to_owned(),
            authority: (&value.authorizing.authority).into(),
            reason: value.authorizing.reason.as_str().to_owned(),
        },
        attempt: (&value.attempt).into(),
        worker_credential: stored_credential(&value.worker_credential),
    })
}

pub(crate) fn decision_identity(value: &DecisionRecord) -> Result<String, StoredError> {
    encode_identity(&StoredDecisionRecord::from(value))
}

pub(crate) fn activation_identity(value: &ActivationOpening) -> Result<String, StoredError> {
    let activation = &value.activation;
    let case = match &value.case {
        ActivationCase::OperatorBootstrap => StoredActivationCase::OperatorBootstrap,
        ActivationCase::ActorAuthorizedRotation { authority } => {
            StoredActivationCase::ActorAuthorizedRotation {
                authority: authority.into(),
            }
        }
        ActivationCase::OperatorRecovery => StoredActivationCase::OperatorRecovery,
        ActivationCase::OperatorOrchestratorEnrolment => {
            StoredActivationCase::OperatorOrchestratorEnrolment
        }
    };
    encode_identity(&StoredActivationOpening {
        activation: StoredProfileActivation {
            operation: activation.operation.as_str().to_owned(),
            actor: activation.actor.as_str().to_owned(),
            profile: activation.profile.as_str().to_owned(),
            profile_hash: activation.profile_hash.as_str().to_owned(),
            class: activation.class().into(),
            occupancy: match activation.occupancy() {
                OccupancyClass::Singleton => StoredOccupancyClass::Singleton,
                OccupancyClass::Shared => StoredOccupancyClass::Shared,
            },
            grants: activation
                .grants()
                .iter()
                .map(|grant| StoredGrant {
                    capability: grant.capability.as_str().to_owned(),
                    scope: (&grant.scope).into(),
                })
                .collect(),
        },
        case,
        credential: stored_credential(&value.credential),
    })
}

pub(crate) fn report_identity(
    fenced_identity: &str,
    draft: &SignalDraft,
) -> Result<String, StoredError> {
    encode_identity(&StoredFencedPayload {
        fenced_identity: fenced_identity.to_owned(),
        payload: StoredSignalDraft {
            id: draft.id.as_str().to_owned(),
            sender: (&draft.sender).into(),
            subject: (&draft.subject).into(),
            body: (&draft.body).into(),
        },
    })
}

pub(crate) fn evidence_identity(
    fenced_identity: &str,
    evidence: &Evidence,
) -> Result<String, StoredError> {
    encode_identity(&StoredFencedPayload {
        fenced_identity: fenced_identity.to_owned(),
        payload: StoredEvidence::from(evidence),
    })
}

pub(crate) fn handoff_identity(
    fenced_identity: &str,
    handoff: &HandoffRecord,
) -> Result<String, StoredError> {
    encode_identity(&StoredFencedPayload {
        fenced_identity: fenced_identity.to_owned(),
        payload: StoredHandoffRecord::from(handoff),
    })
}

pub(crate) fn renewal_identity(
    fenced_identity: &str,
    until: Timestamp,
) -> Result<String, StoredError> {
    encode_identity(&StoredRenewalIdentity {
        fenced_identity: fenced_identity.to_owned(),
        until: until.0,
    })
}

pub(crate) fn envelope_identity(
    association_key: &str,
    envelope: &EnvelopeSnapshot,
) -> Result<String, StoredError> {
    encode_identity(&StoredAssociationPayload {
        association_key: association_key.to_owned(),
        payload: StoredEnvelope::from(envelope),
    })
}

pub(crate) fn runtime_observation_identity(
    value: &RuntimeObservationRecord,
) -> Result<String, StoredError> {
    encode_identity(&StoredRuntimeObservation::from(value))
}

pub(crate) fn application_attempt_identity(
    value: &ApplicationAttempt,
) -> Result<String, StoredError> {
    encode_identity(&StoredApplicationAttempt::from(value))
}

pub(crate) fn application_receipt_identity(
    value: &ApplicationReceipt,
) -> Result<String, StoredError> {
    encode_identity(&StoredApplicationReceipt::from(value))
}

fn validate_state(state: &State) -> Result<(), StoredError> {
    for (key, assignment) in &state.assignments {
        if key != assignment.record.id.as_str() {
            return Err(invalid("assignment map key", key));
        }
        for attempt in &assignment.attempts {
            if attempt.record.assignment != assignment.record.id
                || state.attempt_owners.get(attempt.record.id.as_str()) != Some(key)
            {
                return Err(invalid("attempt ownership", attempt.record.id.as_str()));
            }
        }
    }
    for (owner, binding) in &state.credentials {
        if state.credential_owners.get(binding.credential.as_str()) != Some(owner) {
            return Err(invalid("credential ownership", binding.credential.as_str()));
        }
    }
    for signal in &state.signals {
        if signal.seq.0 == 0 || signal.seq.0 > state.head {
            return Err(invalid("signal sequence", signal.seq.0));
        }
    }
    for action in &state.response_actions {
        if action.seq.0 == 0 || action.seq.0 > state.head {
            return Err(invalid("response sequence", action.seq.0));
        }
    }
    for (seq, event) in &state.audit_events {
        if *seq == 0 || *seq > state.head || *seq != event.seq.0 {
            return Err(invalid("audit sequence", seq));
        }
    }
    for (key, handoff) in &state.handoffs {
        if key != handoff.id.as_str() {
            return Err(invalid("handoff map key", key));
        }
    }
    for (key, decision) in &state.decisions {
        if key != decision.operation.as_str() {
            return Err(invalid("decision map key", key));
        }
    }
    for (key, projection) in &state.projections {
        if key != projection.operation.as_str() || projection.committed_at.0 > state.head {
            return Err(invalid("projection map key", key));
        }
    }
    for (target, attempts) in &state.application_attempts {
        if attempts
            .iter()
            .any(|attempt| attempt.target.as_str() != target)
        {
            return Err(invalid("application target", target));
        }
    }
    for (target, receipt) in &state.receipts {
        if receipt.target.as_str() != target {
            return Err(invalid("receipt map key", target));
        }
    }
    Ok(())
}

fn assignment_state_name(state: AssignmentState) -> &'static str {
    match state {
        AssignmentState::Active => "active",
        AssignmentState::Accepted => "accepted",
        AssignmentState::Cancelled => "cancelled",
    }
}

fn parse_assignment_state(value: &str) -> Result<AssignmentState, StoredError> {
    match value {
        "active" => Ok(AssignmentState::Active),
        "accepted" => Ok(AssignmentState::Accepted),
        "cancelled" => Ok(AssignmentState::Cancelled),
        _ => Err(invalid("assignment state", value)),
    }
}

fn attempt_state_name(state: AttemptState) -> &'static str {
    match state {
        AttemptState::Active => "active",
        AttemptState::Submitted => "submitted",
        AttemptState::Accepted => "accepted",
        AttemptState::Rejected => "rejected",
        AttemptState::Revoked => "revoked",
        AttemptState::Expired => "expired",
        AttemptState::Aborted => "aborted",
    }
}

fn parse_attempt_state(value: &str) -> Result<AttemptState, StoredError> {
    match value {
        "active" => Ok(AttemptState::Active),
        "submitted" => Ok(AttemptState::Submitted),
        "accepted" => Ok(AttemptState::Accepted),
        "rejected" => Ok(AttemptState::Rejected),
        "revoked" => Ok(AttemptState::Revoked),
        "expired" => Ok(AttemptState::Expired),
        "aborted" => Ok(AttemptState::Aborted),
        _ => Err(invalid("attempt state", value)),
    }
}

fn authority_class_name(class: AuthorityClass) -> &'static str {
    class.as_str()
}

fn parse_authority_class(value: &str) -> Result<AuthorityClass, StoredError> {
    match value {
        "orchestrator" => Ok(AuthorityClass::Orchestrator),
        "worker" => Ok(AuthorityClass::Worker),
        _ => Err(invalid("authority class", value)),
    }
}

/// Rebuild the canonical behavioral cache from relational rows.
///
/// The rows remain the source of truth. This reconstruction is an internal v1
/// convenience and never licenses aggregate snapshot persistence.
pub(crate) fn load_state(connection: &Connection) -> Result<State, StoredError> {
    let meta = connection
        .query_row(
            "SELECT head_seq, bootstrap_complete FROM workflow_meta WHERE singleton = 1",
            [],
            |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)),
        )
        .optional()?;
    let Some((head, bootstrap_complete)) = meta else {
        let orphan_rows: i64 = connection.query_row(
            "SELECT
                 EXISTS(SELECT 1 FROM actors)
                 OR EXISTS(SELECT 1 FROM profile_snapshots)
                 OR EXISTS(SELECT 1 FROM assignments)
                 OR EXISTS(SELECT 1 FROM attempts)
                 OR EXISTS(SELECT 1 FROM credentials)
                 OR EXISTS(SELECT 1 FROM signals)
                 OR EXISTS(SELECT 1 FROM evidence)
                 OR EXISTS(SELECT 1 FROM handoffs)
                 OR EXISTS(SELECT 1 FROM decisions)
                 OR EXISTS(SELECT 1 FROM application_attempts)
                 OR EXISTS(SELECT 1 FROM application_receipts)
                 OR EXISTS(SELECT 1 FROM envelopes)
                 OR EXISTS(SELECT 1 FROM runtime_handles)
                 OR EXISTS(SELECT 1 FROM idempotency)
                 OR EXISTS(SELECT 1 FROM audit_events)
                 OR EXISTS(SELECT 1 FROM actor_classes)
                 OR EXISTS(SELECT 1 FROM active_profile_members)
                 OR EXISTS(SELECT 1 FROM credential_bindings)
                 OR EXISTS(SELECT 1 FROM response_actions)
                 OR EXISTS(SELECT 1 FROM report_outcomes)
                 OR EXISTS(SELECT 1 FROM evidence_outcomes)
                 OR EXISTS(SELECT 1 FROM submission_outcomes)
                 OR EXISTS(SELECT 1 FROM work_projections)
                 OR EXISTS(SELECT 1 FROM runtime_observations)",
            [],
            |row| row.get(0),
        )?;
        if orphan_rows != 0 {
            return Err(invalid(
                "workflow metadata",
                "record rows exist without the transaction sentinel",
            ));
        }
        return Ok(State::new());
    };
    let head = u64::try_from(head).map_err(|_| invalid("head sequence", head))?;
    let mut state = State::new();
    state.head = head;
    state.bootstrap_complete = match bootstrap_complete {
        0 => false,
        1 => true,
        other => return Err(invalid("bootstrap flag", other)),
    };

    {
        let mut statement = connection.prepare(
            "SELECT operation_key, operation_id, request_identity
             FROM idempotency ORDER BY operation_key",
        )?;
        let mut rows = statement.query([])?;
        while let Some(row) = rows.next()? {
            let key: String = row.get(0)?;
            let operation: Option<String> = row.get(1)?;
            let request: Option<String> = row.get(2)?;
            let operation = operation.ok_or_else(|| invalid("idempotency operation", &key))?;
            let request = request.ok_or_else(|| invalid("idempotency request", &key))?;
            parse_id!(OperationId, operation.clone(), "operation id")?;
            state.operations.insert(key, request);
            state.committed_operations.insert(operation);
        }
    }
    {
        let mut statement = connection
            .prepare("SELECT actor_id, authority_class FROM actor_classes ORDER BY actor_id")?;
        let mut rows = statement.query([])?;
        while let Some(row) = rows.next()? {
            let actor: String = row.get(0)?;
            parse_id!(ActorId, actor.clone(), "actor id")?;
            let class: String = row.get(1)?;
            state
                .actor_classes
                .insert(actor, parse_authority_class(&class)?);
        }
    }
    {
        let mut statement = connection.prepare(
            "SELECT profile_name, actor_id
             FROM active_profile_members ORDER BY profile_name, actor_id",
        )?;
        let mut rows = statement.query([])?;
        while let Some(row) = rows.next()? {
            let profile: String = row.get(0)?;
            let actor: String = row.get(1)?;
            parse_id!(ProfileName, profile.clone(), "profile name")?;
            parse_id!(ActorId, actor.clone(), "actor id")?;
            state
                .active_members
                .entry(profile)
                .or_default()
                .insert(actor);
        }
    }
    {
        let mut statement = connection.prepare(
            "SELECT owner_key, credential_id, record_version, record_json
             FROM credential_bindings ORDER BY owner_key",
        )?;
        let mut rows = statement.query([])?;
        while let Some(row) = rows.next()? {
            let owner: String = row.get(0)?;
            let credential: String = row.get(1)?;
            let version: i64 = row.get(2)?;
            let json: String = row.get(3)?;
            let stored: StoredCredentialBinding = decode_row(version, &json)?;
            let binding: CredentialBinding = stored.try_into()?;
            if binding.credential.as_str() != credential {
                return Err(invalid("credential row identity", credential));
            }
            state.credential_owners.insert(credential, owner.clone());
            state.credentials.insert(owner, binding);
        }
    }
    {
        let mut statement = connection.prepare(
            "SELECT assignment_id, current_state, record_version, record_json
             FROM assignments ORDER BY assignment_id",
        )?;
        let mut rows = statement.query([])?;
        while let Some(row) = rows.next()? {
            let key: String = row.get(0)?;
            let current: Option<String> = row.get(1)?;
            let version: Option<i64> = row.get(2)?;
            let json: Option<String> = row.get(3)?;
            let version = version.ok_or_else(|| invalid("assignment row version", &key))?;
            let json = json.ok_or_else(|| invalid("assignment row body", &key))?;
            let stored: StoredAssignmentRecord = decode_row(version, &json)?;
            let record: AssignmentRecord = stored.try_into()?;
            if record.id.as_str() != key {
                return Err(invalid("assignment row identity", key));
            }
            state.assignments.insert(
                key,
                AssignmentEntry {
                    record,
                    state: parse_assignment_state(
                        current
                            .as_deref()
                            .ok_or_else(|| invalid("assignment current state", "null"))?,
                    )?,
                    attempts: Vec::new(),
                },
            );
        }
    }
    {
        let mut statement = connection.prepare(
            "SELECT attempt_id, assignment_id, state, authorizing_operation,
                    record_version, record_json
             FROM attempts ORDER BY rowid",
        )?;
        let mut rows = statement.query([])?;
        while let Some(row) = rows.next()? {
            let key: String = row.get(0)?;
            let assignment: String = row.get(1)?;
            let current: String = row.get(2)?;
            let authorizing: Option<String> = row.get(3)?;
            let version: Option<i64> = row.get(4)?;
            let json: Option<String> = row.get(5)?;
            let version = version.ok_or_else(|| invalid("attempt row version", &key))?;
            let json = json.ok_or_else(|| invalid("attempt row body", &key))?;
            let stored: StoredAttemptRecord = decode_row(version, &json)?;
            let record: AttemptRecord = stored.try_into()?;
            if record.id.as_str() != key || record.assignment.as_str() != assignment {
                return Err(invalid("attempt row identity", key));
            }
            let owner = state
                .assignments
                .get_mut(&assignment)
                .ok_or_else(|| invalid("attempt assignment", &assignment))?;
            let authorizing =
                authorizing.ok_or_else(|| invalid("attempt authorizing operation", &key))?;
            owner.attempts.push(AttemptEntry {
                record,
                state: parse_attempt_state(&current)?,
                authorizing: parse_id!(OperationId, authorizing, "operation id")?,
            });
            state.attempt_owners.insert(key, assignment);
        }
    }
    {
        let mut seen = BTreeSet::new();
        let mut statement = connection.prepare(
            "SELECT attempt_id, fencing_token, expires_at FROM leases ORDER BY attempt_id",
        )?;
        let mut rows = statement.query([])?;
        while let Some(row) = rows.next()? {
            let attempt_id: String = row.get(0)?;
            let token: i64 = row.get(1)?;
            let expires_at: i64 = row.get(2)?;
            let token = u64::try_from(token).map_err(|_| invalid("fencing token", token))?;
            let expires_at =
                u64::try_from(expires_at).map_err(|_| invalid("lease expiry", expires_at))?;
            let owner = state
                .attempt_owners
                .get(&attempt_id)
                .and_then(|assignment| state.assignments.get(assignment))
                .and_then(|assignment| {
                    assignment
                        .attempts
                        .iter()
                        .find(|attempt| attempt.record.id.as_str() == attempt_id)
                })
                .ok_or_else(|| invalid("lease attempt", &attempt_id))?;
            if owner.record.lease.token.0 != token || owner.record.lease.expires_at.0 != expires_at
            {
                return Err(invalid("lease row disagreement", attempt_id));
            }
            seen.insert(attempt_id);
        }
        if state
            .attempt_owners
            .keys()
            .any(|attempt| !seen.contains(attempt))
        {
            return Err(invalid("missing lease row", "attempt"));
        }
    }

    {
        let mut statement = connection.prepare(
            "SELECT signal_id, committed_seq, record_version, record_json
             FROM signals ORDER BY committed_seq, rowid",
        )?;
        let mut rows = statement.query([])?;
        while let Some(row) = rows.next()? {
            let id: String = row.get(0)?;
            let committed_seq: i64 = row.get(1)?;
            let stored: StoredSignal = decode_row(row.get(2)?, &row.get::<_, String>(3)?)?;
            let signal: Signal = stored.try_into()?;
            let committed_seq = u64::try_from(committed_seq)
                .map_err(|_| invalid("signal sequence", committed_seq))?;
            if signal.id.as_str() != id || signal.seq.0 != committed_seq {
                return Err(invalid("signal row identity", id));
            }
            state.signals.push(signal);
        }
    }
    {
        let mut ordinals = BTreeMap::<u64, i64>::new();
        let mut statement = connection.prepare(
            "SELECT committed_seq, ordinal, record_version, record_json
             FROM response_actions ORDER BY committed_seq, ordinal",
        )?;
        let mut rows = statement.query([])?;
        while let Some(row) = rows.next()? {
            let committed_seq: i64 = row.get(0)?;
            let ordinal: i64 = row.get(1)?;
            let stored: StoredResponseAction = decode_row(row.get(2)?, &row.get::<_, String>(3)?)?;
            let action: ResponseAction = stored.try_into()?;
            let committed_seq = u64::try_from(committed_seq)
                .map_err(|_| invalid("response sequence", committed_seq))?;
            let expected_ordinal = ordinals.entry(committed_seq).or_default();
            if action.seq.0 != committed_seq || ordinal != *expected_ordinal {
                return Err(invalid("response action row identity", committed_seq));
            }
            *expected_ordinal += 1;
            state.response_actions.push(action);
        }
    }
    load_record_map::<StoredReportOutcome, ReportOutcome>(
        connection,
        "SELECT operation_id, record_version, record_json FROM report_outcomes
         ORDER BY operation_id",
        &mut state.report_outcomes,
    )?;
    {
        let mut statement = connection.prepare(
            "SELECT operation_id, attempt_id, record_version, record_json
             FROM evidence ORDER BY committed_seq, rowid",
        )?;
        let mut rows = statement.query([])?;
        while let Some(row) = rows.next()? {
            let operation: String = row.get(0)?;
            let attempt: String = row.get(1)?;
            let stored: StoredEvidenceRecord = decode_row(row.get(2)?, &row.get::<_, String>(3)?)?;
            let record: EvidenceRecord = stored.try_into()?;
            if record.operation.as_str() != operation || record.attempt.as_str() != attempt {
                return Err(invalid("evidence row identity", operation));
            }
            state.evidence.push(record);
        }
    }
    {
        let mut statement = connection.prepare(
            "SELECT operation_id, record_version, record_json
             FROM evidence_outcomes ORDER BY operation_id",
        )?;
        let mut rows = statement.query([])?;
        while let Some(row) = rows.next()? {
            let key: String = row.get(0)?;
            let stored: StoredEvidenceOutcome = decode_row(row.get(1)?, &row.get::<_, String>(2)?)?;
            state.evidence_outcomes.insert(key, stored.into());
        }
    }
    {
        let mut statement = connection.prepare(
            "SELECT operation_id, request_identity, record_version, record_json
             FROM submission_outcomes ORDER BY operation_id",
        )?;
        let mut rows = statement.query([])?;
        while let Some(row) = rows.next()? {
            let key: String = row.get(0)?;
            let request: String = row.get(1)?;
            let stored: StoredSubmissionOutcome =
                decode_row(row.get(2)?, &row.get::<_, String>(3)?)?;
            state.submissions.insert(key, (request, stored.try_into()?));
        }
    }
    load_record_map::<StoredHandoffRecord, HandoffRecord>(
        connection,
        "SELECT handoff_id, record_version, record_json FROM handoffs ORDER BY rowid",
        &mut state.handoffs,
    )?;
    load_record_map::<StoredDecisionRecord, DecisionRecord>(
        connection,
        "SELECT operation_id, record_version, record_json FROM decisions ORDER BY rowid",
        &mut state.decisions,
    )?;
    load_record_map::<StoredEnvelope, EnvelopeSnapshot>(
        connection,
        "SELECT launch_subject_key, record_version, record_json FROM envelopes ORDER BY rowid",
        &mut state.envelopes,
    )?;
    {
        let mut statement = connection
            .prepare("SELECT launch_subject_key, handle FROM runtime_handles ORDER BY rowid")?;
        let mut rows = statement.query([])?;
        while let Some(row) = rows.next()? {
            state
                .handles
                .insert(row.get(0)?, RuntimeHandle::new(row.get::<_, String>(1)?));
        }
    }
    load_record_map::<StoredPendingApplication, PendingApplication>(
        connection,
        "SELECT operation_id, record_version, record_json FROM work_projections
         ORDER BY committed_seq, operation_id",
        &mut state.projections,
    )?;
    {
        let mut statement = connection.prepare(
            "SELECT operation_id, target_operation_id, record_version, record_json
             FROM application_attempts ORDER BY committed_seq, rowid",
        )?;
        let mut rows = statement.query([])?;
        while let Some(row) = rows.next()? {
            let operation: String = row.get(0)?;
            let target: String = row.get(1)?;
            let stored: StoredApplicationAttempt =
                decode_row(row.get(2)?, &row.get::<_, String>(3)?)?;
            let attempt: ApplicationAttempt = stored.try_into()?;
            if attempt.id.as_str() != operation || attempt.target.as_str() != target {
                return Err(invalid("application attempt row identity", operation));
            }
            state
                .application_attempts
                .entry(target)
                .or_default()
                .push(attempt);
        }
    }
    load_record_map::<StoredApplicationReceipt, ApplicationReceipt>(
        connection,
        "SELECT target_operation_id, record_version, record_json
         FROM application_receipts ORDER BY committed_seq, rowid",
        &mut state.receipts,
    )?;
    {
        let mut statement = connection.prepare(
            "SELECT event_seq, record_version, record_json
             FROM audit_events ORDER BY event_seq",
        )?;
        let mut rows = statement.query([])?;
        while let Some(row) = rows.next()? {
            let seq: i64 = row.get(0)?;
            let seq = u64::try_from(seq).map_err(|_| invalid("audit sequence", seq))?;
            let stored: StoredAuditEvent = decode_row(row.get(1)?, &row.get::<_, String>(2)?)?;
            state.audit_events.insert(seq, stored.try_into()?);
        }
    }
    load_record_map::<StoredRuntimeObservation, RuntimeObservationRecord>(
        connection,
        "SELECT operation_id, record_version, record_json
         FROM runtime_observations ORDER BY operation_id",
        &mut state.runtime_observations,
    )?;
    validate_state(&state)?;
    Ok(state)
}

fn load_record_map<S, D>(
    connection: &Connection,
    sql: &str,
    destination: &mut BTreeMap<String, D>,
) -> Result<(), StoredError>
where
    S: DeserializeOwned,
    D: TryFrom<S, Error = StoredError>,
{
    let mut statement = connection.prepare(sql)?;
    let mut rows = statement.query([])?;
    while let Some(row) = rows.next()? {
        let key: String = row.get(0)?;
        let stored: S = decode_row(row.get(1)?, &row.get::<_, String>(2)?)?;
        if destination
            .insert(key.clone(), stored.try_into()?)
            .is_some()
        {
            return Err(invalid("duplicate record key", key));
        }
    }
    Ok(())
}

fn sql_u64(kind: &str, value: u64) -> Result<i64, StoredError> {
    i64::try_from(value).map_err(|_| invalid(kind, value))
}

fn operation_from_key(key: &str) -> Result<&str, StoredError> {
    let (_, operation) = key
        .rsplit_once(':')
        .ok_or_else(|| invalid("idempotency key", key))?;
    OperationId::new(operation).map_err(|_| invalid("idempotency operation", operation))?;
    Ok(operation)
}

fn audit_operation_id(operation: &AuditOperation) -> Option<&str> {
    match operation {
        AuditOperation::Operation(id) => Some(id.as_str()),
        AuditOperation::Signal(_) => None,
    }
}

fn audit_kind_name(kind: &AuditKind) -> &'static str {
    match kind {
        AuditKind::AssignmentOpened => "assignment_opened",
        AuditKind::AttemptOpened => "attempt_opened",
        AuditKind::DecisionRecorded { .. } => "decision_recorded",
        AuditKind::ProfileActivated { .. } => "profile_activated",
        AuditKind::ProfileDeactivated => "profile_deactivated",
        AuditKind::DirectiveAppended { .. } => "directive_appended",
        AuditKind::RequestAppended { .. } => "request_appended",
        AuditKind::ReportRecorded { .. } => "report_recorded",
        AuditKind::ReportRefused { .. } => "report_refused",
        AuditKind::EvidenceRecorded => "evidence_recorded",
        AuditKind::EvidenceRefused { .. } => "evidence_refused",
        AuditKind::HandoffRecorded { .. } => "handoff_recorded",
        AuditKind::HandoffRefused { .. } => "handoff_refused",
        AuditKind::LeaseRenewed => "lease_renewed",
        AuditKind::AttemptAborted => "attempt_aborted",
        AuditKind::EnvelopePersisted => "envelope_persisted",
        AuditKind::RuntimeHandleBound => "runtime_handle_bound",
        AuditKind::RuntimeHandleUnbound => "runtime_handle_unbound",
        AuditKind::RuntimeObservationRecorded => "runtime_observation_recorded",
        AuditKind::ApplicationAttemptRecorded { .. } => "application_attempt_recorded",
        AuditKind::ApplicationReceiptRecorded => "application_receipt_recorded",
    }
}

fn event_seq_for_operation(state: &State, operation: &str) -> u64 {
    state
        .audit_events
        .values()
        .rev()
        .find(|event| audit_operation_id(&event.operation) == Some(operation))
        .map_or(0, |event| event.seq.0)
}

fn assignment_created_seq(state: &State, assignment: &AssignmentId) -> u64 {
    state
        .audit_events
        .values()
        .find(|event| {
            event.kind == AuditKind::AssignmentOpened
                && event.subject
                    == AuditSubject::Workflow(SubjectRef::Assignment(assignment.clone()))
        })
        .map_or(0, |event| event.seq.0)
}

/// Persist exactly one successful port-call delta under `BEGIN IMMEDIATE`.
/// Immutable families have INSERT paths only; current-state families are the
/// only rows this function updates or deletes.
pub(crate) fn persist_delta(
    transaction: &Transaction<'_>,
    before: &State,
    after: &State,
) -> Result<(), StoredError> {
    validate_state(after)?;
    persist_meta_and_current(transaction, before, after)?;
    persist_immutable_records(transaction, before, after)?;
    Ok(())
}

fn persist_meta_and_current(
    transaction: &Transaction<'_>,
    before: &State,
    after: &State,
) -> Result<(), StoredError> {
    transaction.execute(
        "INSERT INTO workflow_meta(singleton, head_seq, bootstrap_complete)
         VALUES (1, ?1, ?2)
         ON CONFLICT(singleton) DO UPDATE SET
             head_seq = excluded.head_seq,
             bootstrap_complete = excluded.bootstrap_complete",
        params![
            sql_u64("head sequence", after.head)?,
            i64::from(after.bootstrap_complete)
        ],
    )?;

    for (actor, class) in &after.actor_classes {
        match before.actor_classes.get(actor) {
            None => {
                transaction.execute(
                    "INSERT INTO actor_classes(actor_id, authority_class) VALUES (?1, ?2)",
                    params![actor, authority_class_name(*class)],
                )?;
            }
            Some(previous) if previous == class => {}
            Some(_) => return Err(invalid("actor class rewrite", actor)),
        }
    }
    if before
        .actor_classes
        .keys()
        .any(|actor| !after.actor_classes.contains_key(actor))
    {
        return Err(invalid("actor class deletion", "not permitted"));
    }

    for (profile, members) in &before.active_members {
        for actor in members {
            if !after
                .active_members
                .get(profile)
                .is_some_and(|current| current.contains(actor))
            {
                transaction.execute(
                    "DELETE FROM active_profile_members
                     WHERE profile_name = ?1 AND actor_id = ?2",
                    params![profile, actor],
                )?;
            }
        }
    }
    for (profile, members) in &after.active_members {
        for actor in members {
            if !before
                .active_members
                .get(profile)
                .is_some_and(|previous| previous.contains(actor))
            {
                transaction.execute(
                    "INSERT INTO active_profile_members(profile_name, actor_id)
                     VALUES (?1, ?2)",
                    params![profile, actor],
                )?;
            }
        }
    }

    for (owner, binding) in &after.credentials {
        let stored = StoredCredentialBinding::from(binding);
        let json = encode_row(&stored)?;
        match before.credentials.get(owner) {
            None => {
                transaction.execute(
                    "INSERT INTO credential_bindings(
                         owner_key, credential_id, revoked, record_version, record_json
                     ) VALUES (?1, ?2, ?3, ?4, ?5)",
                    params![
                        owner,
                        binding.credential.as_str(),
                        i64::from(binding.revoked),
                        i64::from(SCHEMA_VERSION),
                        json
                    ],
                )?;
            }
            Some(previous) => {
                if previous.credential != binding.credential
                    || previous.digest != binding.digest
                    || previous.actor != binding.actor
                    || previous.profile != binding.profile
                    || previous.assignment != binding.assignment
                {
                    return Err(invalid("credential binding rewrite", owner));
                }
                transaction.execute(
                    "UPDATE credential_bindings
                     SET revoked = ?2, record_version = ?3, record_json = ?4
                     WHERE owner_key = ?1",
                    params![
                        owner,
                        i64::from(binding.revoked),
                        i64::from(SCHEMA_VERSION),
                        json
                    ],
                )?;
            }
        }
    }
    if before
        .credentials
        .keys()
        .any(|owner| !after.credentials.contains_key(owner))
    {
        return Err(invalid("credential deletion", "not permitted"));
    }

    for (key, assignment) in &after.assignments {
        let stored = StoredAssignmentRecord::from(&assignment.record);
        let record_json = encode_row(&stored)?;
        match before.assignments.get(key) {
            None => {
                transaction.execute(
                    "INSERT INTO assignments(
                         assignment_id, bead_id, bead_content_hash, scope_map_json,
                         worker_json, decision_actor_json, edit_scope_json,
                         acceptance_json, attempt_policy_json, declared_base,
                         created_seq, record_version, record_json, current_state
                     ) VALUES (
                         ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14
                     )",
                    params![
                        assignment.record.id.as_str(),
                        assignment.record.bead.as_str(),
                        assignment.record.bead_content_hash.as_str(),
                        encode_row(&stored.scope_map)?,
                        encode_row(&stored.worker)?,
                        encode_row(&stored.decision_actor)?,
                        encode_row(&stored.edit_scope)?,
                        encode_row(&stored.acceptance)?,
                        encode_row(&stored.attempt_cap)?,
                        assignment.record.declared_base.as_str(),
                        sql_u64(
                            "assignment creation sequence",
                            assignment_created_seq(after, &assignment.record.id)
                        )?,
                        i64::from(SCHEMA_VERSION),
                        record_json,
                        assignment_state_name(assignment.state),
                    ],
                )?;
            }
            Some(_) => {
                transaction.execute(
                    "UPDATE assignments
                     SET decision_actor_json = ?2, record_version = ?3,
                         record_json = ?4, current_state = ?5
                     WHERE assignment_id = ?1",
                    params![
                        key,
                        encode_row(&stored.decision_actor)?,
                        i64::from(SCHEMA_VERSION),
                        record_json,
                        assignment_state_name(assignment.state),
                    ],
                )?;
            }
        }
    }
    if before
        .assignments
        .keys()
        .any(|key| !after.assignments.contains_key(key))
    {
        return Err(invalid("assignment deletion", "not permitted"));
    }

    let before_attempts: BTreeMap<&str, &AttemptEntry> = before
        .assignments
        .values()
        .flat_map(|assignment| assignment.attempts.iter())
        .map(|attempt| (attempt.record.id.as_str(), attempt))
        .collect();
    for attempt in after
        .assignments
        .values()
        .flat_map(|assignment| assignment.attempts.iter())
    {
        let stored = StoredAttemptRecord::from(&attempt.record);
        let record_json = encode_row(&stored)?;
        match before_attempts.get(attempt.record.id.as_str()) {
            None => {
                transaction.execute(
                    "INSERT INTO attempts(
                         attempt_id, assignment_id, fencing_token, lease_expires_at,
                         state, record_version, record_json, authorizing_operation
                     ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                    params![
                        attempt.record.id.as_str(),
                        attempt.record.assignment.as_str(),
                        sql_u64("fencing token", attempt.record.lease.token.0)?,
                        sql_u64("lease expiry", attempt.record.lease.expires_at.0)?,
                        attempt_state_name(attempt.state),
                        i64::from(SCHEMA_VERSION),
                        record_json,
                        attempt.authorizing.as_str(),
                    ],
                )?;
                transaction.execute(
                    "INSERT INTO leases(attempt_id, fencing_token, expires_at, updated_seq)
                     VALUES (?1, ?2, ?3, ?4)",
                    params![
                        attempt.record.id.as_str(),
                        sql_u64("fencing token", attempt.record.lease.token.0)?,
                        sql_u64("lease expiry", attempt.record.lease.expires_at.0)?,
                        sql_u64("lease update sequence", after.head)?,
                    ],
                )?;
            }
            Some(previous) => {
                if previous.record.id != attempt.record.id
                    || previous.record.assignment != attempt.record.assignment
                    || previous.record.lease.token != attempt.record.lease.token
                    || previous.authorizing != attempt.authorizing
                {
                    return Err(invalid(
                        "attempt identity rewrite",
                        attempt.record.id.as_str(),
                    ));
                }
                transaction.execute(
                    "UPDATE attempts
                     SET lease_expires_at = ?2, state = ?3,
                         record_version = ?4, record_json = ?5
                     WHERE attempt_id = ?1",
                    params![
                        attempt.record.id.as_str(),
                        sql_u64("lease expiry", attempt.record.lease.expires_at.0)?,
                        attempt_state_name(attempt.state),
                        i64::from(SCHEMA_VERSION),
                        record_json,
                    ],
                )?;
                transaction.execute(
                    "UPDATE leases SET expires_at = ?2, updated_seq = ?3
                     WHERE attempt_id = ?1",
                    params![
                        attempt.record.id.as_str(),
                        sql_u64("lease expiry", attempt.record.lease.expires_at.0)?,
                        sql_u64("lease update sequence", after.head)?,
                    ],
                )?;
            }
        }
    }

    for (key, handle) in &before.handles {
        match after.handles.get(key) {
            None => {
                transaction.execute(
                    "DELETE FROM runtime_handles WHERE launch_subject_key = ?1",
                    [key],
                )?;
            }
            Some(current) if current == handle => {}
            Some(_) => return Err(invalid("runtime handle rewrite", key)),
        }
    }
    for (key, handle) in &after.handles {
        if !before.handles.contains_key(key) {
            transaction.execute(
                "INSERT INTO runtime_handles(launch_subject_key, handle, committed_seq)
                 VALUES (?1, ?2, ?3)",
                params![
                    key,
                    handle.as_str(),
                    sql_u64("handle commit sequence", after.head)?
                ],
            )?;
        }
    }
    Ok(())
}

fn persist_immutable_records(
    transaction: &Transaction<'_>,
    before: &State,
    after: &State,
) -> Result<(), StoredError> {
    for (key, request) in &after.operations {
        if !before.operations.contains_key(key) {
            let operation = operation_from_key(key)?;
            transaction.execute(
                "INSERT INTO idempotency(
                     operation_key, request_hash, result_json, committed_seq,
                     operation_id, request_identity
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    key,
                    "v3:identity-in-request_identity",
                    "{}",
                    sql_u64(
                        "idempotency commit sequence",
                        event_seq_for_operation(after, operation)
                    )?,
                    operation,
                    request,
                ],
            )?;
        } else if before.operations.get(key) != Some(request) {
            return Err(invalid("idempotency rewrite", key));
        }
    }
    if before
        .operations
        .keys()
        .any(|key| !after.operations.contains_key(key))
    {
        return Err(invalid("idempotency deletion", "not permitted"));
    }

    let prior_signal_ids: BTreeSet<&str> = before
        .signals
        .iter()
        .map(|signal| signal.id.as_str())
        .collect();
    for signal in &after.signals {
        if prior_signal_ids.contains(signal.id.as_str()) {
            continue;
        }
        let stored = StoredSignal::from(signal);
        let attempt = match &signal.body {
            SignalBody::Directive { attempt, .. } | SignalBody::Report { attempt, .. } => {
                Some(attempt.as_str())
            }
            SignalBody::Request { .. } => None,
        };
        transaction.execute(
            "INSERT INTO signals(
                 signal_id, attempt_id, sender_json, subject_json, body_json,
                 committed_seq, record_version, record_json
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                signal.id.as_str(),
                attempt,
                encode_row(&stored.sender)?,
                encode_row(&stored.subject)?,
                encode_row(&stored.body)?,
                sql_u64("signal sequence", signal.seq.0)?,
                i64::from(SCHEMA_VERSION),
                encode_row(&stored)?,
            ],
        )?;
    }

    if !after.response_actions.starts_with(&before.response_actions) {
        return Err(invalid("response action history", "non-append mutation"));
    }
    let mut ordinals: BTreeMap<u64, i64> = BTreeMap::new();
    for (index, action) in after.response_actions.iter().enumerate() {
        let ordinal = ordinals.entry(action.seq.0).or_default();
        let current = *ordinal;
        *ordinal += 1;
        if index < before.response_actions.len() {
            continue;
        }
        transaction.execute(
            "INSERT INTO response_actions(
                 committed_seq, ordinal, record_version, record_json
             ) VALUES (?1, ?2, ?3, ?4)",
            params![
                sql_u64("response sequence", action.seq.0)?,
                current,
                i64::from(SCHEMA_VERSION),
                encode_row(&StoredResponseAction::from(action))?,
            ],
        )?;
    }

    for (operation, outcome) in &after.report_outcomes {
        if !before.report_outcomes.contains_key(operation) {
            transaction.execute(
                "INSERT INTO report_outcomes(
                     operation_id, record_version, record_json
                 ) VALUES (?1, ?2, ?3)",
                params![
                    operation,
                    i64::from(SCHEMA_VERSION),
                    encode_row(&StoredReportOutcome::from(outcome))?,
                ],
            )?;
        }
    }

    let prior_evidence: BTreeSet<&str> = before
        .evidence
        .iter()
        .map(|record| record.operation.as_str())
        .collect();
    for record in &after.evidence {
        if prior_evidence.contains(record.operation.as_str()) {
            continue;
        }
        let stored = StoredEvidenceRecord::from(record);
        transaction.execute(
            "INSERT INTO evidence(
                 operation_id, attempt_id, evidence_json, committed_seq,
                 record_version, record_json
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                record.operation.as_str(),
                record.attempt.as_str(),
                encode_row(&stored.evidence)?,
                sql_u64(
                    "evidence commit sequence",
                    event_seq_for_operation(after, record.operation.as_str())
                )?,
                i64::from(SCHEMA_VERSION),
                encode_row(&stored)?,
            ],
        )?;
    }
    for (operation, outcome) in &after.evidence_outcomes {
        if !before.evidence_outcomes.contains_key(operation) {
            transaction.execute(
                "INSERT INTO evidence_outcomes(
                     operation_id, record_version, record_json
                 ) VALUES (?1, ?2, ?3)",
                params![
                    operation,
                    i64::from(SCHEMA_VERSION),
                    encode_row(&StoredEvidenceOutcome::from(*outcome))?,
                ],
            )?;
        }
    }
    for (operation, (request, outcome)) in &after.submissions {
        if !before.submissions.contains_key(operation) {
            transaction.execute(
                "INSERT INTO submission_outcomes(
                     operation_id, request_identity, record_version, record_json
                 ) VALUES (?1, ?2, ?3, ?4)",
                params![
                    operation,
                    request,
                    i64::from(SCHEMA_VERSION),
                    encode_row(&StoredSubmissionOutcome::from(outcome))?,
                ],
            )?;
        }
    }
    for (key, record) in &after.handoffs {
        if !before.handoffs.contains_key(key) {
            let stored = StoredHandoffRecord::from(record);
            transaction.execute(
                "INSERT INTO handoffs(
                     handoff_id, attempt_id, handoff_json, committed_seq,
                     record_version, record_json
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    record.id.as_str(),
                    record.attempt.as_str(),
                    encode_row(&stored)?,
                    sql_u64("handoff commit sequence", after.head)?,
                    i64::from(SCHEMA_VERSION),
                    encode_row(&stored)?,
                ],
            )?;
        }
    }
    for (operation, record) in &after.decisions {
        if !before.decisions.contains_key(operation) {
            let stored = StoredDecisionRecord::from(record);
            let decided_handoff = match &record.kind {
                DecisionKind::Accept { handoff, .. } | DecisionKind::Reject { handoff, .. } => {
                    Some(handoff.as_str())
                }
                DecisionKind::Cancel { .. }
                | DecisionKind::Revoke { .. }
                | DecisionKind::Reclaim { .. }
                | DecisionKind::TransferAuthority { .. } => None,
            };
            transaction.execute(
                "INSERT INTO decisions(
                     operation_id, assignment_id, decision_json, committed_seq,
                     record_version, record_json, decided_handoff_id
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![
                    record.operation.as_str(),
                    record.assignment.as_str(),
                    encode_row(&stored)?,
                    sql_u64(
                        "decision commit sequence",
                        event_seq_for_operation(after, record.operation.as_str())
                    )?,
                    i64::from(SCHEMA_VERSION),
                    encode_row(&stored)?,
                    decided_handoff,
                ],
            )?;
        }
    }
    for (key, envelope) in &after.envelopes {
        if !before.envelopes.contains_key(key) {
            let stored = StoredEnvelope::from(envelope);
            transaction.execute(
                "INSERT INTO envelopes(
                     launch_subject_key, envelope_json, committed_seq,
                     record_version, record_json
                 ) VALUES (?1, ?2, ?3, ?4, ?5)",
                params![
                    key,
                    encode_row(&stored)?,
                    sql_u64("envelope commit sequence", after.head)?,
                    i64::from(SCHEMA_VERSION),
                    encode_row(&stored)?,
                ],
            )?;
        }
    }
    for (operation, projection) in &after.projections {
        if !before.projections.contains_key(operation) {
            transaction.execute(
                "INSERT INTO work_projections(
                     operation_id, committed_seq, record_version, record_json
                 ) VALUES (?1, ?2, ?3, ?4)",
                params![
                    operation,
                    sql_u64("projection sequence", projection.committed_at.0)?,
                    i64::from(SCHEMA_VERSION),
                    encode_row(&StoredPendingApplication::from(projection))?,
                ],
            )?;
        }
    }

    let prior_application_ids: BTreeSet<&str> = before
        .application_attempts
        .values()
        .flatten()
        .map(|attempt| attempt.id.as_str())
        .collect();
    for attempt in after.application_attempts.values().flatten() {
        if prior_application_ids.contains(attempt.id.as_str()) {
            continue;
        }
        let stored = StoredApplicationAttempt::from(attempt);
        transaction.execute(
            "INSERT INTO application_attempts(
                 operation_id, target_operation_id, outcome_json, committed_seq,
                 record_version, record_json
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                attempt.id.as_str(),
                attempt.target.as_str(),
                encode_row(&stored.outcome)?,
                sql_u64(
                    "application attempt sequence",
                    event_seq_for_operation(after, attempt.id.as_str())
                )?,
                i64::from(SCHEMA_VERSION),
                encode_row(&stored)?,
            ],
        )?;
    }
    for (target, receipt) in &after.receipts {
        if !before.receipts.contains_key(target) {
            let stored = StoredApplicationReceipt::from(receipt);
            transaction.execute(
                "INSERT INTO application_receipts(
                     target_operation_id, attempt_operation_id, after_revision,
                     committed_seq, record_version, record_json
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    receipt.target.as_str(),
                    receipt.attempt.as_str(),
                    receipt.after.0.as_str(),
                    sql_u64(
                        "application receipt sequence",
                        event_seq_for_operation(after, receipt.target.as_str())
                    )?,
                    i64::from(SCHEMA_VERSION),
                    encode_row(&stored)?,
                ],
            )?;
        }
    }
    for (seq, event) in &after.audit_events {
        if !before.audit_events.contains_key(seq) {
            let stored = StoredAuditEvent::from(event);
            transaction.execute(
                "INSERT INTO audit_events(
                     event_seq, operation_id, event_kind, event_json,
                     record_version, record_json
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    sql_u64("audit sequence", *seq)?,
                    audit_operation_id(&event.operation),
                    audit_kind_name(&event.kind),
                    encode_row(&stored)?,
                    i64::from(SCHEMA_VERSION),
                    encode_row(&stored)?,
                ],
            )?;
        }
    }
    for (operation, observation) in &after.runtime_observations {
        if !before.runtime_observations.contains_key(operation) {
            transaction.execute(
                "INSERT INTO runtime_observations(
                     operation_id, record_version, record_json
                 ) VALUES (?1, ?2, ?3)",
                params![
                    operation,
                    i64::from(SCHEMA_VERSION),
                    encode_row(&StoredRuntimeObservation::from(observation))?,
                ],
            )?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_representation_version_is_loud() {
        let stored = StoredScopeExpr {
            canonical: "*".into(),
            declared_keys: Vec::new(),
        };
        let json = encode_row(&stored).expect("encode scope row");
        assert!(matches!(
            decode_row::<StoredScopeExpr>(999, &json),
            Err(StoredError::UnsupportedVersion {
                found: 999,
                supported: SCHEMA_VERSION
            })
        ));
    }

    #[test]
    fn non_universal_scope_round_trips_with_its_declared_key_set() {
        let keys = vec![
            ScopeKey::new("area").expect("valid key"),
            ScopeKey::new("tier").expect("valid key"),
        ];
        let scope =
            ScopeExpr::parse("tier!=slow & area=state | area=core", &keys).expect("valid scope");
        let stored = StoredScopeExpr::from(&scope);
        let json = encode_row(&stored).expect("encode scope row");
        let decoded: StoredScopeExpr =
            decode_row(i64::from(SCHEMA_VERSION), &json).expect("decode scope row");
        let recovered: ScopeExpr = decoded.try_into().expect("recover scope");
        assert_eq!(recovered, scope);
    }

    #[test]
    fn legacy_effect_presence_decodes_as_conservative_found_present() {
        let legacy = format!(
            r#"{{"schema_version":{SCHEMA_VERSION},"value":{{"kind":"effect_already_present","status":{{"kind":"in_progress"}},"revision":"{}"}}}}"#,
            "a".repeat(64)
        );
        let stored: StoredApplicationOutcome =
            decode_row(i64::from(SCHEMA_VERSION), &legacy).expect("decode legacy attempt");
        assert_eq!(
            ApplicationOutcome::try_from(stored).expect("recover legacy attempt"),
            ApplicationOutcome::FoundPresent {
                status: WorkStatus::InProgress,
                revision: WorkRevision(
                    ContentHash::new(&"a".repeat(64)).expect("valid legacy revision")
                ),
            },
            "the provenance-losing legacy value remains readable but never receipt-eligible"
        );

        let stored_audit: StoredAuditApplicationOutcome =
            serde_json::from_str(r#""effect_already_present""#).expect("decode legacy audit");
        assert_eq!(
            AuditApplicationOutcome::from(stored_audit),
            AuditApplicationOutcome::FoundPresent
        );
    }

    #[test]
    fn overlay_evidence_round_trips_through_checked_constructors() {
        let command = Argv::new(vec!["cargo".into(), "test".into()]).expect("valid argv");
        let path = WorkPath::new("src/lib.rs").expect("valid path");
        let verification = VerificationSet::new(
            vec![command.clone()],
            PathSet::new(vec![path.clone()]).expect("valid path set"),
        )
        .expect("valid verification set");
        let files = FileDigestSet::new(vec![OverlayFile {
            path,
            digest: ContentHash::new(&"a".repeat(64)).expect("valid digest"),
        }])
        .expect("valid file set");
        let evidence = Evidence::new(
            command,
            verification,
            1,
            VerificationOutcome::AssertFail,
            CommitId::new(&"b".repeat(40)).expect("valid commit"),
            WorkspaceDigest::new(&"c".repeat(64)).expect("valid digest"),
            WorkspaceDigest::new(&"c".repeat(64)).expect("valid digest"),
            Some(OverlayCapture {
                declared_base: CommitId::new(&"b".repeat(40)).expect("valid commit"),
                files: files.clone(),
            }),
            files,
            Some(ContentHash::new(&"d".repeat(64)).expect("valid fingerprint")),
        )
        .expect("coherent evidence");
        let stored = StoredEvidence::from(&evidence);
        let json = encode_row(&stored).expect("encode evidence row");
        let decoded: StoredEvidence =
            decode_row(i64::from(SCHEMA_VERSION), &json).expect("decode evidence row");
        let recovered: Evidence = decoded.try_into().expect("recover evidence");
        assert_eq!(recovered, evidence);
    }

    #[test]
    fn typed_audit_event_round_trips_without_record_payload_coupling() {
        let keys = vec![ScopeKey::new("area").expect("valid key")];
        let event = AuditEvent {
            seq: Seq(17),
            at: Timestamp(42),
            initiator: AuditInitiator::Authority(AuthoritySnapshot {
                actor: DecisionActor {
                    actor: ActorId::new("lead-codec").expect("valid actor"),
                    class: AuthorityClass::Orchestrator,
                    profile: ProfileName::new("lead").expect("valid profile"),
                    profile_hash: ContentHash::new(&"e".repeat(64)).expect("valid hash"),
                },
                capability: CapabilityId::new("state:decide").expect("valid capability"),
                scope: ScopeExpr::parse("area=state", &keys).expect("valid scope"),
            }),
            operation: AuditOperation::Signal(SignalId::new("signal-codec").expect("valid signal")),
            subject: AuditSubject::Workflow(SubjectRef::Scope(
                ScopeExpr::parse("area=state", &keys).expect("valid scope"),
            )),
            kind: AuditKind::HandoffRefused {
                reason: AuditSubmissionRefusal::Directive(DirectiveGateRefusal::AmendUndischarged),
            },
        };
        let stored = StoredAuditEvent::from(&event);
        let json = encode_row(&stored).expect("encode audit row");
        let decoded: StoredAuditEvent =
            decode_row(i64::from(SCHEMA_VERSION), &json).expect("decode audit row");
        let recovered: AuditEvent = decoded.try_into().expect("recover audit event");
        assert_eq!(recovered, event);
    }
}
