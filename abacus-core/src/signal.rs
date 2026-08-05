//! Typed coordination Signals: Directive, Report, Request.
//!
//! One closed family (ADR-0001 §8, CONTEXT I19): immutable, idempotently
//! appended, sender-fenced with the full authority snapshot, and bound
//! to exactly one validated workflow subject. Payloads are bounded
//! typed values — never generic mail bodies — and a Request stores its
//! concrete recipient (ADR-0002 §5: addressing, not subject). No
//! read/ack state exists anywhere in these types; exposure and
//! discharge derive from immutable commit ordering and linked
//! responding workflow actions, and "unresolved" is a pure query over
//! ordered records (I10).
//!
//! The ordered inputs (commit sequence numbers, response actions) are
//! supplied by the caller: persistence order is `abacus-state`'s fact,
//! and core only derives meaning from it (I13).

pub use crate::assignment::AuthoritySnapshot;
use crate::id::{ActorId, AssignmentId, AttemptId, BeadId, SignalId};
use crate::scope::ScopeExpr;

/// Ledger commit order for Signals and responding actions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Seq(pub u64);

/// Bounded validated text payload for Signal bodies and instructions:
/// 1..=500 bytes, no control characters. A constraint, not mail.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoundedText(String);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BoundedTextError {
    Empty,
    TooLong,
    InvalidCharacter,
}

impl BoundedText {
    pub fn new(raw: &str) -> Result<Self, BoundedTextError> {
        if raw.is_empty() {
            return Err(BoundedTextError::Empty);
        }
        if raw.len() > 500 {
            return Err(BoundedTextError::TooLong);
        }
        if raw.bytes().any(|b| b.is_ascii_control()) {
            return Err(BoundedTextError::InvalidCharacter);
        }
        Ok(Self(raw.to_owned()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// The four subject shapes (CONTEXT §2; ADR-0002 §5 adds no fifth).
/// Scope subjects are canonical [`ScopeExpr`] values — parsed against
/// declared keys, never raw text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SubjectRef {
    Bead(BeadId),
    Assignment(AssignmentId),
    Attempt(AttemptId),
    Scope(ScopeExpr),
}

/// Directive payloads: binding orchestrator→Attempt instructions with
/// their bounded content.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DirectiveKind {
    /// Amended instructions within the bound bead/scope/policy.
    Amend {
        instruction: BoundedText,
    },
    Pause {
        reason: BoundedText,
    },
    Abort {
        reason: BoundedText,
    },
    /// Answer to a referenced Report.
    Answer {
        report: SignalId,
        answer: BoundedText,
    },
}

/// The closed semantic-phase vocabulary (CONTEXT §5): the only way
/// semantic phase enters the system is worker self-report through the
/// facade, and composed state consumes exactly these values.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SemanticPhase {
    Claimed,
    Verifying,
    HandingOff,
}

/// Report payloads: worker→decision-actor state under the current
/// lease — structured phase for state composition, bounded free text
/// only as an optional annotation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReportKind {
    Progress {
        phase: SemanticPhase,
        summary: Option<BoundedText>,
    },
    BlockedWithReason {
        reason: BoundedText,
    },
}

/// Request payloads: the closed set of decision-shaped asks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RequestKind {
    Arbitration,
    AuthorityTransfer,
    Reconciliation,
}

/// The closed Signal family. A Request stores its resolved concrete
/// recipient — addressing, distinct from its workflow subject.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SignalBody {
    Directive {
        assignment: AssignmentId,
        attempt: AttemptId,
        kind: DirectiveKind,
    },
    Report {
        attempt: AttemptId,
        kind: ReportKind,
    },
    Request {
        recipient: ActorId,
        kind: RequestKind,
        ask: BoundedText,
    },
}

/// One immutable Signal record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Signal {
    pub id: SignalId,
    pub seq: Seq,
    pub sender: AuthoritySnapshot,
    pub subject: SubjectRef,
    pub body: SignalBody,
}

