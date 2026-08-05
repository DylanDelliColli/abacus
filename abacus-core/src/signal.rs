//! Typed coordination Signals: Directive, Report, Request.
//!
//! One closed family (ADR-0001 §8, CONTEXT I19): immutable, idempotently
//! appended, sender-fenced with the full decision-actor snapshot, and
//! bound to exactly one validated workflow subject. No read/ack state
//! exists anywhere in these types — exposure and discharge derive from
//! immutable commit ordering and linked responding workflow actions,
//! and "unresolved" is a pure query over ordered records, exactly like
//! accepted-decisions-lacking-receipts (I10). Attention is a Herdr
//! doorbell; nothing here models delivery.
//!
//! The ordered inputs (commit sequence numbers, response actions) are
//! supplied by the caller: persistence order is `abacus-state`'s fact,
//! and core only derives meaning from it (I13).

use crate::assignment::DecisionActor;
use crate::id::{AssignmentId, AttemptId, BeadId, CapabilityId, IdError, SignalId};

/// Ledger commit order for Signals and responding actions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Seq(pub u64);

/// Canonical scope-expression text used as a Signal subject or fence
/// scope. Shape-validated here; full algebra parsing arrives with the
/// profile schema (ABACUS-9NH.6, ADR-0002) and tightens this seam
/// internally (C0).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ScopeText(String);

impl ScopeText {
    pub fn new(raw: &str) -> Result<Self, IdError> {
        if raw.is_empty() {
            return Err(IdError::Empty);
        }
        if raw.len() > 300 {
            return Err(IdError::TooLong);
        }
        let ok = raw.bytes().all(|b| {
            b.is_ascii_lowercase()
                || b.is_ascii_digit()
                || matches!(b, b' ' | b'=' | b'!' | b'|' | b'&' | b'*' | b'_' | b'-')
        });
        if !ok {
            return Err(IdError::InvalidCharacter);
        }
        Ok(Self(raw.to_owned()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// The four subject shapes (CONTEXT §2; ADR-0002 §5 adds no fifth).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SubjectRef {
    Bead(BeadId),
    Assignment(AssignmentId),
    Attempt(AttemptId),
    Scope(ScopeText),
}

/// Full sender snapshot on every Signal (I17): who, as what, exercising
/// which capability over which scope.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SenderFence {
    pub actor: DecisionActor,
    pub capability: CapabilityId,
    pub scope: ScopeText,
}

/// Directive payloads: binding orchestrator→Attempt instructions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DirectiveKind {
    /// Amended instructions within the bound bead/scope/policy.
    Amend,
    Pause,
    Abort,
    /// Answer to a referenced Report.
    Answer { report: SignalId },
}

/// Report payloads: worker→decision-actor state under the current lease.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReportKind {
    Progress,
    BlockedWithReason,
}

/// Request payloads: the closed set of decision-shaped asks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RequestKind {
    Arbitration,
    AuthorityTransfer,
    Reconciliation,
}

/// The closed Signal family.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SignalBody {
    Directive { assignment: AssignmentId, attempt: AttemptId, kind: DirectiveKind },
    Report { attempt: AttemptId, kind: ReportKind },
    Request { kind: RequestKind },
}

/// One immutable Signal record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Signal {
    pub id: SignalId,
    pub seq: Seq,
    pub sender: SenderFence,
    pub subject: SubjectRef,
    pub body: SignalBody,
}

/// Subject-validation failures (closed-kind rules).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SubjectError {
    /// Directives and Reports must be subject-bound to their Attempt.
    AttemptSubjectRequired,
    /// The Attempt subject does not match the body's Attempt.
    SubjectAttemptMismatch,
}

