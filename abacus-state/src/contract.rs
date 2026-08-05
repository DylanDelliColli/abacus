//! Portable behavioral contract for [`WorkflowStatePort`] implementations.
//!
//! Every assertion in this module goes through the public port. The
//! in-memory state and the future SQLite state therefore receive the same
//! scenarios, including time-dependent lease rows driven by an injected
//! clock rather than sleeping or reaching into implementation internals.

use abacus_core::evidence::{AcceptancePolicy, PolicyForm};
use abacus_core::ports::*;
use abacus_core::profile::ProfileActivation;
use abacus_core::*;

/// Test-side control required by time-dependent state scenarios.
///
/// Implementations expose only their public state port plus the injected
/// clock controller. No storage or fake-specific inspection crosses this
/// boundary.
pub trait StateContractHarness {
    fn port(&self) -> &dyn WorkflowStatePort;
    fn set_now(&self, now: Timestamp);
}

/// Additional control for durable implementations whose process-local cache
/// can be discarded and rebuilt from authoritative storage.
pub trait RestartStateContractHarness: StateContractHarness {
    fn restart(&mut self);
}

/// Run the provider-independent state contract.
///
/// `build` is called for each independent scenario. The returned state must
/// be empty and its clock must initially read the supplied timestamp.
pub fn run_workflow_state_suite<H, F>(build: F)
where
    H: StateContractHarness,
    F: Fn(Timestamp) -> H,
{
    opening_is_atomic_idempotent_and_queryable(&build);
    refused_opening_claims_nothing(&build);
    profile_lifecycle_is_derived_and_atomic(&build);
    fencing_tracks_clock_expiry_renewal_and_supersession(&build);
    response_links_are_causal_and_idempotent(&build);
    abort_refusals_are_response_bearing_and_trace_free(&build);
    abort_terminal_is_explicit_idempotent_and_causal(&build);
    decision_terminals_discharge_abort_and_pause(&build);
    pause_and_amend_still_permit_worker_appends(&build);
    handoff_refusals_and_acceptance_are_transactional(&build);
    unresolved_signals_are_derived_from_responses(&build);
    runtime_associations_use_the_full_launch_subject(&build);
    audit_lineage_is_transactional_typed_and_filterable(&build);
    projection_receipts_clear_only_proven_success(&build);
}

/// Exercise restart recovery across every major persisted record family.
///
/// This complements [`run_workflow_state_suite`]: the portable suite proves
/// behavior, while this scenario proves the durable implementation reconstructs
/// that same behavior from its rows rather than process memory.
pub fn run_workflow_state_restart_suite<H, F>(build: F)
where
    H: RestartStateContractHarness,
    F: Fn(Timestamp) -> H,
{
    let mut harness = build(Timestamp(50));
    let pause = {
        let port = harness.port();
        open(port);
        let (pause, _) = port
            .append_signal(&pause("sig-restart-pause"))
            .expect("pause commits");
        let (report, response) = port
            .fenced_report(
                &action("op-restart-report", None),
                &report_draft("sig-restart-report"),
            )
            .expect("report commits under Pause");
        assert!(matches!(report, ReportOutcome::Recorded { .. }));
        assert_eq!(response.binding_directives, vec![pause.clone()]);
        assert_eq!(
            port.fenced_evidence(&action("op-restart-evidence", None), &evidence())
                .expect("evidence commits under Pause")
                .0,
            EvidenceOutcome::Recorded
        );
        let refusal = handoff("handoff-restart-refused", Vec::new());
        assert_eq!(
            port.fenced_submit_handoff(&action("op-restart-refusal", None), &refusal)
                .expect("ordinary refusal commits")
                .0,
            SubmissionOutcome::Refused {
                reason: SubmissionRefusalReason::MissingEvidence
            }
        );
        let subject = worker_subject();
        let envelope =
            EnvelopeSnapshot::new("restart envelope".into(), hash('5')).expect("bounded envelope");
        assert_eq!(
            port.persist_envelope(&op("op-restart-envelope"), &subject, &envelope),
            Ok(StateApplied::Applied)
        );
        assert_eq!(
            port.bind_runtime_handle(
                &op("op-restart-handle"),
                &subject,
                &RuntimeHandle::new("runtime-restart")
            ),
            Ok(StateApplied::Applied)
        );
        let observation = RuntimeObservationRecord {
            reporter: lead_authority("state:observe"),
            subject,
            observation: LivenessObservation {
                observed_at: Timestamp(49),
                kind: LivenessKind::Running,
            },
        };
        assert_eq!(
            port.record_runtime_observation(&op("op-restart-observation"), &observation),
            Ok(StateApplied::Applied)
        );
        let failed = ApplicationAttempt {
            id: op("app-restart-failed"),
            target: op("op-contract-open"),
            outcome: ApplicationOutcome::Failed {
                error: WorkError::ScopeLabelMalformed {
                    label: "malformed:label".into(),
                },
            },
        };
        assert_eq!(
            port.record_application_attempt(&failed),
            Ok(StateApplied::Applied)
        );
        let successful = ApplicationAttempt {
            id: op("app-restart-success"),
            target: op("op-contract-open"),
            outcome: ApplicationOutcome::EffectAlreadyPresent {
                status: WorkStatus::InProgress,
                revision: revision('9'),
            },
        };
        assert_eq!(
            port.record_application_attempt(&successful),
            Ok(StateApplied::Applied)
        );
        assert_eq!(
            port.record_application_receipt(&ApplicationReceipt {
                target: successful.target,
                attempt: successful.id,
                after: revision('9'),
            }),
            Ok(StateApplied::Applied)
        );
        assert_eq!(
            port.renew_lease(&call("op-restart-renew"), Timestamp(150))
                .expect("lease renews")
                .0
                .expires_at,
            Timestamp(150)
        );
        pause
    };
    let audit_before_restart = harness
        .port()
        .audit_events(&AuditQuery::default())
        .expect("audit reads before restart");

    harness.restart();
    {
        let port = harness.port();
        assert_eq!(
            port.assignment(&assignment_id())
                .expect("assignment reloads")
                .attempts,
            vec![(attempt_id(), AttemptState::Active)]
        );
        assert_eq!(
            port.signals_for(&attempt_id())
                .expect("signals reload")
                .len(),
            2
        );
        assert_eq!(
            port.evidence_for(&attempt_id())
                .expect("evidence reloads")
                .len(),
            1
        );
        assert_eq!(
            port.envelope(&worker_subject())
                .expect("envelope reloads")
                .content(),
            "restart envelope"
        );
        assert_eq!(
            port.runtime_handle(&worker_subject()),
            Ok(Some(RuntimeHandle::new("runtime-restart")))
        );
        assert_eq!(
            port.runtime_observation(&op("op-restart-observation"))
                .expect("observation reloads")
                .observation
                .kind,
            LivenessKind::Running
        );
        assert!(
            port.pending_applications()
                .expect("receipts reload")
                .is_empty()
        );
        assert_eq!(
            port.audit_events(&AuditQuery::default())
                .expect("audit reloads"),
            audit_before_restart
        );
        assert_eq!(
            port.fenced_evidence(&action("op-restart-evidence", None), &evidence())
                .expect("evidence replay survives restart")
                .1
                .applied,
            StateApplied::AlreadyApplied
        );
        assert_eq!(
            port.fenced_submit_handoff(
                &action("op-restart-refusal", None),
                &handoff("handoff-restart-refused", Vec::new())
            )
            .expect("refusal replay survives restart")
            .1
            .applied,
            StateApplied::AlreadyApplied
        );
    }

    harness.set_now(Timestamp(151));
    assert_eq!(
        harness
            .port()
            .fenced_evidence(&action("op-restart-expired", None), &evidence()),
        Err(StateError::LeaseExpired),
        "the persisted renewal deadline remains authoritative after restart"
    );
    let reclaim = DecisionRecord {
        operation: op("op-restart-reclaim"),
        assignment: assignment_id(),
        authority: lead_authority("state:reclaim"),
        kind: DecisionKind::Reclaim {
            attempt: attempt_id(),
            reason: reason("lease expired after restart"),
        },
        resolves: Some(SignalId::new("sig-restart-report").expect("valid signal")),
    };
    assert_eq!(
        harness.port().record_decision(&reclaim),
        Ok(StateApplied::Applied)
    );
    harness.restart();
    let port = harness.port();
    assert_eq!(
        port.assignment(&assignment_id())
            .expect("assignment reloads")
            .attempts,
        vec![(attempt_id(), AttemptState::Expired)]
    );
    assert_eq!(port.decision(&reclaim.operation), Ok(reclaim));
    assert_eq!(
        port.verify_launch_subject(&worker_subject(), &hash('f')),
        Err(StateError::CredentialRevoked)
    );
    assert!(
        !port
            .unresolved_signals(None)
            .expect("unresolved derivation reloads")
            .contains(&pause),
        "the reloaded terminal response action discharges Pause"
    );
}

fn op(raw: &str) -> OperationId {
    OperationId::new(raw).expect("contract operation id is valid")
}

fn hash(ch: char) -> ContentHash {
    ContentHash::new(&ch.to_string().repeat(64)).expect("contract hash is valid")
}

fn revision(ch: char) -> WorkRevision {
    WorkRevision(hash(ch))
}

fn commit(ch: char) -> CommitId {
    CommitId::new(&ch.to_string().repeat(40)).expect("contract commit is valid")
}

fn actor(raw: &str) -> ActorId {
    ActorId::new(raw).expect("contract actor id is valid")
}

fn assignment_id() -> AssignmentId {
    AssignmentId::new("asg-contract-1").expect("contract assignment id is valid")
}

fn attempt_id() -> AttemptId {
    AttemptId::new("att-contract-1").expect("contract attempt id is valid")
}

fn lead() -> DecisionActor {
    DecisionActor {
        actor: actor("lead-contract-1"),
        class: AuthorityClass::Orchestrator,
        profile: ProfileName::new("lead").expect("contract profile is valid"),
        profile_hash: hash('a'),
    }
}

fn worker() -> DecisionActor {
    DecisionActor {
        actor: actor("worker-contract-1"),
        class: AuthorityClass::Worker,
        profile: ProfileName::new("worker").expect("contract profile is valid"),
        profile_hash: hash('b'),
    }
}

fn authority_for(actor: DecisionActor, capability: &str) -> AuthoritySnapshot {
    AuthoritySnapshot {
        actor,
        capability: CapabilityId::new(capability).expect("contract capability is valid"),
        scope: ScopeExpr::Universal,
    }
}

