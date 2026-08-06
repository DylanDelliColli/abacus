//! Portable behavioral contract for the runtime seam.
//!
//! Every assertion drives core's `RuntimePort` through the facade over
//! the harness's provider. The hermetic fake peer implements the full
//! harness; the `gyh.2` live Herdr lane implements the compatible
//! subset with real sessions and runs the same rows.

use abacus_core::ports::*;
use abacus_core::{AttemptId, Timestamp};
use std::collections::BTreeMap;

/// Test-side control surface. The fake implements everything; a live
/// harness maps controls onto real provider operations.
pub trait RuntimeContractHarness {
    fn port(&self) -> &dyn RuntimePort;
    /// Simulate provider restart/live-handoff: the session keeps its
    /// pane but its terminal generation rotates.
    fn rotate_generation(&self, correlation: &str);
    /// Force the provider-reported raw status word.
    fn set_raw_status(&self, correlation: &str, raw: &str);
    /// Arm exactly one scripted failure (see the fake's vocabulary).
    fn arm(&self, failure: crate::fake::FakeFailure);
    /// Prompts the provider accepted for the session, in order.
    fn accepted_prompts(&self, correlation: &str) -> Vec<String>;
    /// Startup Envelopes the provider accepted.
    fn startup_deliveries(&self, correlation: &str) -> Vec<String>;
    /// Recorded stop calls (`true` = forced).
    fn stops(&self, correlation: &str) -> Vec<bool>;
    /// Every start invocation's argv and environment.
    fn starts(&self) -> Vec<crate::fake::StartRecord>;
}

fn subject(suffix: &str) -> LaunchSubject {
    LaunchSubject::WorkerAttempt {
        attempt: AttemptId::new(&format!("att-rt-{suffix}")).expect("valid attempt id"),
    }
}

fn spec(suffix: &str) -> LaunchSpec {
    LaunchSpec {
        subject: subject(suffix),
        correlation: LaunchCorrelation::new(&format!("corr-{suffix}")).expect("valid correlation"),
        agent_kind: "claude".to_owned(),
        executable: "/opt/agent/bin".to_owned(),
        args: vec!["--resume".to_owned()],
        working_directory: HostPath::new("/work/repo").expect("valid host path"),
        environment: BTreeMap::from([("LANG".to_owned(), "C.UTF-8".to_owned())]),
        envelope: EnvelopeSnapshot::new(
            "sanitized envelope".into(),
            abacus_core::ContentHash::new(&"a".repeat(64)).expect("valid hash"),
        )
        .expect("bounded envelope"),
        startup_deadline: Timestamp(100),
        delivery_deadline: Timestamp(120),
    }
}

fn launched(port: &dyn RuntimePort, suffix: &str) -> LaunchOutcome {
    match port.launch(&spec(suffix)).expect("launch succeeds") {
        LaunchAttempt::Launched(outcome) => outcome,
        LaunchAttempt::Ambiguous { .. } => panic!("unscripted launch must not be ambiguous"),
    }
}

/// Run the provider-independent runtime contract.
pub fn run_runtime_suite<H, F>(build: F)
where
    H: RuntimeContractHarness,
    F: Fn() -> H,
{
    launch_delivers_envelope_out_of_argv_and_env(&build);
    ambiguous_launch_recovers_by_subject_and_correlation(&build);
    a_foreign_subject_never_rebinds_a_correlation(&build);
    generation_rotation_fences_every_verb_until_reassociation(&build);
    delivery_and_control_ambiguity_is_reported_never_retried(&build);
    doorbell_is_content_free_and_status_words_normalize(&build);
    stop_policy_and_bounded_reads_hold(&build);
    version_drift_and_foreign_handles_fail_closed(&build);
}

fn launch_delivers_envelope_out_of_argv_and_env<H: RuntimeContractHarness>(build: &impl Fn() -> H) {
    let harness = build();
    let outcome = launched(harness.port(), "happy");
    assert_eq!(outcome.startup_delivery, StartupDelivery::Submitted);
    assert_eq!(outcome.observation.kind, LivenessKind::Running);

    let deliveries = harness.startup_deliveries("corr-happy");
    assert_eq!(
        deliveries,
        vec!["sanitized envelope".to_owned()],
        "the Envelope rides the startup channel exactly once"
    );
    for start in harness.starts() {
        assert!(
            !start.args.iter().any(|arg| arg == "sanitized envelope"),
            "the Envelope never appears in argv"
        );
        assert!(
            !start
                .environment
                .values()
                .any(|value| value == "sanitized envelope"),
            "the Envelope never appears in the environment"
        );
    }
    assert!(
        !outcome.handle.as_str().contains("sanitized envelope"),
        "the opaque handle carries no Envelope content"
    );
}