/// Caller-side Signal input: everything but the commit order. Commit
/// order is a Scribe fact — clients never assert `Seq`; Scribe
/// allocates it at commit and returns the stored [`Signal`],
/// identically on first call and idempotent retry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignalDraft {
    pub id: SignalId,
    pub sender: AuthoritySnapshot,
    pub subject: SubjectRef,
    pub body: SignalBody,
}

impl SignalDraft {
    /// Commit this draft at the Scribe-allocated order.
    pub fn commit(self, seq: Seq) -> Signal {
        Signal {
            id: self.id,
            seq,
            sender: self.sender,
            subject: self.subject,
            body: self.body,
        }
    }
}

/// Subject-validation failures (closed-kind rules).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SubjectError {
    /// Directives and Reports must be subject-bound to their Attempt.
    AttemptSubjectRequired,
    /// The Attempt subject does not match the body's Attempt.
    SubjectAttemptMismatch,
}

/// A Signal variant accepts only its legal subject shape: Directives
/// and Reports bind to the exact Attempt they govern; Requests may
/// carry any of the four shapes. No variant accepts a subject-free
/// body — the type makes that unrepresentable.
pub fn validate_subject(body: &SignalBody, subject: &SubjectRef) -> Result<(), SubjectError> {
    match body {
        SignalBody::Directive { attempt, .. } | SignalBody::Report { attempt, .. } => match subject
        {
            SubjectRef::Attempt(subject_attempt) if subject_attempt == attempt => Ok(()),
            SubjectRef::Attempt(_) => Err(SubjectError::SubjectAttemptMismatch),
            _ => Err(SubjectError::AttemptSubjectRequired),
        },
        SignalBody::Request { .. } => Ok(()),
    }
}

/// Idempotent append outcome.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AppendOutcome {
    Appended,
    /// Same id, byte-identical record: the retry case, absorbed.
    Duplicate,
}

/// Idempotent-append conflict: same id, different content — corrupt
/// input refused loudly, never merged.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ConflictingDuplicate;

pub fn append_idempotent(
    log: &mut Vec<Signal>,
    signal: Signal,
) -> Result<AppendOutcome, ConflictingDuplicate> {
    if let Some(existing) = log.iter().find(|s| s.id == signal.id) {
        return if *existing == signal {
            Ok(AppendOutcome::Duplicate)
        } else {
            Err(ConflictingDuplicate)
        };
    }
    log.push(signal);
    Ok(AppendOutcome::Appended)
}

/// A responding workflow action, in Ledger commit order. These are the
/// only things that discharge or resolve a Signal (I19) — an
/// acknowledgement-only record is deliberately unrepresentable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResponseAction {
    pub seq: Seq,
    pub kind: ResponseKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResponseKind {
    /// A permitted fenced worker workflow action, optionally linked as
    /// the substantive response to an amend/answer Directive.
    WorkerAction {
        attempt: AttemptId,
        responds_to: Option<SignalId>,
    },
    /// A later authorized Directive on the same Attempt (supersedes an
    /// earlier pause).
    DirectiveCommitted {
        attempt: AttemptId,
        directive: SignalId,
    },
    /// A fenced decision, optionally linked as the resolution of a
    /// Report or Request (a fenced refusal resolves too).
    FencedDecision { responds_to: Option<SignalId> },
    /// A terminal Attempt action; `abort_consistent` marks the subset
    /// legal after an abort Directive.
    TerminalAttemptAction {
        attempt: AttemptId,
        abort_consistent: bool,
    },
}

/// Derived status of one Directive against the ordered action log.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DirectiveStatus {
    Binding,
    Discharged(Seq),
}