fn lead_authority(capability: &str) -> AuthoritySnapshot {
    authority_for(lead(), capability)
}

fn worker_authority(capability: &str) -> AuthoritySnapshot {
    authority_for(worker(), capability)
}

fn reason(raw: &str) -> DecisionReason {
    DecisionReason::new(raw).expect("contract reason is valid")
}

fn verification() -> VerificationSet {
    VerificationSet::new(
        vec![Argv::new(vec!["cargo".into(), "test".into()]).expect("valid argv")],
        PathSet::new(vec![
            WorkPath::new("tests/contract.rs").expect("valid path"),
        ])
        .expect("valid path set"),
    )
    .expect("valid verification set")
}

fn opening() -> AssignmentOpening {
    let assignment = assignment_id();
    let attempt = attempt_id();
    AssignmentOpening {
        assignment: AssignmentRecord {
            id: assignment.clone(),
            bead: BeadId::new("ABACUS-contract.1").expect("valid bead id"),
            bead_content_hash: hash('c'),
            scope_map: ScopeMap::default(),
            worker: worker(),
            decision_actor: lead(),
            edit_scope: EditScope::new(vec![WorkPath::new("src").expect("valid path")])
                .expect("valid edit scope"),
            acceptance: AcceptancePolicy {
                verification: verification(),
                form: PolicyForm::Standard,
            },
            attempt_policy: AttemptPolicy::default(),
            declared_base: commit('d'),
        },
        first_attempt: AttemptRecord {
            id: attempt.clone(),
            assignment: assignment.clone(),
            lease: Lease {
                token: FencingToken(3),
                expires_at: Timestamp(100),
            },
        },
        authorizing: AssignDecision {
            operation: op("op-contract-open"),
            assignment,
            first_attempt: attempt,
            authority: lead_authority("state:assign"),
        },
        bead_revision: revision('e'),
        worker_credential: CredentialProvisioning {
            id: CredentialId::new("cred-contract-1").expect("valid credential id"),
            digest: hash('f'),
        },
    }
}

fn opening_for(
    assignment: &str,
    attempt: &str,
    credential: &str,
    operation: &str,
) -> AssignmentOpening {
    let mut opening = opening();
    let assignment = AssignmentId::new(assignment).expect("valid assignment id");
    let attempt = AttemptId::new(attempt).expect("valid attempt id");
    opening.assignment.id = assignment.clone();
    opening.assignment.bead = BeadId::new("ABACUS-contract.2").expect("valid bead id");
    opening.first_attempt.id = attempt.clone();
    opening.first_attempt.assignment = assignment.clone();
    opening.first_attempt.lease.token = FencingToken(8);
    opening.authorizing.operation = op(operation);
    opening.authorizing.assignment = assignment;
    opening.authorizing.first_attempt = attempt;
    opening.worker_credential.id = CredentialId::new(credential).expect("valid credential id");
    opening
}

fn worker_subject() -> LaunchSubject {
    LaunchSubject::WorkerAttempt {
        attempt: attempt_id(),
        credential: CredentialId::new("cred-contract-1").expect("valid credential id"),
    }
}

fn call(operation: &str) -> FencedCall {
    FencedCall {
        assignment: assignment_id(),
        attempt: attempt_id(),
        actor: worker().actor,
        token: FencingToken(3),
        operation: op(operation),
    }
}

fn action(operation: &str, responds_to: Option<SignalId>) -> FencedAction {
    FencedAction {
        call: call(operation),
        responds_to,
    }
}

fn evidence() -> Evidence {
    let verification = verification();
    Evidence::new(
        verification.commands()[0].clone(),
        verification,
        0,
        VerificationOutcome::Pass,
        commit('d'),
        WorkspaceDigest::new(&"1".repeat(64)).expect("valid workspace digest"),
        WorkspaceDigest::new(&"2".repeat(64)).expect("valid workspace digest"),
        None,
        FileDigestSet::default(),
        None,
    )
    .expect("contract evidence is coherent")
}

fn report_draft(id: &str) -> SignalDraft {
    SignalDraft {
        id: SignalId::new(id).expect("valid signal id"),
        sender: worker_authority("state:report"),
        subject: SubjectRef::Attempt(attempt_id()),
        body: SignalBody::Report {
            attempt: attempt_id(),
            kind: ReportKind::Progress {
                phase: SemanticPhase::Verifying,
                summary: None,
            },
        },
    }
}

fn directive_draft(id: &str, kind: DirectiveKind) -> SignalDraft {
    SignalDraft {
        id: SignalId::new(id).expect("valid signal id"),
        sender: lead_authority("state:directive"),
        subject: SubjectRef::Attempt(attempt_id()),
        body: SignalBody::Directive {
            assignment: assignment_id(),
            attempt: attempt_id(),
            kind,
        },
    }
}

fn request_draft(id: &str, recipient: ActorId) -> SignalDraft {
    SignalDraft {
        id: SignalId::new(id).expect("valid signal id"),
        sender: lead_authority("state:request"),
        subject: SubjectRef::Assignment(assignment_id()),
        body: SignalBody::Request {
            recipient,
            kind: RequestKind::Reconciliation,
            ask: BoundedText::new("reconcile the observed state").expect("valid request text"),
        },
    }
}

fn handoff(id: &str, evidence_operations: Vec<OperationId>) -> HandoffRecord {
    HandoffRecord {
        id: HandoffId::new(id).expect("valid handoff id"),
        attempt: attempt_id(),
        commit: commit('9'),
        expected_base: commit('d'),
        clean_tree: WorkspaceDigest::new(&"3".repeat(64)).expect("valid workspace digest"),
        changed_paths: PathSet::new(vec![WorkPath::new("src/lib.rs").expect("valid path")])
            .expect("valid changed paths"),
        evidence_operations: OperationSet::new(evidence_operations).expect("valid operations"),
        attestation: hash('8'),
    }
}

fn validated_profiles() -> ValidatedProfileSet {
    let registry = vec![
        CapabilityDescriptor {
            id: CapabilityId::new("work:select").expect("valid capability"),
            class: CheckClass::Exclusive,
            bundle: None,
            work_scoped: true,
        },
        CapabilityDescriptor {
            id: CapabilityId::new("state:report").expect("valid capability"),
            class: CheckClass::Fenced,
            bundle: None,
            work_scoped: true,
        },
    ];
    let keys = vec![ScopeKey::new("area").expect("valid scope key")];
    let profiles = vec![
        ProfileSpec {
            name: ProfileName::new("lead").expect("valid profile"),
            class: AuthorityClass::Orchestrator,
            grants: vec![Grant {
                capability: CapabilityId::new("work:select").expect("valid capability"),
                scope: ScopeExpr::parse("area=frontend", &keys).expect("valid scope"),
            }],
        },
        ProfileSpec {
            name: ProfileName::new("second-lead").expect("valid profile"),
            class: AuthorityClass::Orchestrator,
            grants: vec![Grant {
                capability: CapabilityId::new("work:select").expect("valid capability"),
                scope: ScopeExpr::parse("area!=frontend", &keys).expect("valid scope"),
            }],
        },
        ProfileSpec {
            name: ProfileName::new("worker").expect("valid profile"),
            class: AuthorityClass::Worker,
            grants: vec![Grant {
                capability: CapabilityId::new("state:report").expect("valid capability"),
                scope: ScopeExpr::Universal,
            }],
        },
    ];
    validate_profiles(&profiles, &registry).expect("contract profiles validate")
}

fn activation(operation: &str, actor_id: &str, profile: &str) -> ActivationOpening {
    ActivationOpening {
        activation: ProfileActivation::from_validated(
            &validated_profiles(),
            op(operation),
            actor(actor_id),
            ProfileName::new(profile).expect("valid profile"),
            hash('a'),
        )
        .expect("profile exists"),
        case: ActivationCase::OperatorBootstrap,
        credential: CredentialProvisioning {
            id: CredentialId::new(&format!("cred-{operation}")).expect("valid credential"),
            digest: hash('7'),
        },
    }
}

fn with_case(mut opening: ActivationOpening, case: ActivationCase) -> ActivationOpening {
    opening.case = case;
    opening
}

fn open(port: &dyn WorkflowStatePort) {
    assert_eq!(
        port.open_assignment(&opening()),
        Ok(StateApplied::Applied),
        "fixture assignment must open"
    );
}

fn opening_is_atomic_idempotent_and_queryable<H: StateContractHarness>(
    build: &impl Fn(Timestamp) -> H,
) {
    let harness = build(Timestamp(50));
    let port = harness.port();
    let opening = opening();

    assert_eq!(port.open_assignment(&opening), Ok(StateApplied::Applied));
    assert_eq!(
        port.open_assignment(&opening),
        Ok(StateApplied::AlreadyApplied),
        "identical lost-response retry must not duplicate the opening"
    );
    let mut changed = opening.clone();
    changed.bead_revision = revision('9');
    assert_eq!(
        port.open_assignment(&changed),
        Err(StateError::ConflictingOperation),
        "the complete opening bundle participates in operation identity"
    );

    let view = port
        .assignment(&assignment_id())
        .expect("assignment exists");
    assert_eq!(view.record, opening.assignment);
    assert_eq!(view.state, AssignmentState::Active);
    assert_eq!(view.attempts, vec![(attempt_id(), AttemptState::Active)]);
    assert_eq!(view.head, Seq(1));

    assert_eq!(
        port.verify_launch_subject(&worker_subject(), &hash('f')),
        Ok(())
    );
    assert_eq!(
        port.verify_launch_subject(&worker_subject(), &hash('0')),
        Err(StateError::CredentialInvalid)
    );
    let wrong_binding = LaunchSubject::WorkerAttempt {
        attempt: attempt_id(),
        credential: CredentialId::new("cred-contract-wrong").expect("valid credential"),
    };
    assert_eq!(
        port.verify_launch_subject(&wrong_binding, &hash('f')),
        Err(StateError::CredentialBindingMismatch)
    );

    let pending = port.pending_applications().expect("pending query succeeds");
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].operation, op("op-contract-open"));
    assert_eq!(pending[0].projection, WorkProjection::MarkInProgress);
    assert_eq!(pending[0].authorized_revision, Some(revision('e')));
}