/// A Signal variant accepts only its legal subject shape: Directives and
/// Reports bind to the exact Attempt they govern; Requests may carry any
/// of the four shapes. No variant accepts a subject-free body — the type
/// makes that unrepresentable.
pub fn validate_subject(body: &SignalBody, subject: &SubjectRef) -> Result<(), SubjectError> {
    match body {
        SignalBody::Directive { attempt, .. } | SignalBody::Report { attempt, .. } => {
            match subject {
                SubjectRef::Attempt(subject_attempt) if subject_attempt == attempt => Ok(()),
                SubjectRef::Attempt(_) => Err(SubjectError::SubjectAttemptMismatch),
                _ => Err(SubjectError::AttemptSubjectRequired),
            }
        }
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
    WorkerAction { attempt: AttemptId, responds_to: Option<SignalId> },
    /// A later authorized Directive on the same Attempt (supersedes an
    /// earlier pause).
    DirectiveCommitted { attempt: AttemptId, directive: SignalId },
    /// A fenced decision, optionally linked as the resolution of a
    /// Report or Request (a fenced refusal resolves too).
    FencedDecision { responds_to: Option<SignalId> },
    /// A terminal Attempt action; `abort_consistent` marks the subset
    /// legal after an abort Directive.
    TerminalAttemptAction { attempt: AttemptId, abort_consistent: bool },
}

/// Derived status of one Directive against the ordered action log.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DirectiveStatus {
    Binding,
    Discharged(Seq),
}