/// Discharge rules: an amend or answer Directive is discharged by a
/// causally later linked worker action; a pause by a later authorized
/// Directive or terminal action; an abort only by an abort-consistent
/// terminal action. Only actions strictly after the Directive's commit
/// count — a call cannot leapfrog the response that surfaced it.
pub fn directive_status(directive: &Signal, actions: &[ResponseAction]) -> DirectiveStatus {
    let SignalBody::Directive { attempt, kind, .. } = &directive.body else {
        return DirectiveStatus::Binding;
    };
    for action in actions.iter().filter(|a| a.seq > directive.seq) {
        let discharges = match (kind, &action.kind) {
            (
                DirectiveKind::Amend { .. } | DirectiveKind::Answer { .. },
                ResponseKind::WorkerAction {
                    attempt: acting,
                    responds_to: Some(target),
                },
            ) => acting == attempt && *target == directive.id,
            (
                DirectiveKind::Pause { .. },
                ResponseKind::DirectiveCommitted {
                    attempt: acting,
                    directive: other,
                },
            ) => acting == attempt && *other != directive.id,
            (
                DirectiveKind::Pause { .. },
                ResponseKind::TerminalAttemptAction {
                    attempt: acting, ..
                },
            ) => acting == attempt,
            (
                DirectiveKind::Abort { .. },
                ResponseKind::TerminalAttemptAction {
                    attempt: acting,
                    abort_consistent,
                },
            ) => acting == attempt && *abort_consistent,
            _ => false,
        };
        if discharges {
            return DirectiveStatus::Discharged(action.seq);
        }
    }
    DirectiveStatus::Binding
}

/// The current binding-Directive set for an Attempt, in commit order —
/// what every fenced worker response mechanically surfaces (a Scribe
/// protocol property, never worker discipline).
pub fn binding_directives<'a>(
    attempt: &AttemptId,
    signals: &'a [Signal],
    actions: &[ResponseAction],
) -> Vec<&'a Signal> {
    let mut out: Vec<&Signal> = signals
        .iter()
        .filter(|s| matches!(&s.body, SignalBody::Directive { attempt: a, .. } if a == attempt))
        .filter(|s| directive_status(s, actions) == DirectiveStatus::Binding)
        .collect();
    out.sort_by_key(|s| s.seq);
    out
}

/// Distinct refusals for consequential actions that conflict with the
/// effective Directive sequence.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DirectiveGateRefusal {
    AmendUndischarged,
    PauseInForce,
    AbortInForce,
}

/// May a Handoff be submitted under the current binding set? The
/// refusal names the most constraining condition — abort outranks
/// pause outranks amend.
pub fn handoff_gate(binding: &[&Signal]) -> Result<(), DirectiveGateRefusal> {
    let mut refusal: Option<DirectiveGateRefusal> = None;
    for signal in binding {
        if let SignalBody::Directive { kind, .. } = &signal.body {
            let candidate = match kind {
                DirectiveKind::Abort { .. } => DirectiveGateRefusal::AbortInForce,
                DirectiveKind::Pause { .. } => DirectiveGateRefusal::PauseInForce,
                DirectiveKind::Amend { .. } | DirectiveKind::Answer { .. } => {
                    DirectiveGateRefusal::AmendUndischarged
                }
            };
            refusal = Some(match (refusal, candidate) {
                (Some(DirectiveGateRefusal::AbortInForce), _)
                | (_, DirectiveGateRefusal::AbortInForce) => DirectiveGateRefusal::AbortInForce,
                (Some(DirectiveGateRefusal::PauseInForce), _)
                | (_, DirectiveGateRefusal::PauseInForce) => DirectiveGateRefusal::PauseInForce,
                _ => DirectiveGateRefusal::AmendUndischarged,
            });
        }
    }
    match refusal {
        Some(r) => Err(r),
        None => Ok(()),
    }
}