fn refused_opening_claims_nothing<H: StateContractHarness>(build: &impl Fn(Timestamp) -> H) {
    let harness = build(Timestamp(50));
    let port = harness.port();
    let mut malformed = opening();
    malformed.first_attempt.assignment = AssignmentId::new("asg-foreign").expect("valid id");

    assert_eq!(
        port.open_assignment(&malformed),
        Err(StateError::IncoherentBundle)
    );
    assert_eq!(
        port.assignment(&assignment_id()),
        Err(StateError::UnknownRecord)
    );
    assert!(
        port.pending_applications()
            .expect("query succeeds")
            .is_empty()
    );
    assert!(
        port.active_occupants(&worker().profile)
            .expect("query succeeds")
            .is_empty()
    );
    assert_eq!(
        port.verify_launch_subject(&worker_subject(), &hash('f')),
        Err(StateError::UnknownRecord)
    );
    assert_eq!(
        port.open_assignment(&opening()),
        Ok(StateApplied::Applied),
        "a refused bundle must not claim its operation identity"
    );
}

fn profile_lifecycle_is_derived_and_atomic<H: StateContractHarness>(
    build: &impl Fn(Timestamp) -> H,
) {
    let harness = build(Timestamp(50));
    let port = harness.port();
    let boot = activation("op-boot", "lead-profile-1", "lead");

    assert_eq!(port.activate_profile(&boot), Ok(StateApplied::Applied));
    assert_eq!(
        port.activate_profile(&boot),
        Ok(StateApplied::AlreadyApplied)
    );
    assert_eq!(
        port.active_occupants(&ProfileName::new("lead").expect("valid profile")),
        Ok(vec![actor("lead-profile-1")])
    );

    let occupied = with_case(
        activation("op-enrol-occupied", "lead-profile-2", "lead"),
        ActivationCase::OperatorOrchestratorEnrolment,
    );
    assert_eq!(
        port.activate_profile(&occupied),
        Err(StateError::ProfileOccupied),
        "singleton occupancy is derived from the validated profile"
    );
    let failed_rotation = with_case(
        activation("op-rotate-unknown", "lead-profile-2", "lead"),
        ActivationCase::ActorAuthorizedRotation {
            authority: lead_authority("profile:rotate"),
        },
    );
    assert_eq!(
        port.activate_profile(&failed_rotation),
        Err(StateError::UnknownActor),
        "a refused enrolment must not register the target actor"
    );

    let old_subject = LaunchSubject::ActorActivation {
        actor: actor("lead-profile-1"),
        profile: ProfileName::new("lead").expect("valid profile"),
        generation: op("op-boot"),
        credential: boot.credential.id.clone(),
    };
    assert_eq!(
        port.verify_launch_subject(&old_subject, &boot.credential.digest),
        Ok(())
    );

    let rotation = with_case(
        activation("op-rotate", "lead-profile-1", "lead"),
        ActivationCase::ActorAuthorizedRotation {
            authority: lead_authority("profile:rotate"),
        },
    );
    assert_eq!(port.activate_profile(&rotation), Ok(StateApplied::Applied));
    assert_eq!(
        port.activate_profile(&rotation),
        Ok(StateApplied::AlreadyApplied)
    );
    let mut changed_rotation = rotation.clone();
    changed_rotation.credential.digest = hash('6');
    assert_eq!(
        port.activate_profile(&changed_rotation),
        Err(StateError::ConflictingOperation),
        "fresh provisioning is part of activation identity"
    );
    let rotated_subject = LaunchSubject::ActorActivation {
        actor: actor("lead-profile-1"),
        profile: ProfileName::new("lead").expect("valid profile"),
        generation: op("op-rotate"),
        credential: rotation.credential.id.clone(),
    };
    assert_eq!(
        port.verify_launch_subject(&old_subject, &boot.credential.digest),
        Err(StateError::CredentialRevoked),
        "rotation revokes the prior activation generation"
    );
    assert_eq!(
        port.verify_launch_subject(&rotated_subject, &rotation.credential.digest),
        Ok(()),
        "rotation atomically provisions the fresh generation"
    );

    assert_eq!(
        port.deactivate_profile(
            &op("op-deactivate"),
            &actor("lead-profile-1"),
            &ProfileName::new("lead").expect("valid profile")
        ),
        Ok(StateApplied::Applied)
    );
    assert!(
        port.active_occupants(&ProfileName::new("lead").expect("valid profile"))
            .expect("query succeeds")
            .is_empty()
    );
    assert_eq!(
        port.verify_launch_subject(&rotated_subject, &rotation.credential.digest),
        Err(StateError::CredentialRevoked)
    );
    assert_eq!(
        port.deactivate_profile(
            &op("op-deactivate"),
            &actor("lead-profile-1"),
            &ProfileName::new("lead").expect("valid profile")
        ),
        Ok(StateApplied::AlreadyApplied)
    );
    assert_eq!(
        port.activate_profile(&activation("op-second-boot", "lead-profile-2", "lead")),
        Err(StateError::BootstrapAlreadyComplete),
        "deactivation never reopens one-shot bootstrap"
    );

    let enrolment = with_case(
        activation("op-enrol-second-profile", "lead-profile-2", "second-lead"),
        ActivationCase::OperatorOrchestratorEnrolment,
    );
    assert_eq!(
        port.activate_profile(&enrolment),
        Ok(StateApplied::Applied),
        "operator enrolment may add a new orchestrator in another singleton profile"
    );
    let recovery = with_case(
        activation("op-recover-second-profile", "lead-profile-2", "second-lead"),
        ActivationCase::OperatorRecovery,
    );
    assert_eq!(
        port.activate_profile(&recovery),
        Ok(StateApplied::Applied),
        "operator recovery rotates an existing orchestrator"
    );

    assert_eq!(port.open_assignment(&opening()), Ok(StateApplied::Applied));
    let mut second_worker = opening_for(
        "asg-contract-worker-2",
        "att-contract-worker-2",
        "cred-contract-worker-2",
        "op-open-worker-2",
    );
    second_worker.assignment.worker.actor = actor("worker-contract-2");
    second_worker.assignment.worker.profile_hash = hash('3');
    second_worker.worker_credential.digest = hash('2');
    assert_eq!(
        port.open_assignment(&second_worker),
        Ok(StateApplied::Applied)
    );
    let worker_profile = ProfileName::new("worker").expect("valid profile");
    assert_eq!(
        port.active_occupants(&worker_profile),
        Ok(vec![actor("worker-contract-1"), actor("worker-contract-2")]),
        "shared profiles expose every active member in stable order"
    );
    assert_eq!(
        port.deactivate_profile(
            &op("op-deactivate-worker-1"),
            &actor("worker-contract-1"),
            &worker_profile
        ),
        Ok(StateApplied::Applied)
    );
    assert_eq!(
        port.active_occupants(&worker_profile),
        Ok(vec![actor("worker-contract-2")]),
        "deactivation removes only the named shared-profile member"
    );
    assert_eq!(
        port.verify_launch_subject(&worker_subject(), &hash('f')),
        Err(StateError::CredentialRevoked)
    );
    let second_worker_subject = LaunchSubject::WorkerAttempt {
        attempt: second_worker.first_attempt.id,
        credential: second_worker.worker_credential.id,
    };
    assert_eq!(
        port.verify_launch_subject(
            &second_worker_subject,
            &second_worker.worker_credential.digest
        ),
        Ok(()),
        "deactivation revokes no co-occupant credential"
    );
}

fn fencing_tracks_clock_expiry_renewal_and_supersession<H: StateContractHarness>(
    build: &impl Fn(Timestamp) -> H,
) {
    let boundary_harness = build(Timestamp(50));
    let boundary = boundary_harness.port();
    open(boundary);
    boundary_harness.set_now(Timestamp(100));
    assert_eq!(
        boundary
            .fenced_evidence(&action("op-at-expiry-boundary", None), &evidence())
            .expect("the lease is live at its exact deadline")
            .0,
        EvidenceOutcome::Recorded
    );
    boundary_harness.set_now(Timestamp(101));
    assert_eq!(
        boundary.fenced_evidence(&action("op-past-expiry-boundary", None), &evidence()),
        Err(StateError::LeaseExpired),
        "lease expiry is strictly now greater than the deadline"
    );

    let harness = build(Timestamp(50));
    let port = harness.port();
    open(port);

    let stale = FencedAction {
        call: FencedCall {
            token: FencingToken(2),
            ..call("op-stale")
        },
        responds_to: None,
    };
    assert_eq!(
        port.fenced_evidence(&stale, &evidence()),
        Err(StateError::StaleFencing)
    );
    assert_eq!(
        port.assignment(&assignment_id()).expect("exists").head,
        Seq(1)
    );
    assert!(
        port.evidence_for(&attempt_id())
            .expect("query succeeds")
            .is_empty()
    );

    let renewal = call("op-renew");
    assert_eq!(
        port.renew_lease(&call("op-nonextending-renewal"), Timestamp(100)),
        Err(StateError::NonExtendingLease),
        "a live but non-extending request is not reported as expiry"
    );
    let (lease, response) = port
        .renew_lease(&renewal, Timestamp(150))
        .expect("live lease extends");
    assert_eq!(lease.expires_at, Timestamp(150));
    assert_eq!(response.applied, StateApplied::Applied);

    harness.set_now(Timestamp(120));
    assert_eq!(
        port.fenced_evidence(&action("op-before-renewed-expiry", None), &evidence())
            .expect("renewed lease remains live")
            .0,
        EvidenceOutcome::Recorded
    );

    harness.set_now(Timestamp(151));
    let before_expired = port.assignment(&assignment_id()).expect("exists").head;
    assert_eq!(
        port.fenced_evidence(&action("op-after-expiry", None), &evidence()),
        Err(StateError::LeaseExpired)
    );
    assert_eq!(
        port.renew_lease(&call("op-renew-after-expiry"), Timestamp(200)),
        Err(StateError::LeaseExpired),
        "renewal cannot revive an already expired lease"
    );
    assert_eq!(
        port.assignment(&assignment_id()).expect("exists").head,
        before_expired,
        "expiry refusals commit no ordering or payload facts"
    );
    assert_eq!(
        port.fenced_evidence(&action("op-before-renewed-expiry", None), &evidence())
            .expect("a committed call replays after expiry")
            .1
            .applied,
        StateApplied::AlreadyApplied
    );
    assert_eq!(
        port.renew_lease(&renewal, Timestamp(150))
            .expect("a committed renewal replays after expiry")
            .1
            .applied,
        StateApplied::AlreadyApplied
    );
    assert_eq!(
        port.renew_lease(&renewal, Timestamp(175)),
        Err(StateError::ConflictingOperation),
        "operation conflict wins over re-evaluating a committed renewal"
    );

    let reclaim = DecisionRecord {
        operation: op("op-reclaim"),
        assignment: assignment_id(),
        authority: lead_authority("state:reclaim"),
        kind: DecisionKind::Reclaim {
            attempt: attempt_id(),
            reason: reason("lease expired"),
        },
        resolves: None,
    };
    assert_eq!(port.record_decision(&reclaim), Ok(StateApplied::Applied));

    let successor = AttemptOpening {
        authorizing: RetryDecision {
            operation: op("op-retry"),
            assignment: assignment_id(),
            authority: lead_authority("state:retry"),
            reason: reason("reclaimed predecessor"),
        },
        attempt: AttemptRecord {
            id: AttemptId::new("att-contract-2").expect("valid attempt"),
            assignment: assignment_id(),
            lease: Lease {
                token: FencingToken(4),
                expires_at: Timestamp(300),
            },
        },
        worker_credential: CredentialProvisioning {
            id: CredentialId::new("cred-contract-2").expect("valid credential"),
            digest: hash('6'),
        },
    };
    let mut nonmonotonic = successor.clone();
    nonmonotonic.attempt.lease.token = FencingToken(3);
    assert_eq!(
        port.append_attempt(&nonmonotonic),
        Err(StateError::IncoherentBundle),
        "a successor token must advance monotonically"
    );
    assert_eq!(port.append_attempt(&successor), Ok(StateApplied::Applied));
    assert_eq!(
        port.append_attempt(&successor),
        Ok(StateApplied::AlreadyApplied)
    );
    let stale_successor = FencedAction {
        call: FencedCall {
            assignment: assignment_id(),
            attempt: successor.attempt.id.clone(),
            actor: worker().actor,
            token: FencingToken(3),
            operation: op("op-stale-successor"),
        },
        responds_to: None,
    };
    assert_eq!(
        port.fenced_evidence(&stale_successor, &evidence()),
        Err(StateError::StaleFencing),
        "the successor's monotonic token fences the predecessor token"
    );
    assert_eq!(
        port.verify_launch_subject(&worker_subject(), &hash('f')),
        Err(StateError::CredentialRevoked)
    );
    let successor_subject = LaunchSubject::WorkerAttempt {
        attempt: successor.attempt.id.clone(),
        credential: successor.worker_credential.id.clone(),
    };
    assert_eq!(
        port.verify_launch_subject(&successor_subject, &successor.worker_credential.digest),
        Ok(())
    );

    let capped_harness = build(Timestamp(50));
    let capped = capped_harness.port();
    let mut capped_opening = opening();
    capped_opening.assignment.attempt_policy = AttemptPolicy {
        cap: Some(AttemptCap::new(1).expect("valid attempt cap")),
    };
    assert_eq!(
        capped.open_assignment(&capped_opening),
        Ok(StateApplied::Applied)
    );
    capped_harness.set_now(Timestamp(101));
    assert_eq!(capped.record_decision(&reclaim), Ok(StateApplied::Applied));
    assert_eq!(
        capped.append_attempt(&successor),
        Err(StateError::IncoherentBundle),
        "the assignment's authored attempt cap is enforced on explicit retry"
    );
    assert_eq!(
        capped
            .assignment(&assignment_id())
            .expect("exists")
            .attempts,
        vec![(attempt_id(), AttemptState::Expired)]
    );
}