/// Discharge rules (core contract): an amend or answer Directive is
/// discharged by a causally later linked worker action; a pause by a
/// later authorized Directive or terminal action; an abort only by an
/// abort-consistent terminal action. Only actions with `seq` strictly
/// after the Directive's commit count — a call cannot leapfrog the
/// response that surfaced the Directive.
pub fn directive_status(directive: &Signal, actions: &[ResponseAction]) -> DirectiveStatus {
    let SignalBody::Directive { attempt, kind, .. } = &directive.body else {
        return DirectiveStatus::Binding;
    };
    for action in actions.iter().filter(|a| a.seq > directive.seq) {
        let discharges = match (kind, &action.kind) {
            (
                DirectiveKind::Amend | DirectiveKind::Answer { .. },
                ResponseKind::WorkerAction { attempt: acting, responds_to: Some(target) },
            ) => acting == attempt && *target == directive.id,
            (
                DirectiveKind::Pause,
                ResponseKind::DirectiveCommitted { attempt: acting, directive: other },
            ) => acting == attempt && *other != directive.id,
            (
                DirectiveKind::Pause,
                ResponseKind::TerminalAttemptAction { attempt: acting, .. },
            ) => acting == attempt,
            (
                DirectiveKind::Abort,
                ResponseKind::TerminalAttemptAction { attempt: acting, abort_consistent },
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
/// effective Directive sequence (surfaced as Submission refusals with
/// these reasons; the Attempt stays active).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DirectiveGateRefusal {
    AmendUndischarged,
    PauseInForce,
    AbortInForce,
}

/// May a Handoff be submitted under the current binding set? The
/// refusal names the *most constraining* condition — an abort in force
/// outranks a pause, which outranks an undischarged amend — so the
/// worker is told the fact that actually governs its next legal action.
pub fn handoff_gate(binding: &[&Signal]) -> Result<(), DirectiveGateRefusal> {
    let mut refusal: Option<DirectiveGateRefusal> = None;
    for signal in binding {
        if let SignalBody::Directive { kind, .. } = &signal.body {
            let candidate = match kind {
                DirectiveKind::Abort => DirectiveGateRefusal::AbortInForce,
                DirectiveKind::Pause => DirectiveGateRefusal::PauseInForce,
                DirectiveKind::Amend | DirectiveKind::Answer { .. } => {
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
/// responding action. Pure derivation — there is no flag to set.
pub fn unresolved<'a>(signals: &'a [Signal], actions: &[ResponseAction]) -> Vec<&'a Signal> {
    let mut out: Vec<&Signal> = signals
        .iter()
        .filter(|signal| match &signal.body {
            SignalBody::Report { .. } => !actions.iter().any(|a| {
                a.seq > signal.seq
                    && match &a.kind {
                        ResponseKind::FencedDecision { responds_to: Some(t) } => *t == signal.id,
                        _ => false,
                    }
            }) && !signals.iter().any(|other| {
                matches!(
                    &other.body,
                    SignalBody::Directive { kind: DirectiveKind::Answer { report }, .. }
                        if *report == signal.id && other.seq > signal.seq
                )
            }),
            SignalBody::Request { .. } => !actions.iter().any(|a| {
                a.seq > signal.seq
                    && matches!(&a.kind, ResponseKind::FencedDecision { responds_to: Some(t) } if *t == signal.id)
            }),
            SignalBody::Directive { .. } => false,
        })
        .collect();
    out.sort_by_key(|s| s.seq);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::authority::AuthorityClass;
    use crate::content::ContentHash;
    use crate::id::{ActorId, ProfileName};

    fn fence() -> SenderFence {
        SenderFence {
            actor: DecisionActor {
                actor: ActorId::new("lead-1").unwrap(),
                class: AuthorityClass::Orchestrator,
                profile: ProfileName::new("lead").unwrap(),
                profile_hash: ContentHash::new(&"a".repeat(64)).unwrap(),
            },
            capability: CapabilityId::new("state:directive").unwrap(),
            scope: ScopeText::new("*").unwrap(),
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

    fn report(id: &str, seq: u64) -> Signal {
        Signal {
            id: SignalId::new(id).unwrap(),
            seq: Seq(seq),
            sender: fence(),
            subject: SubjectRef::Attempt(attempt()),
            body: SignalBody::Report { attempt: attempt(), kind: ReportKind::BlockedWithReason },
        }
    }

    fn request(id: &str, seq: u64) -> Signal {
        Signal {
            id: SignalId::new(id).unwrap(),
            seq: Seq(seq),
            sender: fence(),
            subject: SubjectRef::Bead(BeadId::new("ABACUS-9nh").unwrap()),
            body: SignalBody::Request { kind: RequestKind::Arbitration },
        }
    }

    #[test]
    fn subject_rules_per_variant() {
        let d = directive("sig-d", 1, DirectiveKind::Amend);
        assert_eq!(validate_subject(&d.body, &d.subject), Ok(()));
        assert_eq!(
            validate_subject(&d.body, &SubjectRef::Bead(BeadId::new("ABACUS-x").unwrap())),
            Err(SubjectError::AttemptSubjectRequired)
        );
        assert_eq!(
            validate_subject(&d.body, &SubjectRef::Attempt(AttemptId::new("att-9").unwrap())),
            Err(SubjectError::SubjectAttemptMismatch)
        );
        let r = request("sig-r", 2);
        assert_eq!(validate_subject(&r.body, &r.subject), Ok(()));
        assert_eq!(
            validate_subject(&r.body, &SubjectRef::Scope(ScopeText::new("area=frontend").unwrap())),
            Ok(())
        );
    }

    #[test]
    fn append_is_idempotent_and_conflicts_are_loud() {
        let mut log = Vec::new();
        let s = report("sig-1", 1);
        assert_eq!(append_idempotent(&mut log, s.clone()), Ok(AppendOutcome::Appended));
        assert_eq!(append_idempotent(&mut log, s.clone()), Ok(AppendOutcome::Duplicate));
        let mut altered = s;
        altered.seq = Seq(9);
        assert_eq!(append_idempotent(&mut log, altered), Err(ConflictingDuplicate));
        assert_eq!(log.len(), 1);
    }

    #[test]
    fn amend_discharges_only_by_linked_later_worker_action() {
        let d = directive("sig-d", 5, DirectiveKind::Amend);
        // Unlinked action: no discharge.
        let unlinked = [ResponseAction {
            seq: Seq(6),
            kind: ResponseKind::WorkerAction { attempt: attempt(), responds_to: None },
        }];
        assert_eq!(directive_status(&d, &unlinked), DirectiveStatus::Binding);
        // Linked but causally earlier: cannot leapfrog.
        let earlier = [ResponseAction {
            seq: Seq(4),
            kind: ResponseKind::WorkerAction {
                attempt: attempt(),
                responds_to: Some(SignalId::new("sig-d").unwrap()),
            },
        }];
        assert_eq!(directive_status(&d, &earlier), DirectiveStatus::Binding);
        // Linked, later, same attempt: discharged.
        let linked = [ResponseAction {
            seq: Seq(7),
            kind: ResponseKind::WorkerAction {
                attempt: attempt(),
                responds_to: Some(SignalId::new("sig-d").unwrap()),
            },
        }];
        assert_eq!(directive_status(&d, &linked), DirectiveStatus::Discharged(Seq(7)));
    }

    #[test]
    fn pause_discharges_by_later_directive_or_terminal_action() {
        let pause = directive("sig-p", 5, DirectiveKind::Pause);
        let later_directive = [ResponseAction {
            seq: Seq(6),
            kind: ResponseKind::DirectiveCommitted {
                attempt: attempt(),
                directive: SignalId::new("sig-q").unwrap(),
            },
        }];
        assert_eq!(directive_status(&pause, &later_directive), DirectiveStatus::Discharged(Seq(6)));
        let terminal = [ResponseAction {
            seq: Seq(8),
            kind: ResponseKind::TerminalAttemptAction { attempt: attempt(), abort_consistent: false },
        }];
        assert_eq!(directive_status(&pause, &terminal), DirectiveStatus::Discharged(Seq(8)));
        // Its own commit event does not discharge it.
        let self_commit = [ResponseAction {
            seq: Seq(6),
            kind: ResponseKind::DirectiveCommitted {
                attempt: attempt(),
                directive: SignalId::new("sig-p").unwrap(),
            },
        }];
        assert_eq!(directive_status(&pause, &self_commit), DirectiveStatus::Binding);
    }

    #[test]
    fn abort_discharges_only_abort_consistently() {
        let abort = directive("sig-a", 5, DirectiveKind::Abort);
        let ordinary_terminal = [ResponseAction {
            seq: Seq(6),
            kind: ResponseKind::TerminalAttemptAction { attempt: attempt(), abort_consistent: false },
        }];
        assert_eq!(directive_status(&abort, &ordinary_terminal), DirectiveStatus::Binding);
        let consistent = [ResponseAction {
            seq: Seq(7),
            kind: ResponseKind::TerminalAttemptAction { attempt: attempt(), abort_consistent: true },
        }];
        assert_eq!(directive_status(&abort, &consistent), DirectiveStatus::Discharged(Seq(7)));
    }

    #[test]
    fn binding_set_is_ordered_and_gates_handoff_distinctly() {
        let amend = directive("sig-1", 3, DirectiveKind::Amend);
        let pause = directive("sig-2", 5, DirectiveKind::Pause);
        let abort = directive("sig-3", 7, DirectiveKind::Abort);
        let signals = vec![pause.clone(), amend.clone(), abort.clone()];
        let binding = binding_directives(&attempt(), &signals, &[]);
        assert_eq!(
            binding.iter().map(|s| s.seq).collect::<Vec<_>>(),
            vec![Seq(3), Seq(5), Seq(7)]
        );
        // Abort outranks pause outranks amend in the refusal.
        assert_eq!(handoff_gate(&binding), Err(DirectiveGateRefusal::AbortInForce));
        let pause_and_amend = vec![pause, amend.clone()];
        let no_abort = binding_directives(&attempt(), &pause_and_amend, &[]);
        assert_eq!(handoff_gate(&no_abort), Err(DirectiveGateRefusal::PauseInForce));
        let amend_only = vec![amend];
        let only_amend = binding_directives(&attempt(), &amend_only, &[]);
        assert_eq!(handoff_gate(&only_amend), Err(DirectiveGateRefusal::AmendUndischarged));
        assert_eq!(handoff_gate(&[]), Ok(()));
    }

    #[test]
    fn unresolved_reports_and_requests_are_derived() {
        let r = report("sig-r", 2);
        let q = request("sig-q", 4);
        let signals = vec![r.clone(), q.clone()];

        // Nothing responded: both unresolved, in order.
        let open = unresolved(&signals, &[]);
        assert_eq!(open.iter().map(|s| s.id.as_str()).collect::<Vec<_>>(), vec!["sig-r", "sig-q"]);

        // A linked fenced decision resolves the Request (a refusal counts).
        let decided = [ResponseAction {
            seq: Seq(5),
            kind: ResponseKind::FencedDecision { responds_to: Some(SignalId::new("sig-q").unwrap()) },
        }];
        let open = unresolved(&signals, &decided);
        assert_eq!(open.iter().map(|s| s.id.as_str()).collect::<Vec<_>>(), vec!["sig-r"]);

        // An answer Directive resolves the Report.
        let answer = directive("sig-ans", 6, DirectiveKind::Answer {
            report: SignalId::new("sig-r").unwrap(),
        });
        let with_answer = vec![r, q, answer];
        let open = unresolved(&with_answer, &decided);
        assert!(open.is_empty());

        // An unlinked decision resolves nothing: no ambient acks.
        let unlinked = [ResponseAction {
            seq: Seq(9),
            kind: ResponseKind::FencedDecision { responds_to: None },
        }];
        assert_eq!(unresolved(&signals, &unlinked).len(), 2);
    }
}