fn ambiguous_launch_recovers_by_subject_and_correlation<H: RuntimeContractHarness>(
    build: &impl Fn() -> H,
) {
    let harness = build();
    let port = harness.port();
    harness.arm(crate::fake::FakeFailure::AmbiguousStartCreated);
    let attempt = port
        .launch(&spec("amb"))
        .expect("ambiguous launch is an outcome, not an error");
    let LaunchAttempt::Ambiguous {
        subject: echoed,
        correlation,
    } = attempt
    else {
        panic!("scripted ambiguity must surface");
    };
    assert_eq!(echoed, subject("amb"));

    let recovered = port
        .recover_launch(&echoed, &correlation, Timestamp(200))
        .expect("recovery resolves")
        .expect("the session exists");
    assert_eq!(
        recovered.startup_delivery,
        StartupDelivery::Ambiguous,
        "a recovered Submitted would be manufactured"
    );
    assert_eq!(
        port.observe(&recovered.handle, Timestamp(210))
            .expect("recovered handle observes")
            .kind,
        LivenessKind::Starting
    );

    let lost = build();
    lost.arm(crate::fake::FakeFailure::AmbiguousStartLost);
    let attempt = lost
        .port()
        .launch(&spec("lost"))
        .expect("ambiguous launch reported");
    let LaunchAttempt::Ambiguous { correlation, .. } = attempt else {
        panic!("scripted ambiguity must surface");
    };
    assert_eq!(
        lost.port()
            .recover_launch(&subject("lost"), &correlation, Timestamp(200))
            .expect("recovery resolves"),
        None,
        "nothing was created; recovery says so honestly"
    );
}

fn a_foreign_subject_never_rebinds_a_correlation<H: RuntimeContractHarness>(
    build: &impl Fn() -> H,
) {
    let harness = build();
    let port = harness.port();
    launched(port, "own");
    assert_eq!(
        port.recover_launch(
            &subject("intruder"),
            &LaunchCorrelation::new("corr-own").expect("valid correlation"),
            Timestamp(200),
        )
        .err(),
        Some(RuntimeError::Rejected),
        "a correlation alone never rebinds a session to another identity"
    );
}

fn generation_rotation_fences_every_verb_until_reassociation<H: RuntimeContractHarness>(
    build: &impl Fn() -> H,
) {
    let harness = build();
    let port = harness.port();
    let outcome = launched(port, "fence");
    let stale = outcome.handle;
    harness.rotate_generation("corr-fence");

    assert_eq!(
        port.observe(&stale, Timestamp(200))
            .expect("observe reads")
            .kind,
        LivenessKind::StaleGeneration,
        "observe reports staleness as an observation"
    );
    assert_eq!(
        port.prompt(&stale, "hello", Timestamp(200)).err(),
        Some(RuntimeError::HandleStale)
    );
    assert_eq!(
        port.wait(&stale, LivenessKind::Idle, Timestamp(200)).err(),
        Some(RuntimeError::HandleStale)
    );
    assert_eq!(
        port.stop(&stale, StopMode::Graceful, Timestamp(200)).err(),
        Some(RuntimeError::HandleStale)
    );
    assert_eq!(
        port.control(&stale, ControlAction::CancelBlockedDialog, Timestamp(200))
            .err(),
        Some(RuntimeError::HandleStale)
    );

    let fresh = port
        .reassociate(&stale, Timestamp(210))
        .expect("explicit re-association crosses the generation");
    assert_ne!(fresh.as_str(), stale.as_str());
    assert_eq!(
        port.prompt(&fresh, "hello again", Timestamp(220)),
        Ok(DeliveryReport::Submitted)
    );
    assert_eq!(
        harness.accepted_prompts("corr-fence"),
        vec!["hello again".to_owned()],
        "no prompt crossed the stale generation"
    );
}