fn amend(id: &str) -> SignalDraft {
    directive_draft(
        id,
        DirectiveKind::Amend {
            instruction: BoundedText::new("also update the portable fixture")
                .expect("valid directive text"),
        },
    )
}

fn pause(id: &str) -> SignalDraft {
    directive_draft(
        id,
        DirectiveKind::Pause {
            reason: BoundedText::new("wait for operator review").expect("valid directive text"),
        },
    )
}

fn abort(id: &str) -> SignalDraft {
    directive_draft(
        id,
        DirectiveKind::Abort {
            reason: BoundedText::new("the assignment was superseded")
                .expect("valid directive text"),
        },
    )
}

fn response_links_are_causal_and_idempotent<H: StateContractHarness>(
    build: &impl Fn(Timestamp) -> H,
) {
    let harness = build(Timestamp(50));
    let port = harness.port();
    open(port);
    let directive = amend("sig-amend");
    let (committed, _) = port
        .append_signal(&directive)
        .expect("decision actor may append a directive");

    let (outcome, first_response) = port
        .fenced_evidence(&action("op-evidence-unlinked", None), &evidence())
        .expect("amend does not gate evidence");
    assert_eq!(outcome, EvidenceOutcome::Recorded);
    assert_eq!(first_response.binding_directives, vec![committed.clone()]);

    let linked = action("op-evidence-linked", Some(committed.id.clone()));
    let (outcome, response) = port
        .fenced_evidence(&linked, &evidence())
        .expect("linked evidence records");
    assert_eq!(outcome, EvidenceOutcome::Recorded);
    assert!(
        response.binding_directives.is_empty(),
        "the response to the discharging call is post-commit"
    );
    let head_after_link = response.head;
    let records_after_link = port.evidence_for(&attempt_id()).expect("query succeeds");
    assert_eq!(records_after_link.len(), 2);

    let (retry_outcome, retry_response) = port
        .fenced_evidence(&linked, &evidence())
        .expect("identical replay succeeds");
    assert_eq!(retry_outcome, EvidenceOutcome::Recorded);
    assert_eq!(retry_response.applied, StateApplied::AlreadyApplied);
    assert_eq!(retry_response.head, head_after_link);
    assert_eq!(
        port.evidence_for(&attempt_id()).expect("query succeeds"),
        records_after_link,
        "replay never duplicates the substantive record"
    );
    assert_eq!(
        port.fenced_evidence(&action("op-evidence-linked", None), &evidence()),
        Err(StateError::ConflictingOperation),
        "the response link participates in idempotent call identity"
    );
    for altered in [
        FencedAction {
            call: FencedCall {
                assignment: AssignmentId::new("asg-altered").expect("valid id"),
                ..linked.call.clone()
            },
            ..linked.clone()
        },
        FencedAction {
            call: FencedCall {
                actor: actor("worker-altered"),
                ..linked.call.clone()
            },
            ..linked.clone()
        },
        FencedAction {
            call: FencedCall {
                token: FencingToken(99),
                ..linked.call.clone()
            },
            ..linked.clone()
        },
    ] {
        assert_eq!(
            port.fenced_evidence(&altered, &evidence()),
            Err(StateError::ConflictingOperation),
            "every fenced identity field participates in replay identity"
        );
    }

    let causal_harness = build(Timestamp(50));
    let causal = causal_harness.port();
    open(causal);
    let earlier = action("op-before-directive", None);
    causal
        .fenced_evidence(&earlier, &evidence())
        .expect("earlier action records");
    let (later_directive, _) = causal
        .append_signal(&amend("sig-later-amend"))
        .expect("later directive records");
    let (_, replay) = causal
        .fenced_evidence(&earlier, &evidence())
        .expect("earlier action replays");
    assert_eq!(replay.applied, StateApplied::AlreadyApplied);
    assert_eq!(
        replay.binding_directives,
        vec![later_directive],
        "an earlier action cannot discharge a later directive, while replay returns a live envelope"
    );

    let foreign_opening = opening_for(
        "asg-contract-foreign",
        "att-contract-foreign",
        "cred-contract-foreign",
        "op-open-foreign",
    );
    assert_eq!(
        causal.open_assignment(&foreign_opening),
        Ok(StateApplied::Applied)
    );
    let foreign_directive = SignalDraft {
        id: SignalId::new("sig-foreign-directive").expect("valid signal id"),
        sender: lead_authority("state:directive"),
        subject: SubjectRef::Attempt(foreign_opening.first_attempt.id.clone()),
        body: SignalBody::Directive {
            assignment: foreign_opening.assignment.id.clone(),
            attempt: foreign_opening.first_attempt.id.clone(),
            kind: DirectiveKind::Amend {
                instruction: BoundedText::new("foreign instruction").expect("valid text"),
            },
        },
    };
    causal
        .append_signal(&foreign_directive)
        .expect("foreign directive commits to its own attempt");
    let head_before_foreign_link = causal.assignment(&assignment_id()).expect("exists").head;
    assert_eq!(
        causal.fenced_evidence(
            &action(
                "op-foreign-response-link",
                Some(foreign_directive.id.clone())
            ),
            &evidence()
        ),
        Err(StateError::IncoherentBundle),
        "a response link cannot cross attempt or assignment identity"
    );
    assert_eq!(
        causal.assignment(&assignment_id()).expect("exists").head,
        head_before_foreign_link,
        "foreign-target validation refuses without a call record"
    );
}

#[derive(Debug, PartialEq, Eq)]
struct VisibleAttemptFacts {
    view: AssignmentView,
    signals: Vec<Signal>,
    evidence: Vec<EvidenceRecord>,
    unresolved: Vec<Signal>,
    pending: Vec<PendingApplication>,
}

fn visible_facts(port: &dyn WorkflowStatePort) -> VisibleAttemptFacts {
    VisibleAttemptFacts {
        view: port
            .assignment(&assignment_id())
            .expect("assignment exists"),
        signals: port
            .signals_for(&attempt_id())
            .expect("signal query succeeds"),
        evidence: port
            .evidence_for(&attempt_id())
            .expect("evidence query succeeds"),
        unresolved: port.unresolved_signals(None).expect("query succeeds"),
        pending: port.pending_applications().expect("query succeeds"),
    }
}

fn assert_only_call_order_advanced_among_workflow_facts(
    before: &VisibleAttemptFacts,
    after: &VisibleAttemptFacts,
) {
    assert_eq!(after.view.record, before.view.record);
    assert_eq!(after.view.state, before.view.state);
    assert_eq!(after.view.attempts, before.view.attempts);
    assert_eq!(after.view.head, Seq(before.view.head.0 + 1));
    assert_eq!(after.signals, before.signals);
    assert_eq!(after.evidence, before.evidence);
    assert_eq!(after.unresolved, before.unresolved);
    assert_eq!(after.pending, before.pending);
}