/// Unresolved Reports and Requests: Signals lacking their typed linked
/// responding action. Pure derivation — there is no flag to set. When
/// `recipient` is given, Requests are filtered to that stored
/// recipient; Reports resolve to their Assignment's decision actor,
/// which the state port composes, so they appear only in the global
/// (None) query at this layer.
pub fn unresolved<'a>(
    signals: &'a [Signal],
    actions: &[ResponseAction],
    recipient: Option<&ActorId>,
) -> Vec<&'a Signal> {
    let mut out: Vec<&Signal> = signals
        .iter()
        .filter(|signal| match &signal.body {
            SignalBody::Report { .. } => {
                recipient.is_none()
                    && !actions.iter().any(|a| {
                        a.seq > signal.seq
                            && matches!(
                                &a.kind,
                                ResponseKind::FencedDecision { responds_to: Some(t) }
                                    if *t == signal.id
                            )
                    })
                    && !signals.iter().any(|other| {
                        matches!(
                            &other.body,
                            SignalBody::Directive {
                                kind: DirectiveKind::Answer { report, .. },
                                ..
                            } if *report == signal.id && other.seq > signal.seq
                        )
                    })
            }
            SignalBody::Request {
                recipient: stored, ..
            } => {
                recipient.is_none_or(|r| r == stored)
                    && !actions.iter().any(|a| {
                        a.seq > signal.seq
                            && matches!(
                                &a.kind,
                                ResponseKind::FencedDecision { responds_to: Some(t) }
                                    if *t == signal.id
                            )
                    })
            }
            SignalBody::Directive { .. } => false,
        })
        .collect();
    out.sort_by_key(|s| s.seq);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::assignment::DecisionActor;
    use crate::authority::AuthorityClass;
    use crate::content::ContentHash;
    use crate::id::{ActorId, CapabilityId, ProfileName};

    fn text(raw: &str) -> BoundedText {
        BoundedText::new(raw).unwrap()
    }

    fn fence() -> AuthoritySnapshot {
        AuthoritySnapshot {
            actor: DecisionActor {
                actor: ActorId::new("lead-1").unwrap(),
                class: AuthorityClass::Orchestrator,
                profile: ProfileName::new("lead").unwrap(),
                profile_hash: ContentHash::new(&"a".repeat(64)).unwrap(),
            },
            capability: CapabilityId::new("state:directive").unwrap(),
            scope: ScopeExpr::Universal,
        }
    }

    fn attempt() -> AttemptId {
        AttemptId::new("att-1").unwrap()
    }

    fn directive(id: &str, seq: u64, kind: DirectiveKind) -> Signal {
        Signal {
            id: SignalId::new(id).unwrap(),
            seq: Seq(seq),
            sender: fence(),
            subject: SubjectRef::Attempt(attempt()),
            body: SignalBody::Directive {
                assignment: AssignmentId::new("asg-1").unwrap(),
                attempt: attempt(),
                kind,
            },
        }
    }

    fn amend() -> DirectiveKind {
        DirectiveKind::Amend {
            instruction: text("also update the fixture"),
        }
    }

    fn pause() -> DirectiveKind {
        DirectiveKind::Pause {
            reason: text("operator review"),
        }
    }

    fn abort() -> DirectiveKind {
        DirectiveKind::Abort {
            reason: text("superseded"),
        }
    }

    fn report(id: &str, seq: u64) -> Signal {
        Signal {
            id: SignalId::new(id).unwrap(),
            seq: Seq(seq),
            sender: fence(),
            subject: SubjectRef::Attempt(attempt()),
            body: SignalBody::Report {
                attempt: attempt(),
                kind: ReportKind::BlockedWithReason {
                    reason: text("dependency missing"),
                },
            },
        }
    }

    fn request(id: &str, seq: u64, recipient: &str) -> Signal {
        Signal {
            id: SignalId::new(id).unwrap(),
            seq: Seq(seq),
            sender: fence(),
            subject: SubjectRef::Bead(BeadId::new("ABACUS-9nh").unwrap()),
            body: SignalBody::Request {
                recipient: ActorId::new(recipient).unwrap(),
                kind: RequestKind::Arbitration,
                ask: text("who owns this bead"),
            },
        }
    }

    #[test]
    fn bounded_text_is_bounded() {
        assert!(BoundedText::new("progress: half done").is_ok());
        assert_eq!(BoundedText::new(""), Err(BoundedTextError::Empty));
        assert_eq!(
            BoundedText::new(&"x".repeat(501)),
            Err(BoundedTextError::TooLong)
        );
        assert_eq!(
            BoundedText::new("a\nb"),
            Err(BoundedTextError::InvalidCharacter)
        );
    }

    #[test]
    fn subject_rules_per_variant() {
        let d = directive("sig-d", 1, amend());
        assert_eq!(validate_subject(&d.body, &d.subject), Ok(()));
        assert_eq!(
            validate_subject(&d.body, &SubjectRef::Bead(BeadId::new("ABACUS-x").unwrap())),
            Err(SubjectError::AttemptSubjectRequired)
        );
        assert_eq!(
            validate_subject(
                &d.body,
                &SubjectRef::Attempt(AttemptId::new("att-9").unwrap())
            ),
            Err(SubjectError::SubjectAttemptMismatch)
        );
        let r = request("sig-r", 2, "lead-2");
        assert_eq!(validate_subject(&r.body, &r.subject), Ok(()));
        // Scope subjects are canonical expressions, not raw text.
        let scope_subject = SubjectRef::Scope(ScopeExpr::Universal);
        assert_eq!(validate_subject(&r.body, &scope_subject), Ok(()));
    }

    #[test]
    fn append_is_idempotent_and_content_conflicts_are_loud() {
        let mut log = Vec::new();
        let s = report("sig-1", 1);
        assert_eq!(
            append_idempotent(&mut log, s.clone()),
            Ok(AppendOutcome::Appended)
        );
        assert_eq!(
            append_idempotent(&mut log, s.clone()),
            Ok(AppendOutcome::Duplicate)
        );
        // Same id, different payload content: conflict (payloads are
        // part of identity, not decoration).
        let mut altered = s;
        if let SignalBody::Report { kind, .. } = &mut altered.body {
            *kind = ReportKind::Progress {
                phase: SemanticPhase::Verifying,
                summary: Some(text("actually fine")),
            };
        }
        assert_eq!(
            append_idempotent(&mut log, altered),
            Err(ConflictingDuplicate)
        );
        assert_eq!(log.len(), 1);
    }

    #[test]
    fn amend_discharges_only_by_linked_later_worker_action() {
        let d = directive("sig-d", 5, amend());
        let unlinked = [ResponseAction {
            seq: Seq(6),
            kind: ResponseKind::WorkerAction {
                attempt: attempt(),
                responds_to: None,
            },
        }];
        assert_eq!(directive_status(&d, &unlinked), DirectiveStatus::Binding);
        let earlier = [ResponseAction {
            seq: Seq(4),
            kind: ResponseKind::WorkerAction {
                attempt: attempt(),
                responds_to: Some(SignalId::new("sig-d").unwrap()),
            },
        }];
        assert_eq!(directive_status(&d, &earlier), DirectiveStatus::Binding);
        let linked = [ResponseAction {
            seq: Seq(7),
            kind: ResponseKind::WorkerAction {
                attempt: attempt(),
                responds_to: Some(SignalId::new("sig-d").unwrap()),
            },
        }];
        assert_eq!(
            directive_status(&d, &linked),
            DirectiveStatus::Discharged(Seq(7))
        );
    }

    #[test]
    fn pause_and_abort_discharge_rules() {
        let p = directive("sig-p", 5, pause());
        let later_directive = [ResponseAction {
            seq: Seq(6),
            kind: ResponseKind::DirectiveCommitted {
                attempt: attempt(),
                directive: SignalId::new("sig-q").unwrap(),
            },
        }];
        assert_eq!(
            directive_status(&p, &later_directive),
            DirectiveStatus::Discharged(Seq(6))
        );
        let a = directive("sig-a", 5, abort());
        let ordinary = [ResponseAction {
            seq: Seq(6),
            kind: ResponseKind::TerminalAttemptAction {
                attempt: attempt(),
                abort_consistent: false,
            },
        }];
        assert_eq!(directive_status(&a, &ordinary), DirectiveStatus::Binding);
        let consistent = [ResponseAction {
            seq: Seq(7),
            kind: ResponseKind::TerminalAttemptAction {
                attempt: attempt(),
                abort_consistent: true,
            },
        }];
        assert_eq!(
            directive_status(&a, &consistent),
            DirectiveStatus::Discharged(Seq(7))
        );
    }

    #[test]
    fn binding_set_is_ordered_and_gates_handoff_distinctly() {
        let amend_sig = directive("sig-1", 3, amend());
        let pause_sig = directive("sig-2", 5, pause());
        let abort_sig = directive("sig-3", 7, abort());
        let signals = vec![pause_sig.clone(), amend_sig.clone(), abort_sig];
        let binding = binding_directives(&attempt(), &signals, &[]);
        assert_eq!(
            binding.iter().map(|s| s.seq).collect::<Vec<_>>(),
            vec![Seq(3), Seq(5), Seq(7)]
        );
        assert_eq!(
            handoff_gate(&binding),
            Err(DirectiveGateRefusal::AbortInForce)
        );
        let pause_and_amend = vec![pause_sig, amend_sig.clone()];
        let no_abort = binding_directives(&attempt(), &pause_and_amend, &[]);
        assert_eq!(
            handoff_gate(&no_abort),
            Err(DirectiveGateRefusal::PauseInForce)
        );
        let amend_only = vec![amend_sig];
        let only_amend = binding_directives(&attempt(), &amend_only, &[]);
        assert_eq!(
            handoff_gate(&only_amend),
            Err(DirectiveGateRefusal::AmendUndischarged)
        );
        assert_eq!(handoff_gate(&[]), Ok(()));
    }

    #[test]
    fn unresolved_supports_recipient_scoping() {
        let r = report("sig-r", 2);
        let q_for_lead2 = request("sig-q", 4, "lead-2");
        let q_for_lead3 = request("sig-s", 6, "lead-3");
        let signals = vec![r.clone(), q_for_lead2.clone(), q_for_lead3.clone()];

        // Global query: everything unresolved, in order.
        let open = unresolved(&signals, &[], None);
        assert_eq!(
            open.iter().map(|s| s.id.as_str()).collect::<Vec<_>>(),
            vec!["sig-r", "sig-q", "sig-s"]
        );

        // Recipient-scoped query returns only that actor's Requests.
        let lead2 = ActorId::new("lead-2").unwrap();
        let scoped = unresolved(&signals, &[], Some(&lead2));
        assert_eq!(
            scoped.iter().map(|s| s.id.as_str()).collect::<Vec<_>>(),
            vec!["sig-q"]
        );

        // A linked fenced decision resolves the Request (a refusal
        // counts), removing it from every query.
        let decided = [ResponseAction {
            seq: Seq(5),
            kind: ResponseKind::FencedDecision {
                responds_to: Some(SignalId::new("sig-q").unwrap()),
            },
        }];
        assert!(unresolved(&signals, &decided, Some(&lead2)).is_empty());

        // An answer Directive resolves the Report.
        let answer = directive(
            "sig-ans",
            8,
            DirectiveKind::Answer {
                report: SignalId::new("sig-r").unwrap(),
                answer: text("use the pinned version"),
            },
        );
        let with_answer = vec![r, q_for_lead2, q_for_lead3, answer];
        let open = unresolved(&with_answer, &decided, None);
        assert_eq!(
            open.iter().map(|s| s.id.as_str()).collect::<Vec<_>>(),
            vec!["sig-s"]
        );

        // An unlinked decision resolves nothing: no ambient acks.
        let unlinked = [ResponseAction {
            seq: Seq(9),
            kind: ResponseKind::FencedDecision { responds_to: None },
        }];
        assert_eq!(unresolved(&signals, &unlinked, None).len(), 3);
    }
}