fn delivery_and_control_ambiguity_is_reported_never_retried<H: RuntimeContractHarness>(
    build: &impl Fn() -> H,
) {
    let harness = build();
    let port = harness.port();
    let outcome = launched(port, "amb2");

    harness.arm(crate::fake::FakeFailure::AmbiguousPrompt);
    assert_eq!(
        port.prompt(&outcome.handle, "maybe", Timestamp(200)),
        Ok(DeliveryReport::Ambiguous),
        "deadline after submission is a report, not an error"
    );
    assert_eq!(
        harness.accepted_prompts("corr-amb2").len(),
        1,
        "the facade did not blindly retry the ambiguous prompt"
    );

    harness.arm(crate::fake::FakeFailure::PromptDeadlineBefore);
    assert_eq!(
        port.prompt(&outcome.handle, "never", Timestamp(200)).err(),
        Some(RuntimeError::Timeout),
        "deadline before submission is a definite non-effect"
    );

    harness.arm(crate::fake::FakeFailure::AmbiguousStop);
    assert_eq!(
        port.stop(&outcome.handle, StopMode::Graceful, Timestamp(220)),
        Ok(EffectReport::Ambiguous)
    );

    let refused = build();
    refused.arm(crate::fake::FakeFailure::RefusedDelivery);
    let outcome = launched(refused.port(), "refuse");
    assert_eq!(
        outcome.startup_delivery,
        StartupDelivery::NotDelivered(RuntimeError::Rejected),
        "a definite delivery refusal keeps the handle for stop/reconcile"
    );

    let ambiguous = build();
    ambiguous.arm(crate::fake::FakeFailure::AmbiguousDelivery);
    let outcome = launched(ambiguous.port(), "ambdel");
    assert_eq!(outcome.startup_delivery, StartupDelivery::Ambiguous);
    // The submission DID reach the provider, so exactly one Envelope
    // exists and the unit performs no redelivery. Recovery is the
    // caller's reconcile path, never a runtime retry.
    assert_eq!(
        ambiguous.startup_deliveries("corr-ambdel").len(),
        1,
        "an ambiguous delivery submitted the Envelope exactly once"
    );
    let recovered = ambiguous
        .port()
        .recover_launch(
            &subject("ambdel"),
            &LaunchCorrelation::new("corr-ambdel").expect("valid correlation"),
            Timestamp(300),
        )
        .expect("recovery resolves")
        .expect("the session exists");
    assert_eq!(
        recovered.startup_delivery,
        StartupDelivery::Ambiguous,
        "recovery never upgrades an ambiguous delivery"
    );
    assert_eq!(
        ambiguous.startup_deliveries("corr-ambdel").len(),
        1,
        "no path in the unit redelivers the Envelope"
    );
}

fn doorbell_is_content_free_and_status_words_normalize<H: RuntimeContractHarness>(
    build: &impl Fn() -> H,
) {
    let harness = build();
    let port = harness.port();
    let outcome = launched(port, "bell");
    assert_eq!(
        port.doorbell(&outcome.handle, Timestamp(200)),
        Ok(DeliveryReport::Submitted)
    );
    let prompts = harness.accepted_prompts("corr-bell");
    assert_eq!(prompts.len(), 1);
    assert_eq!(
        prompts[0], "workflow signal available; query unresolved",
        "the doorbell carries no signal content, subject, or id"
    );

    for (raw, expected) in [
        ("working", LivenessKind::Running),
        ("idle", LivenessKind::Idle),
        ("blocked", LivenessKind::Blocked),
        ("exited", LivenessKind::Exited),
        ("some-future-word", LivenessKind::Unknown),
    ] {
        harness.set_raw_status("corr-bell", raw);
        assert_eq!(
            port.observe(&outcome.handle, Timestamp(210))
                .expect("observe reads")
                .kind,
            expected,
            "raw word {raw} must normalize without breaking the seam"
        );
    }
}

fn stop_policy_and_bounded_reads_hold<H: RuntimeContractHarness>(build: &impl Fn() -> H) {
    let harness = build();
    let port = harness.port();
    let outcome = launched(port, "stop");
    assert_eq!(
        port.stop(&outcome.handle, StopMode::Graceful, Timestamp(200)),
        Ok(EffectReport::Applied)
    );
    assert_eq!(
        port.stop(&outcome.handle, StopMode::Forced, Timestamp(210)),
        Ok(EffectReport::Applied)
    );
    assert_eq!(
        harness.stops("corr-stop"),
        vec![false, true],
        "graceful precedes forced and both are distinct"
    );

    let reader = build();
    let port = reader.port();
    let outcome = launched(port, "read");
    reader.set_raw_status("corr-read", "working");
    assert_eq!(
        port.read_view(&outcome.handle, 5, Timestamp(200)),
        Ok(String::new()),
        "an empty view reads empty"
    );
}

fn version_drift_and_foreign_handles_fail_closed<H: RuntimeContractHarness>(
    build: &impl Fn() -> H,
) {
    let harness = build();
    harness.arm(crate::fake::FakeFailure::VersionDrift);
    assert_eq!(
        harness.port().launch(&spec("drift")).err(),
        Some(RuntimeError::VersionMismatch),
        "pinned-identity drift fails closed before any session verb"
    );
    assert!(harness.starts().is_empty());

    let foreign = build();
    assert_eq!(
        foreign
            .port()
            .observe(&RuntimeHandle::new("not-ours"), Timestamp(200))
            .err(),
        Some(RuntimeError::NotFound),
        "a handle this facade never minted is NotFound, not stale"
    );
}