fn abort_refusals_are_response_bearing_and_trace_free<H: StateContractHarness>(
    build: &impl Fn(Timestamp) -> H,
) {
    let harness = build(Timestamp(50));
    let port = harness.port();
    open(port);
    let (abort, _) = port
        .append_signal(&abort("sig-abort"))
        .expect("abort commits");

    let before_validation_refusal = visible_facts(port);
    let unknown_link = action(
        "op-aborted-report",
        Some(SignalId::new("sig-unknown").expect("valid signal id")),
    );
    let report = report_draft("sig-refused-report");
    assert_eq!(
        port.fenced_report(&unknown_link, &report),
        Err(StateError::UnknownRecord),
        "link validation precedes the abort gate"
    );
    assert_eq!(
        visible_facts(port),
        before_validation_refusal,
        "a validation refusal changes no derived or durable fact"
    );

    let valid = action("op-aborted-report", None);
    let (outcome, refusal) = port
        .fenced_report(&valid, &report)
        .expect("abort refusal is returned in-band");
    assert_eq!(
        outcome,
        ReportOutcome::Refused {
            reason: DirectiveGateRefusal::AbortInForce
        }
    );
    assert_eq!(refusal.applied, StateApplied::Applied);
    assert_eq!(refusal.binding_directives, vec![abort.clone()]);
    assert_eq!(refusal.head, Seq(before_validation_refusal.view.head.0 + 1));
    assert_only_call_order_advanced_among_workflow_facts(
        &before_validation_refusal,
        &visible_facts(port),
    );
    assert_eq!(
        port.signals_for(&attempt_id()).expect("query succeeds"),
        vec![abort.clone()],
        "the refused report draft remains uncommitted"
    );
    assert!(
        port.evidence_for(&attempt_id())
            .expect("query succeeds")
            .is_empty()
    );
    assert_eq!(
        port.assignment(&assignment_id()).expect("exists").state,
        AssignmentState::Active
    );

    let (pause, _) = port
        .append_signal(&pause("sig-after-refusal"))
        .expect("a later directive commits");
    let (retry_outcome, retry) = port
        .fenced_report(&valid, &report)
        .expect("refusal replay succeeds");
    assert_eq!(retry_outcome, outcome);
    assert_eq!(retry.applied, StateApplied::AlreadyApplied);
    assert_eq!(
        retry.binding_directives,
        vec![abort.clone(), pause],
        "refusal replay returns the current causal envelope"
    );
    let changed_link = action("op-aborted-report", Some(abort.id.clone()));
    assert_eq!(
        port.fenced_report(&changed_link, &report),
        Err(StateError::ConflictingOperation)
    );

    let before_evidence_refusal = visible_facts(port);
    let (outcome, evidence_refusal) = port
        .fenced_evidence(&action("op-aborted-evidence", None), &evidence())
        .expect("abort evidence refusal is in-band");
    assert_eq!(
        outcome,
        EvidenceOutcome::Refused {
            reason: DirectiveGateRefusal::AbortInForce
        }
    );
    assert_eq!(
        evidence_refusal.head,
        Seq(before_evidence_refusal.view.head.0 + 1),
        "ordinary gate refusal owns its operation and call ordering"
    );
    assert_only_call_order_advanced_among_workflow_facts(
        &before_evidence_refusal,
        &visible_facts(port),
    );
    assert!(
        port.evidence_for(&attempt_id())
            .expect("query succeeds")
            .is_empty()
    );

    let (lease, renewal) = port
        .renew_lease(&call("op-renew-under-abort"), Timestamp(150))
        .expect("renewal remains the abort discovery path");
    assert_eq!(lease.expires_at, Timestamp(150));
    assert!(renewal.binding_directives.iter().any(|s| s.id == abort.id));

    let report_events = port
        .audit_events(&AuditQuery {
            class: Some(AuditClass::Report),
            ..AuditQuery::default()
        })
        .expect("report audit query succeeds");
    assert_eq!(report_events.len(), 1);
    assert_eq!(report_events[0].seq, refusal.head);
    assert_eq!(
        report_events[0].kind,
        AuditKind::ReportRefused {
            reason: DirectiveGateRefusal::AbortInForce,
        }
    );
    let evidence_events = port
        .audit_events(&AuditQuery {
            class: Some(AuditClass::Evidence),
            ..AuditQuery::default()
        })
        .expect("evidence audit query succeeds");
    assert_eq!(evidence_events.len(), 1);
    assert_eq!(evidence_events[0].seq, evidence_refusal.head);
    assert_eq!(
        evidence_events[0].kind,
        AuditKind::EvidenceRefused {
            reason: DirectiveGateRefusal::AbortInForce,
        }
    );
}

fn abort_terminal_is_explicit_idempotent_and_causal<H: StateContractHarness>(
    build: &impl Fn(Timestamp) -> H,
) {
    let harness = build(Timestamp(50));
    let port = harness.port();
    open(port);

    let abort_call = call("op-abort-terminal");
    let before_missing_abort = visible_facts(port);
    let audit_before_missing_abort = port
        .audit_events(&AuditQuery::default())
        .expect("audit query succeeds");
    assert_eq!(
        port.fenced_abort_attempt(&abort_call),
        Err(StateError::AbortNotInForce),
        "workers cannot invent voluntary terminal actions"
    );
    assert_eq!(visible_facts(port), before_missing_abort);
    assert_eq!(
        port.audit_events(&AuditQuery::default())
            .expect("audit query succeeds"),
        audit_before_missing_abort,
        "a precondition refusal claims no operation and appends no audit"
    );

    let (amend, _) = port
        .append_signal(&amend("sig-terminal-amend"))
        .expect("amend commits");
    port.append_signal(&pause("sig-terminal-pause"))
        .expect("pause commits");
    port.append_signal(&abort("sig-terminal-abort"))
        .expect("abort commits");

    let response = port
        .fenced_abort_attempt(&abort_call)
        .expect("a live worker may comply with binding Abort");
    assert_eq!(response.applied, StateApplied::Applied);
    assert_eq!(
        response.binding_directives,
        vec![amend.clone()],
        "the same-call terminal action discharges Abort and Pause but not Amend"
    );
    let view = port
        .assignment(&assignment_id())
        .expect("assignment exists");
    assert_eq!(view.state, AssignmentState::Active);
    assert_eq!(view.attempts, vec![(attempt_id(), AttemptState::Aborted)]);
    assert_eq!(
        port.verify_launch_subject(&worker_subject(), &hash('f')),
        Err(StateError::CredentialRevoked)
    );

    let attempt_events = port
        .audit_events(&AuditQuery {
            class: Some(AuditClass::Attempt),
            ..AuditQuery::default()
        })
        .expect("attempt audit query succeeds");
    assert_eq!(attempt_events.len(), 1);
    assert_eq!(
        attempt_events[0],
        AuditEvent {
            seq: response.head,
            at: Timestamp(50),
            initiator: AuditInitiator::WorkerBinding {
                actor: worker(),
                assignment: assignment_id(),
                attempt: attempt_id(),
            },
            operation: AuditOperation::Operation(op("op-abort-terminal")),
            subject: AuditSubject::Workflow(SubjectRef::Attempt(attempt_id())),
            kind: AuditKind::AttemptAborted,
        }
    );

    let audit_after_success = port
        .audit_events(&AuditQuery::default())
        .expect("audit query succeeds");
    let replay = port
        .fenced_abort_attempt(&abort_call)
        .expect("terminal replay bypasses ended state and revoked credential");
    assert_eq!(replay.applied, StateApplied::AlreadyApplied);
    assert_eq!(replay.head, response.head);
    assert_eq!(replay.binding_directives, vec![amend]);
    assert_eq!(
        port.audit_events(&AuditQuery::default())
            .expect("audit query succeeds"),
        audit_after_success,
        "exact replay allocates neither order nor audit"
    );
    let changed_actor = FencedCall {
        actor: actor("different-worker"),
        ..abort_call.clone()
    };
    assert_eq!(
        port.fenced_abort_attempt(&changed_actor),
        Err(StateError::ConflictingOperation),
        "all fenced identity fields participate before mutable validation"
    );
    assert_eq!(
        port.append_signal(&pause("sig-after-aborted")),
        Err(StateError::IncoherentBundle),
        "an ended Attempt cannot receive a Directive it has no worker path to honor"
    );

    let successor = AttemptOpening {
        authorizing: RetryDecision {
            operation: op("op-retry-after-abort"),
            assignment: assignment_id(),
            authority: lead_authority("state:retry"),
            reason: reason("continue after abort compliance"),
        },
        attempt: AttemptRecord {
            id: AttemptId::new("att-contract-after-abort").expect("valid attempt"),
            assignment: assignment_id(),
            lease: Lease {
                token: FencingToken(4),
                expires_at: Timestamp(200),
            },
        },
        worker_credential: CredentialProvisioning {
            id: CredentialId::new("cred-contract-after-abort").expect("valid credential"),
            digest: hash('6'),
        },
    };
    assert_eq!(port.append_attempt(&successor), Ok(StateApplied::Applied));
    assert_eq!(
        port.assignment(&assignment_id())
            .expect("assignment exists")
            .attempts,
        vec![
            (attempt_id(), AttemptState::Aborted),
            (successor.attempt.id, AttemptState::Active),
        ],
        "Aborted is an ended state eligible for explicit retry"
    );

    let expired_harness = build(Timestamp(50));
    let expired = expired_harness.port();
    open(expired);
    expired
        .append_signal(&abort("sig-expired-abort"))
        .expect("abort commits while the lease is live");
    expired_harness.set_now(Timestamp(101));
    let audit_before_expired_call = expired
        .audit_events(&AuditQuery::default())
        .expect("audit query succeeds");
    assert_eq!(
        expired.fenced_abort_attempt(&call("op-expired-abort")),
        Err(StateError::LeaseExpired),
        "abort compliance still requires a live lease"
    );
    assert_eq!(
        expired
            .audit_events(&AuditQuery::default())
            .expect("audit query succeeds"),
        audit_before_expired_call
    );
}

fn decision_terminals_discharge_abort_and_pause<H: StateContractHarness>(
    build: &impl Fn(Timestamp) -> H,
) {
    assert_decision_terminal_case(
        build,
        "revoke",
        DecisionKind::Revoke {
            attempt: attempt_id(),
            reason: reason("decision actor revoked the attempt"),
        },
        None,
        AssignmentState::Active,
        AttemptState::Revoked,
        AuditDecisionKind::Revoke,
        AuditSubject::Workflow(SubjectRef::Attempt(attempt_id())),
    );
    assert_decision_terminal_case(
        build,
        "reclaim",
        DecisionKind::Reclaim {
            attempt: attempt_id(),
            reason: reason("decision actor reclaimed the expired attempt"),
        },
        Some(Timestamp(101)),
        AssignmentState::Active,
        AttemptState::Expired,
        AuditDecisionKind::Reclaim,
        AuditSubject::Workflow(SubjectRef::Attempt(attempt_id())),
    );
    assert_decision_terminal_case(
        build,
        "cancel",
        DecisionKind::Cancel {
            reason: reason("decision actor cancelled the assignment"),
        },
        None,
        AssignmentState::Cancelled,
        AttemptState::Revoked,
        AuditDecisionKind::Cancel,
        AuditSubject::Workflow(SubjectRef::Assignment(assignment_id())),
    );
}

#[allow(clippy::too_many_arguments)]
fn assert_decision_terminal_case<H: StateContractHarness>(
    build: &impl Fn(Timestamp) -> H,
    case: &str,
    kind: DecisionKind,
    decision_time: Option<Timestamp>,
    expected_assignment: AssignmentState,
    expected_attempt: AttemptState,
    expected_kind: AuditDecisionKind,
    expected_subject: AuditSubject,
) {
    let harness = build(Timestamp(50));
    let port = harness.port();
    open(port);
    let carriage = action(&format!("op-before-terminal-{case}"), None);
    assert_eq!(
        port.fenced_evidence(&carriage, &evidence())
            .expect("pre-terminal response carriage commits")
            .0,
        EvidenceOutcome::Recorded
    );
    let (amend, _) = port
        .append_signal(&amend(&format!("sig-{case}-amend")))
        .expect("amend commits");
    port.append_signal(&pause(&format!("sig-{case}-pause")))
        .expect("pause commits");
    port.append_signal(&abort(&format!("sig-{case}-abort")))
        .expect("abort commits");
    if let Some(now) = decision_time {
        harness.set_now(now);
    }

    let decision = DecisionRecord {
        operation: op(&format!("op-decision-terminal-{case}")),
        assignment: assignment_id(),
        authority: lead_authority("state:decide"),
        kind,
        resolves: None,
    };
    assert_eq!(port.record_decision(&decision), Ok(StateApplied::Applied));
    let replay = port
        .fenced_evidence(&carriage, &evidence())
        .expect("earlier response replays after terminal decision")
        .1;
    assert_eq!(replay.applied, StateApplied::AlreadyApplied);
    assert_eq!(
        replay.binding_directives,
        vec![amend],
        "every reachable decision terminal discharges Abort and Pause at its decision Seq"
    );
    let view = port
        .assignment(&assignment_id())
        .expect("assignment exists");
    assert_eq!(view.state, expected_assignment);
    assert_eq!(view.attempts, vec![(attempt_id(), expected_attempt)]);
    let decision_events = port
        .audit_events(&AuditQuery {
            class: Some(AuditClass::Decision),
            ..AuditQuery::default()
        })
        .expect("decision audit query succeeds");
    assert_eq!(decision_events.len(), 1);
    assert_eq!(decision_events[0].seq, replay.head);
    assert_eq!(decision_events[0].subject, expected_subject);
    assert_eq!(
        decision_events[0].kind,
        AuditKind::DecisionRecorded {
            kind: expected_kind,
        }
    );
}

fn pause_and_amend_still_permit_worker_appends<H: StateContractHarness>(
    build: &impl Fn(Timestamp) -> H,
) {
    let harness = build(Timestamp(50));
    let port = harness.port();
    open(port);
    let (amend, _) = port
        .append_signal(&amend("sig-amend-permit"))
        .expect("amend commits");
    let (pause, _) = port
        .append_signal(&pause("sig-pause"))
        .expect("pause commits");

    let report = report_draft("sig-report-under-pause");
    let report_action = action("op-report-under-pause", Some(amend.id.clone()));
    let mut forged_report = report.clone();
    forged_report.sender.actor.profile_hash = hash('0');
    let before_forged_report = visible_facts(port);
    assert_eq!(
        port.fenced_report(&report_action, &forged_report),
        Err(StateError::ActorMismatch),
        "the full worker snapshot, not ActorId alone, binds a report"
    );
    assert_eq!(visible_facts(port), before_forged_report);

    let (report_outcome, report_response) = port
        .fenced_report(&report_action, &report)
        .expect("pause and amend permit reports");
    assert!(matches!(report_outcome, ReportOutcome::Recorded { .. }));
    assert_eq!(report_response.binding_directives, vec![pause.clone()]);
    assert_eq!(
        port.append_signal(&report),
        Err(StateError::IncoherentBundle),
        "even an existing Report has no unfenced replay carriage"
    );

    let (evidence_outcome, evidence_response) = port
        .fenced_evidence(
            &action("op-evidence-under-pause", Some(pause.id.clone())),
            &evidence(),
        )
        .expect("pause permits evidence");
    assert_eq!(evidence_outcome, EvidenceOutcome::Recorded);
    assert_eq!(
        evidence_response.binding_directives,
        vec![pause],
        "carrying a Pause link records an input fact but does not replace kind policy"
    );
}

fn handoff_refusals_and_acceptance_are_transactional<H: StateContractHarness>(
    build: &impl Fn(Timestamp) -> H,
) {
    let harness = build(Timestamp(50));
    let port = harness.port();
    open(port);

    let missing = handoff("handoff-missing", Vec::new());
    let before_missing = visible_facts(port);
    let (outcome, refusal) = port
        .fenced_submit_handoff(&action("op-handoff-missing", None), &missing)
        .expect("missing evidence is an in-band refusal");
    assert_eq!(
        outcome,
        SubmissionOutcome::Refused {
            reason: SubmissionRefusalReason::MissingEvidence
        }
    );
    assert_eq!(refusal.head, Seq(before_missing.view.head.0 + 1));
    assert_only_call_order_advanced_among_workflow_facts(&before_missing, &visible_facts(port));
    assert_eq!(port.handoff(&missing.id), Err(StateError::UnknownRecord));
    assert_eq!(
        port.assignment(&assignment_id()).expect("exists").state,
        AssignmentState::Active
    );
    assert_eq!(
        port.fenced_submit_handoff(&action("op-handoff-missing", None), &missing)
            .expect("refusal replays")
            .1
            .applied,
        StateApplied::AlreadyApplied
    );
    let different = handoff("handoff-different", Vec::new());
    assert_eq!(
        port.fenced_submit_handoff(&action("op-handoff-missing", None), &different),
        Err(StateError::ConflictingOperation)
    );

    assert_eq!(
        port.fenced_evidence(&action("op-handoff-evidence", None), &evidence())
            .expect("evidence records")
            .0,
        EvidenceOutcome::Recorded
    );
    let (amend, _) = port
        .append_signal(&amend("sig-handoff-amend"))
        .expect("amend commits");
    let candidate = handoff("handoff-final", vec![op("op-handoff-evidence")]);
    let before_directive_refusal = visible_facts(port);
    let (outcome, gated_response) = port
        .fenced_submit_handoff(&action("op-handoff-gated", None), &candidate)
        .expect("directive refusal is in-band");
    assert_eq!(
        outcome,
        SubmissionOutcome::Refused {
            reason: SubmissionRefusalReason::Directive(DirectiveGateRefusal::AmendUndischarged)
        }
    );
    assert_eq!(
        gated_response.binding_directives,
        vec![amend.clone()],
        "a refused handoff still returns the directive that explains it"
    );
    assert_only_call_order_advanced_among_workflow_facts(
        &before_directive_refusal,
        &visible_facts(port),
    );
    assert_eq!(port.handoff(&candidate.id), Err(StateError::UnknownRecord));
    assert_eq!(
        port.assignment(&assignment_id()).expect("exists").attempts,
        vec![(attempt_id(), AttemptState::Active)],
        "a refused handoff leaves the attempt active"
    );

    let (recorded, response) = port
        .fenced_submit_handoff(
            &action("op-handoff-record", Some(amend.id.clone())),
            &candidate,
        )
        .expect("same-call discharge permits handoff");
    assert_eq!(
        recorded,
        SubmissionOutcome::Recorded {
            handoff: candidate.id.clone()
        }
    );
    assert!(
        response.binding_directives.is_empty(),
        "the recorded call's response already reflects its discharge"
    );
    assert_eq!(port.handoff(&candidate.id), Ok(candidate.clone()));
    assert_eq!(
        port.assignment(&assignment_id()).expect("exists").attempts,
        vec![(attempt_id(), AttemptState::Submitted)]
    );
    let submitted_facts = visible_facts(port);
    assert_eq!(
        port.append_signal(&pause("sig-directive-after-submission")),
        Err(StateError::IncoherentBundle),
        "Directives may target only an Active Attempt"
    );
    assert_eq!(
        port.fenced_report(
            &action("op-report-after-submission", None),
            &report_draft("sig-report-after-submission")
        ),
        Err(StateError::IncoherentBundle)
    );
    assert_eq!(
        port.fenced_evidence(&action("op-evidence-after-submission", None), &evidence()),
        Err(StateError::IncoherentBundle)
    );
    assert_eq!(
        port.fenced_submit_handoff(
            &action("op-handoff-after-submission", None),
            &handoff("handoff-after-submission", vec![op("op-handoff-evidence")])
        ),
        Err(StateError::IncoherentBundle)
    );
    assert_eq!(
        visible_facts(port),
        submitted_facts,
        "a Submitted attempt accepts no further substantive worker append"
    );

    let acceptance = DecisionRecord {
        operation: op("op-accept-handoff"),
        assignment: assignment_id(),
        authority: lead_authority("state:accept"),
        kind: DecisionKind::Accept {
            handoff: candidate.id.clone(),
            reason: reason("portable suite verified the handoff"),
        },
        resolves: None,
    };
    assert_eq!(port.record_decision(&acceptance), Ok(StateApplied::Applied));
    assert_eq!(
        port.record_decision(&acceptance),
        Ok(StateApplied::AlreadyApplied)
    );
    assert_eq!(port.decision(&acceptance.operation), Ok(acceptance.clone()));
    let view = port.assignment(&assignment_id()).expect("exists");
    assert_eq!(view.state, AssignmentState::Accepted);
    assert_eq!(view.attempts, vec![(attempt_id(), AttemptState::Accepted)]);
    assert_eq!(
        port.verify_launch_subject(&worker_subject(), &hash('f')),
        Err(StateError::CredentialRevoked)
    );
    assert!(
        port.pending_applications()
            .expect("query succeeds")
            .iter()
            .any(|projection| matches!(
                projection.projection,
                WorkProjection::Close {
                    reason: CloseReason::AcceptedHandoff
                }
            ))
    );

    let competing = DecisionRecord {
        operation: op("op-competing-accept"),
        ..acceptance
    };
    assert_eq!(
        port.record_decision(&competing),
        Err(StateError::IncoherentBundle),
        "a terminal assignment cannot accept the same handoff twice"
    );
}

fn unresolved_signals_are_derived_from_responses<H: StateContractHarness>(
    build: &impl Fn(Timestamp) -> H,
) {
    let harness = build(Timestamp(50));
    let port = harness.port();
    open(port);
    let reviewer = actor("reviewer-contract-1");
    let (request, _) = port
        .append_signal(&request_draft("sig-request", lead().actor))
        .expect("request commits");
    let (report_outcome, _) = port
        .fenced_report(
            &action("op-unresolved-report", None),
            &report_draft("sig-unresolved-report"),
        )
        .expect("report commits");
    let ReportOutcome::Recorded { signal: report } = report_outcome else {
        panic!("report must record without abort");
    };

    assert_eq!(
        port.unresolved_signals(None).expect("query succeeds"),
        vec![request.clone(), (*report).clone()],
        "the global set contains causally ordered requests and reports"
    );
    assert_eq!(
        port.unresolved_signals(Some(&reviewer))
            .expect("query succeeds"),
        Vec::<Signal>::new()
    );
    assert_eq!(
        port.unresolved_signals(Some(&lead().actor))
            .expect("query succeeds"),
        vec![request.clone(), (*report).clone()],
        "request addressing and report routing both resolve to the exact decision actor"
    );

    let invalid_answer = directive_draft(
        "sig-answer",
        DirectiveKind::Answer {
            report: SignalId::new("sig-unknown-report").expect("valid signal id"),
            answer: BoundedText::new("the dependency is available").expect("valid answer"),
        },
    );
    let before_invalid_answer = visible_facts(port);
    assert_eq!(
        port.append_signal(&invalid_answer),
        Err(StateError::UnknownRecord)
    );
    assert_eq!(visible_facts(port), before_invalid_answer);
    let answer = directive_draft(
        "sig-answer",
        DirectiveKind::Answer {
            report: report.id.clone(),
            answer: BoundedText::new("the dependency is available").expect("valid answer"),
        },
    );
    port.append_signal(&answer).expect("answer commits");
    assert_eq!(
        port.unresolved_signals(None).expect("query succeeds"),
        vec![request.clone()],
        "a later typed Answer resolves exactly its report"
    );

    let resolution = DecisionRecord {
        operation: op("op-resolve-request"),
        assignment: assignment_id(),
        authority: lead_authority("state:transfer"),
        kind: DecisionKind::TransferAuthority {
            to: DecisionActor {
                actor: actor("lead-contract-2"),
                class: AuthorityClass::Orchestrator,
                profile: ProfileName::new("second-lead").expect("valid profile"),
                profile_hash: hash('4'),
            },
            reason: reason("transfer after reconciliation"),
        },
        resolves: Some(request.id),
    };
    assert_eq!(port.record_decision(&resolution), Ok(StateApplied::Applied));
    assert!(
        port.unresolved_signals(None)
            .expect("query succeeds")
            .is_empty(),
        "a later linked decision resolves the request"
    );
}

fn runtime_associations_use_the_full_launch_subject<H: StateContractHarness>(
    build: &impl Fn(Timestamp) -> H,
) {
    let harness = build(Timestamp(50));
    let port = harness.port();
    open(port);
    let subject = worker_subject();
    let envelope = EnvelopeSnapshot::new("sanitized assignment envelope".into(), hash('5'))
        .expect("bounded envelope");

    assert_eq!(
        port.persist_envelope(&op("op-envelope"), &subject, &envelope),
        Ok(StateApplied::Applied)
    );
    assert_eq!(
        port.persist_envelope(&op("op-envelope"), &subject, &envelope),
        Ok(StateApplied::AlreadyApplied)
    );
    assert_eq!(port.envelope(&subject), Ok(envelope.clone()));
    let changed =
        EnvelopeSnapshot::new("different envelope".into(), hash('6')).expect("bounded envelope");
    assert_eq!(
        port.persist_envelope(&op("op-envelope"), &subject, &changed),
        Err(StateError::ConflictingOperation)
    );

    let wrong_subject = LaunchSubject::WorkerAttempt {
        attempt: attempt_id(),
        credential: CredentialId::new("cred-contract-wrong").expect("valid credential"),
    };
    let head_before_wrong_subject = port.assignment(&assignment_id()).expect("exists").head;
    assert_eq!(
        port.persist_envelope(&op("op-wrong-subject"), &wrong_subject, &envelope),
        Err(StateError::CredentialBindingMismatch)
    );
    assert_eq!(
        port.assignment(&assignment_id()).expect("exists").head,
        head_before_wrong_subject,
        "association validation failures commit nothing"
    );

    let handle = RuntimeHandle::new("runtime-contract-1");
    assert_eq!(
        port.bind_runtime_handle(&op("op-bind"), &subject, &handle),
        Ok(StateApplied::Applied)
    );
    assert_eq!(port.runtime_handle(&subject), Ok(Some(handle.clone())));
    assert_eq!(
        port.bind_runtime_handle(&op("op-bind"), &subject, &handle),
        Ok(StateApplied::AlreadyApplied)
    );
    assert_eq!(
        port.bind_runtime_handle(
            &op("op-bind-conflict"),
            &subject,
            &RuntimeHandle::new("runtime-contract-2")
        ),
        Err(StateError::ConflictingOperation),
        "replacement requires an explicit unbind"
    );
    assert_eq!(
        port.unbind_runtime_handle(&op("op-unbind"), &subject),
        Ok(StateApplied::Applied)
    );
    assert_eq!(port.runtime_handle(&subject), Ok(None));
    let replacement = RuntimeHandle::new("runtime-contract-2");
    assert_eq!(
        port.bind_runtime_handle(&op("op-rebind"), &subject, &replacement),
        Ok(StateApplied::Applied)
    );
    assert_eq!(port.runtime_handle(&subject), Ok(Some(replacement.clone())));
    assert_eq!(
        port.bind_runtime_handle(&op("op-bind"), &subject, &handle),
        Ok(StateApplied::AlreadyApplied),
        "a stale bind replay is absorbed"
    );
    assert_eq!(
        port.unbind_runtime_handle(&op("op-unbind"), &subject),
        Ok(StateApplied::AlreadyApplied),
        "a stale unbind replay is absorbed"
    );
    assert_eq!(
        port.runtime_handle(&subject),
        Ok(Some(replacement)),
        "stale association replays cannot resurrect or erase later state"
    );

    let activation = activation("op-association-boot", "lead-association-1", "lead");
    assert_eq!(
        port.activate_profile(&activation),
        Ok(StateApplied::Applied)
    );
    let actor_subject = LaunchSubject::ActorActivation {
        actor: actor("lead-association-1"),
        profile: ProfileName::new("lead").expect("valid profile"),
        generation: op("op-association-boot"),
        credential: activation.credential.id,
    };
    let actor_envelope = EnvelopeSnapshot::new("sanitized actor envelope".into(), hash('7'))
        .expect("bounded envelope");
    assert_eq!(
        port.persist_envelope(&op("op-actor-envelope"), &actor_subject, &actor_envelope),
        Ok(StateApplied::Applied)
    );
    assert_eq!(port.envelope(&actor_subject), Ok(actor_envelope));
    let actor_handle = RuntimeHandle::new("runtime-actor-contract");
    assert_eq!(
        port.bind_runtime_handle(&op("op-actor-bind"), &actor_subject, &actor_handle),
        Ok(StateApplied::Applied)
    );
    assert_eq!(
        port.runtime_handle(&actor_subject),
        Ok(Some(actor_handle)),
        "actor activations and worker attempts share the closed association seam"
    );
}

fn audit_lineage_is_transactional_typed_and_filterable<H: StateContractHarness>(
    build: &impl Fn(Timestamp) -> H,
) {
    let harness = build(Timestamp(50));
    let port = harness.port();
    let opening = opening();
    assert_eq!(port.open_assignment(&opening), Ok(StateApplied::Applied));
    let opening_events = port
        .audit_events(&AuditQuery::default())
        .expect("audit query succeeds");
    assert_eq!(opening_events.len(), 1);
    assert_eq!(
        opening_events[0],
        AuditEvent {
            seq: Seq(1),
            at: Timestamp(50),
            initiator: AuditInitiator::Authority(opening.authorizing.authority.clone()),
            operation: AuditOperation::Operation(opening.authorizing.operation.clone()),
            subject: AuditSubject::Workflow(SubjectRef::Assignment(assignment_id())),
            kind: AuditKind::AssignmentOpened,
        }
    );
    assert_eq!(
        port.open_assignment(&opening),
        Ok(StateApplied::AlreadyApplied)
    );
    assert_eq!(
        port.audit_events(&AuditQuery::default())
            .expect("audit query succeeds"),
        opening_events,
        "opening replay appends no audit"
    );

    let audit_before_validation = port
        .audit_events(&AuditQuery::default())
        .expect("audit query succeeds");
    assert_eq!(
        port.fenced_evidence(
            &action(
                "op-audit-invalid-link",
                Some(SignalId::new("sig-audit-unknown").expect("valid signal"))
            ),
            &evidence()
        ),
        Err(StateError::UnknownRecord)
    );
    assert_eq!(
        port.audit_events(&AuditQuery::default())
            .expect("audit query succeeds"),
        audit_before_validation,
        "outer validation errors append no audit"
    );

    let (pause, _) = port
        .append_signal(&pause("sig-audit-pause"))
        .expect("directive commits");
    let report_action = action("op-audit-report", None);
    let (report_outcome, report_response) = port
        .fenced_report(&report_action, &report_draft("sig-audit-report"))
        .expect("report commits under Pause");
    let ReportOutcome::Recorded { signal: report } = report_outcome else {
        panic!("report must record without Abort");
    };
    assert_eq!(
        report.seq,
        Seq(3),
        "the payload owns the intermediate position"
    );
    assert_eq!(
        report_response.head,
        Seq(4),
        "the fenced call owns the transaction's final position"
    );

    let (lease, renewal_response) = port
        .renew_lease(&call("op-audit-renew"), Timestamp(150))
        .expect("renewal commits");
    assert_eq!(lease.expires_at, Timestamp(150));
    assert_eq!(renewal_response.head, Seq(5));
    let refusal = handoff("handoff-audit-refusal", Vec::new());
    let (refusal_outcome, refusal_response) = port
        .fenced_submit_handoff(&action("op-audit-handoff-refusal", None), &refusal)
        .expect("ordinary refusal commits");
    assert_eq!(
        refusal_outcome,
        SubmissionOutcome::Refused {
            reason: SubmissionRefusalReason::MissingEvidence
        }
    );
    assert_eq!(refusal_response.head, Seq(6));

    let subject = worker_subject();
    let envelope =
        EnvelopeSnapshot::new("audit envelope".into(), hash('5')).expect("bounded envelope");
    assert_eq!(
        port.persist_envelope(&op("op-audit-envelope"), &subject, &envelope),
        Ok(StateApplied::Applied)
    );
    assert_eq!(
        port.bind_runtime_handle(
            &op("op-audit-bind"),
            &subject,
            &RuntimeHandle::new("runtime-audit")
        ),
        Ok(StateApplied::Applied)
    );
    let observation = RuntimeObservationRecord {
        reporter: lead_authority("state:observe"),
        subject: subject.clone(),
        observation: LivenessObservation {
            observed_at: Timestamp(45),
            kind: LivenessKind::Running,
        },
    };
    assert_eq!(
        port.record_runtime_observation(&op("op-audit-observation"), &observation),
        Ok(StateApplied::Applied)
    );
    assert_eq!(
        port.runtime_observation(&op("op-audit-observation")),
        Ok(observation.clone())
    );

    let application = ApplicationAttempt {
        id: op("op-audit-application"),
        target: op("op-contract-open"),
        outcome: ApplicationOutcome::Applied {
            before: revision('e'),
            after: revision('9'),
        },
    };
    assert_eq!(
        port.record_application_attempt(&application),
        Ok(StateApplied::Applied)
    );
    let receipt = ApplicationReceipt {
        target: application.target.clone(),
        attempt: application.id.clone(),
        after: revision('9'),
    };
    assert_eq!(
        port.record_application_receipt(&receipt),
        Ok(StateApplied::Applied)
    );

    let events = port
        .audit_events(&AuditQuery::default())
        .expect("audit query succeeds");
    assert_eq!(
        events.iter().map(|event| event.seq).collect::<Vec<_>>(),
        vec![
            Seq(1),
            Seq(2),
            Seq(4),
            Seq(5),
            Seq(6),
            Seq(7),
            Seq(8),
            Seq(9),
            Seq(10),
            Seq(11),
        ],
        "each transaction has one final-position event; the Report's intermediate Signal position has none"
    );
    assert_eq!(
        events.iter().filter(|event| event.seq == Seq(4)).count(),
        1,
        "no ordering position carries more than one event"
    );

    let report_events = port
        .audit_events(&AuditQuery {
            subject: Some(AuditSubject::Workflow(SubjectRef::Attempt(attempt_id()))),
            class: Some(AuditClass::Report),
            from: Some(Seq(3)),
            through: Some(Seq(4)),
        })
        .expect("AND-composed report query succeeds");
    assert_eq!(report_events.len(), 1);
    assert_eq!(report_events[0].seq, report_response.head);
    assert_eq!(
        report_events[0].kind,
        AuditKind::ReportRecorded {
            signal: report.id.clone(),
        }
    );
    assert_eq!(
        report_events[0].operation,
        AuditOperation::Operation(report_action.call.operation.clone())
    );
    assert_eq!(
        report_events[0].initiator,
        AuditInitiator::WorkerBinding {
            actor: worker(),
            assignment: assignment_id(),
            attempt: attempt_id(),
        }
    );
    assert_eq!(
        port.audit_events(&AuditQuery {
            subject: Some(AuditSubject::Workflow(SubjectRef::Attempt(attempt_id()))),
            class: Some(AuditClass::Profile),
            ..AuditQuery::default()
        })
        .expect("AND-composed empty query succeeds"),
        Vec::<AuditEvent>::new()
    );

    let signal_event = events
        .iter()
        .find(|event| event.seq == pause.seq)
        .expect("directive audit event exists");
    assert_eq!(
        signal_event.operation,
        AuditOperation::Signal(pause.id.clone())
    );
    let renewal_event = events
        .iter()
        .find(|event| event.seq == renewal_response.head)
        .expect("lease-renewal audit event exists");
    assert_eq!(renewal_event.kind, AuditKind::LeaseRenewed);
    let envelope_event = events
        .iter()
        .find(|event| event.kind == AuditKind::EnvelopePersisted)
        .expect("envelope audit event exists");
    assert_eq!(
        envelope_event.initiator,
        AuditInitiator::SystemProjection {
            authorizing: op("op-contract-open")
        }
    );
    let refusal_event = events
        .iter()
        .find(|event| event.kind.class() == AuditClass::Handoff)
        .expect("handoff audit event exists");
    assert_eq!(
        refusal_event.kind,
        AuditKind::HandoffRefused {
            reason: AuditSubmissionRefusal::MissingEvidence
        }
    );
    let observation_event = events
        .iter()
        .find(|event| event.kind == AuditKind::RuntimeObservationRecorded)
        .expect("observation audit event exists");
    assert_eq!(observation_event.at, Timestamp(50));
    assert_eq!(
        observation_event.initiator,
        AuditInitiator::Authority(observation.reporter.clone())
    );

    let before_replays = events;
    assert_eq!(
        port.fenced_report(&report_action, &report_draft("sig-audit-report"))
            .expect("report replays")
            .1
            .applied,
        StateApplied::AlreadyApplied
    );
    assert_eq!(
        port.record_runtime_observation(&op("op-audit-observation"), &observation),
        Ok(StateApplied::AlreadyApplied)
    );
    let mut changed_observation = observation.clone();
    changed_observation.observation.kind = LivenessKind::Exited;
    assert_eq!(
        port.record_runtime_observation(&op("op-audit-observation"), &changed_observation),
        Err(StateError::ConflictingOperation)
    );
    assert_eq!(
        port.audit_events(&AuditQuery::default())
            .expect("audit query succeeds"),
        before_replays,
        "replay and conflicts append no audit"
    );

    let profile_harness = build(Timestamp(50));
    let profiles = profile_harness.port();
    let boot = activation("op-audit-boot", "lead-audit", "lead");
    assert_eq!(profiles.activate_profile(&boot), Ok(StateApplied::Applied));
    assert_eq!(
        profiles.deactivate_profile(
            &op("op-audit-deactivate"),
            &actor("lead-audit"),
            &ProfileName::new("lead").expect("valid profile")
        ),
        Ok(StateApplied::Applied)
    );
    let profile_events = profiles
        .audit_events(&AuditQuery {
            class: Some(AuditClass::Profile),
            ..AuditQuery::default()
        })
        .expect("profile audit query succeeds");
    assert_eq!(profile_events.len(), 2);
    assert!(
        profile_events
            .iter()
            .all(|event| event.initiator == AuditInitiator::OperatorChannel)
    );
    assert_eq!(profile_events[0].kind, AuditKind::activation(&boot.case));
    assert_eq!(profile_events[1].kind, AuditKind::ProfileDeactivated);
}

fn projection_receipts_clear_only_proven_success<H: StateContractHarness>(
    build: &impl Fn(Timestamp) -> H,
) {
    let ordering_harness = build(Timestamp(50));
    let ordering = ordering_harness.port();
    let mut late_key_opening = opening();
    late_key_opening.authorizing.operation = op("zz-contract-open");
    assert_eq!(
        ordering.open_assignment(&late_key_opening),
        Ok(StateApplied::Applied)
    );
    let early_key_cancel = DecisionRecord {
        operation: op("aa-contract-cancel"),
        assignment: assignment_id(),
        authority: lead_authority("state:cancel"),
        kind: DecisionKind::Cancel {
            reason: reason("assignment became obsolete"),
        },
        resolves: None,
    };
    assert_eq!(
        ordering.record_decision(&early_key_cancel),
        Ok(StateApplied::Applied)
    );
    let ordered = ordering
        .pending_applications()
        .expect("pending query succeeds");
    assert_eq!(ordered.len(), 2);
    assert_eq!(ordered[0].operation, op("zz-contract-open"));
    assert_eq!(ordered[0].projection, WorkProjection::MarkInProgress);
    assert_eq!(ordered[1].operation, op("aa-contract-cancel"));
    assert_eq!(
        ordered[1].projection,
        WorkProjection::Close {
            reason: CloseReason::CancelledObsolete
        }
    );
    assert_eq!(ordered[1].authorized_revision, None);
    assert!(ordered[0].committed_at < ordered[1].committed_at);

    let harness = build(Timestamp(50));
    let port = harness.port();
    open(port);
    let target = op("op-contract-open");

    let head_before_unknown = port.assignment(&assignment_id()).expect("exists").head;
    let unknown = ApplicationAttempt {
        id: op("app-unknown"),
        target: op("op-no-projection"),
        outcome: ApplicationOutcome::Ambiguous,
    };
    assert_eq!(
        port.record_application_attempt(&unknown),
        Err(StateError::UnknownRecord)
    );
    assert_eq!(
        port.assignment(&assignment_id()).expect("exists").head,
        head_before_unknown
    );

    let failed = ApplicationAttempt {
        id: op("app-failed"),
        target: target.clone(),
        outcome: ApplicationOutcome::Failed {
            error: WorkError::Busy,
        },
    };
    assert_eq!(
        port.record_application_attempt(&failed),
        Ok(StateApplied::Applied)
    );
    assert_eq!(
        port.record_application_attempt(&failed),
        Ok(StateApplied::AlreadyApplied)
    );
    assert_eq!(
        port.pending_applications().expect("query succeeds").len(),
        1
    );
    let bad_receipt = ApplicationReceipt {
        target: target.clone(),
        attempt: failed.id.clone(),
        after: revision('9'),
    };
    let head_before_bad_receipt = port.assignment(&assignment_id()).expect("exists").head;
    assert_eq!(
        port.record_application_receipt(&bad_receipt),
        Err(StateError::IncoherentBundle)
    );
    assert_eq!(
        port.assignment(&assignment_id()).expect("exists").head,
        head_before_bad_receipt,
        "a manufactured receipt leaves every projection pending"
    );
    assert_eq!(
        port.pending_applications().expect("query succeeds").len(),
        1
    );

    let successful = ApplicationAttempt {
        id: op("app-success"),
        target: target.clone(),
        outcome: ApplicationOutcome::Applied {
            before: revision('e'),
            after: revision('9'),
        },
    };
    assert_eq!(
        port.record_application_attempt(&successful),
        Ok(StateApplied::Applied)
    );
    let receipt = ApplicationReceipt {
        target,
        attempt: successful.id,
        after: revision('9'),
    };
    assert_eq!(
        port.record_application_receipt(&receipt),
        Ok(StateApplied::Applied)
    );
    assert_eq!(
        port.record_application_receipt(&receipt),
        Ok(StateApplied::AlreadyApplied)
    );
    assert!(
        port.pending_applications()
            .expect("query succeeds")
            .is_empty(),
        "only a receipt linked to a matching successful attempt clears pending"
    );
}
