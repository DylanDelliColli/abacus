# `abacus-core` module contract

Status: Rust domain rules and provider-neutral ports implemented; persistence and
transport implementations continue by migration phase.

## Purpose

`abacus-core` owns the smallest stable domain needed to turn beads into authorized, evidence-backed execution. It provides high leverage through pure rules and use cases while knowing nothing about provider commands, databases, sockets, terminal panes, or user-global configuration.

This module has the broadest downstream impact, so it must change least often. It is not a home for shared convenience code.

## Owns

- Shared identifiers and validation, including `ABACUS-` bead IDs
- The two authority classes: orchestrator and worker
- Generic capability-ID, grant, and responsibility-scope semantics
- Assignment and execution-attempt lifecycles
- Lease and fencing-token rules
- Typed Signal values, subject-reference validation, and resolution-linkage semantics
- Evidence/handoff value semantics
- Acceptance-policy forms, including optional red-green evidence pairing
- Authorization and transition decisions
- Deterministic ready-work fallback policy
- Evidence acceptance policy evaluation
- Provider-neutral ports required by core use cases
- The use-case composition module: functions generic over those ports that sequence multi-seam workflows (ADR-0001 amendment, 2026-08-05). It adds no dependency, holds no provider knowledge, and duplicates no transition policy; `abacus-cli` calls these functions rather than re-implementing them
- Normalized error categories that callers can act on

## Does not own

- Profile file syntax or configuration loading
- Named managers, watchdogs, or historical SABLE roles
- The complete vocabulary of module-owned capabilities
- SQLite tables, migrations, or transport encoding
- `br`/`bv` commands, schemas, paths, or types
- Provider selection, process invocation, or any adapter behavior the use-case module might otherwise be tempted to absorb; composition sequences ports, it never speaks to a provider
- Herdr panes, sessions, socket messages, or status detection
- Git subprocess implementation
- Formatting CLI output
- Push, PR, merge, deployment, CI, or downstream testing methodology
- Generic utility functions used merely by multiple crates

## Deep interface

The external interface is organized around outcomes, not internal steps.

### Identity and authorization

- Validate/create domain identifiers.
- Register an actor as orchestrator or worker.
- Validate a profile's explicit capability set and scope.
- Authorize an actor for a concrete use case and target.
- Snapshot actor ID, profile name/content hash, capability, and scope used for a durable decision.

Capabilities use validated namespaced IDs. Each owning module declares descriptors for its supported use cases; the composition root supplies the known-capability registry. Core evaluates generic grants and scopes without a giant enum of every adapter operation. Unknown capabilities fail profile validation, and no wildcard grants future capabilities. Moving an existing capability between profiles is configuration-only; adding a module-local capability does not change core.

### Assignment use cases

- Create an Assignment from an eligible bead snapshot, worker, scope, evidence/Attempt policy, and exact decision actor/profile hash.
- Authorize explicit start, expiry, revocation, or retry of an execution Attempt.
- Validate lease acquisition/renewal and fencing.
- Issue an immutable Directive to the active Attempt through the Assignment's exact decision authority.
- Submit an immutable Report from the current Attempt under its current lease/fencing token.
- Submit an immutable Request to another in-scope actor and resolve it only with the linked fenced decision.
- Validate every Signal's typed subject and derive its resolution from immutable responding actions.
- Evaluate the current binding Directive set on every fenced worker interaction.
- End the current Attempt through the explicit fenced worker Abort-compliance action when a binding Abort exists.
- Submit a handoff from the current attempt.
- Accept or reject a handoff through the explicitly authorized decision actor.

A submission-precondition failure is an auditable refusal before a Handoff exists and leaves the Attempt active. An explicit Rejection applies only to a recorded Handoff and is terminal for that Attempt, not its Assignment. ABACUS never retries automatically: the Assignment's authorized decision actor may explicitly append a new fenced Attempt under the active Assignment. It does not edit failed history. Accepted and cancelled Assignments are terminal.

Acceptance is one immutable authorizing decision that terminally accepts the Assignment/Attempt before the work-status mutation. A later application receipt confirms that mutation and is not a second decision or lifecycle transition. Decisions lacking a successful receipt are a derived reconciliation set, not a mutable queue or `accepting` state.

### Typed coordination Signals

Every Signal is immutable, idempotently appended, sender-fenced, and carries exactly one validated subject reference: Bead, Assignment, Attempt, or responsibility scope. Decision Signals carry the full exercised authority snapshot: ActorId, authority class, profile name, profile content hash, capability, and scope. A worker submits only `ReportDraft { id, kind }`; Scribe derives its `WorkerBinding { actor, assignment, attempt }`, subject, and Report Attempt from the fenced Attempt locator. This deliberately weaker provenance is honest: Assignment state cannot reconstruct which capability or scope a worker exercised, so neither is fabricated. No Signal variant accepts a subject-free body.

Signals are one closed typed family, not free-form recipient mail:

- A **Directive** is an orchestrator-to-Attempt amended instruction, pause, abort, or answer to a referenced Report. It requires the Assignment's exact decision authority and binds the current Attempt from commit, unread included.
- A **Report** is a worker-to-decision-actor record of structured progress or blocked-with-reason state. It requires the current Attempt and current lease/fencing token.
- A **Request** is an actor-to-actor decision-shaped ask, initially arbitration, authority transfer, reconciliation, or another bounded decision request. It is the orchestrator channel and is resolved only by a linked fenced decision.

A Directive may amend work only within the Assignment's bound bead snapshot, edit scope, and acceptance policy. Widening any of those contracts requires an explicit Assignment/Attempt decision. Abort constrains the remaining legal calls but does not secretly rewrite lifecycle history.

Resolution and discharge are typed workflow semantics, not delivery state:

- an amend or answer Directive is discharged by a later, permitted fenced worker workflow action linked as its substantive response;
- a pause Directive is discharged by a later authorized Directive or terminal Attempt action that supersedes it;
- an abort Directive remains binding until an abort-consistent terminal action;
- a Report is resolved by a linked responding Directive or fenced decision; and
- a Request is resolved by its linked responding fenced decision, including a fenced refusal.

An acknowledgement-only record is not a responding workflow action. Every fenced worker response mechanically surfaces the Attempt's current binding Directives. This is a protocol property of Scribe responses, never worker discipline. Exposure and discharge are derived from immutable call ordering and responding actions: if a Directive committed before the worker's latest fenced call, that call's response surfaced it, and only a causally later permitted action can discharge it.

At the state seam, `FencedCall` carries only the non-secret Attempt locator, fencing token, and operation id. The caller selects which Attempt it acts for but never supplies an actor, Assignment, capability, scope, or other authority claim; Scribe resolves those facts from durable Attempt→Assignment state before mutation. Each substantive worker mutation — Report append, Evidence append, or Handoff submission — carries a `FencedAction` consisting of that call plus an optional `responds_to` Directive link. The link participates in idempotent request identity: an exact replay is absorbed without another action record, while the same operation with a different link is a conflicting duplicate. A present target must be a committed Directive for the same Attempt; unknown and foreign-Attempt targets are typed, mutation-free refusals. Lease renewal remains a bare `FencedCall`, making a response link on lease machinery unrepresentable. Link carriage records an input fact; Directive-kind policy and discharge remain derived in one place.

The type boundary is split deliberately. `WorkerWorkflowStatePort` exposes only worker-safe fenced verbs; worker-facing core use cases are generic only over that trait. `DecisionWorkflowStatePort` explicitly exposes only decision/composition verbs and reads. `WorkflowStatePort` remains the full internal aggregate implemented by the engines so one portable contract can exercise both views; it is never handed to worker composition or dispatch. Adding a worker-reachable decision verb therefore requires an explicit C3 seam change rather than a routing-table edit.

The response envelope is derived after the substantive action commits. Therefore the response to the action that discharges an amend/answer Directive already omits that Directive from its binding set. A replay returns the causally current binding set and Ledger head with replay status, but never allocates another ordering position.

Core refuses consequential actions that conflict with the effective Directive sequence. A Handoff submission made while an amend or pause Directive remains undischarged is an ordinary Submission refusal with a distinct reason; it records no Handoff and leaves the Attempt active. After an abort Directive, further substantive worker mutations are refused except the explicit abort-consistent terminal action; lease renewal is not a substantive worker mutation.

For Report and Evidence, a binding Abort produces a concrete in-band `Refused { AbortInForce }` outcome paired with the normal `FencedResponse`; it is not an outer `StateError`. The audited refusal owns its operation and advances call ordering, but records no Report/Evidence and no responding `WorkerAction`, so malformed or refused work cannot discharge a Directive. Exact replay returns the refusal without duplication and with the causally current response envelope. Validation/authority failures remain outer errors, claim no operation, and take precedence over the Abort gate.

Pause and Amend do not block honest Report or Evidence appends; only Abort does. Lease renewal also remains allowed after Abort because it is lease machinery and its response is a mechanical Directive-discovery path, not a substantive response. Handoff retains the stricter all-kinds gate. `fenced_abort_attempt` is the sole worker Abort-compliance carrier: a bare `FencedCall` under a live lease moves the active Attempt to the distinct ended `Aborted` state, records an abort-consistent terminal action, and returns post-commit Directives. With no binding Abort it returns `AbortNotInForce` and claims nothing; exact replay precedes all validation. A well-formed call from an ended Attempt is `StaleFencing`, not bundle incoherence. Directives may target only an active Attempt. Decision-driven Attempt terminals record the same abort-consistent terminal-action fact at the decision's `Seq`, so historical Abort and Pause discharge stays coherent regardless of who ended the Attempt; Amend remains historically undischargeable but operationally inert after the Attempt ends.

### Audit lineage

The state port exposes a typed audit index, not an event-sourced alternate state model. Every first-time durable mutation has exactly one `AuditEvent` at the transaction's final `Seq`; a recorded fenced Report may first allocate its Signal position, but only the final call position owns the event. Replay adds none, and an outer validation/authority refusal commits nothing. Events carry only a closed kind, typed subject, honest idempotency identity, commit time, and the strongest structurally proven initiator: full authority, recovered worker binding, operator channel, or a system projection joined to a committed authorizing operation. Owning record bodies remain separately joinable.

Audit queries AND-compose typed subject, event class, and inclusive sequence bounds in Ledger order, with no free-text predicate. Actor-reported `RuntimeObservationRecord` values are immutable audit-only facts; their normalized liveness observation never becomes current authority or a completion signal.

No `read_at` columns, per-Directive acknowledgement rows/state, client-asserted “seen head,” mutable inbox, delivery retry, or escalation-on-silence exists. Per-recipient and global unresolved sets are derived from immutable Signals lacking their typed linked responding actions. Lost-response and concurrent-call behavior uses normal idempotency, fencing, and causal protocol ordering; a call cannot leapfrog the response that surfaced a binding Directive.

Signal volume stays bounded by routing doctrine rather than caps: progress is queried; work-shaped blockers become dependency edges in `br`; decision-shaped asks become Requests; and everything else is transient Herdr chatter. Herdr may ring a content-free live doorbell after a Signal commits, but a Signal body never rides the prompt and notification never owns exposure, discharge, or resolution.

If ABACUS ever needs untyped subject-free messages, per-message read/ack state, or escalation-on-silence machinery, that is mail and requires its own ADR rather than another Signal variant.

### Evidence and handoff use cases

- Validate the shape and identity binding of evidence.
- Require a commit object, expected base, clean-tree proof, and policy-required command outcomes.
- Evaluate an Assignment-selected red-green evidence-pair policy without introducing a new evidence record type.
- Bind the Assignment to the bead-content hash used to authorize it and require Acceptance to recheck that hash.
- Require each verification command to record before/after workspace digests so test-induced mutations are visible.
- Require the handed-off commit's changed paths to conform to the Assignment's normalized edit scope.
- Distinguish submitted evidence from independently verified evidence.
- Decide accept/reject without consulting raw pane output.

An Assignment's acceptance policy may select a **red-green evidence pair** form and name the canonical verification command set to which it applies. The policy is fixed as part of the bead-content-hash-bound Assignment; a worker cannot add, remove, or weaken this requirement.

Verification outcomes are normalized at the wrapper boundary into the closed set `pass`, `assert-fail`, and `execution-error`. `assert-fail` means the verification ran to completion and reported an assertion failure. Collection failures, missing files, usage errors, infrastructure failures, and any other failure to run to completion are `execution-error`. Core evaluates this normalized outcome while Evidence retains the honest underlying command and exit details.

The form is satisfied by two ordinary Evidence records produced through the standard wrapper:

- **red:** the policy-named verification files from the worker's current work are overlaid onto an isolated checkout of the Assignment's declared-base implementation, and that composed run records `assert-fail`; and
- **green:** the same policy-named verification set runs natively at the Handoff commit and records `pass`.

The red Evidence binds the declared base commit, the exact overlaid path set, a per-file content digest for every overlaid file, and the composed checkout's before/after workspace digests. Its overlaid paths must be a subset of the policy's verification file set. At acceptance, every overlay digest must equal the digest of the same file in the Handoff commit; verification edited after red capture is stale and must be recaptured. The green Evidence retains its Handoff commit binding and before/after workspace digests. Pairing and overlay validation derive from those existing Evidence values. There is no `RedEvidence`, `GreenEvidence`, pair record, coverage record, or threshold state.

The pair is the structural counter to vacuous verification: a test or command set that cannot fail cannot produce a valid red half, even if it passes at the Handoff commit.

When this policy form is required, green-only or invalid-red submissions fail with distinct policy reasons: red evidence missing, red bound to a commit other than the declared base, the claimed red run actually passed, red produced `execution-error` (`red-errored`), or the overlay digests do not match the Handoff commit (`red-stale`). An overlay path outside the policy's verification file set is malformed evidence and is refused before pairing. The ordinary missing/failing-green reasons continue to apply to the green half. An expectation flag supplied to the wrapper cannot change the recorded exit code or normalized outcome; policy evaluates the honest record.

Red-green pairing is a per-Assignment policy choice, never a universal completion gate or compiled default. The future orchestrator role card defaults to it for unsupervised autonomous runs; core merely validates the explicit policy stored on each Assignment. Downstream projects remain free to choose their verification policy, and core defines no coverage machinery or thresholds.

The initial Git verification implementation may live privately in the composition module behind a core-owned `CommitVerifier` port. If Git behavior grows into a substantial independent implementation, extracting an `abacus-git` module requires a focused ADR rather than expanding core.

### Required ports

Core use cases may define narrow consumer-owned ports for:

- normalized work reads/mutations;
- optional work advice;
- durable workflow persistence/transactions;
- runtime launch/observation/control;
- commit verification;
- clock and ID generation.

These are not a generic plugin framework. A port exists only for behavior a core use case consumes and for which at least a production adapter and hermetic fake are required.

## Key invariants

1. A worker can submit but cannot accept its own handoff.
2. The assignment records its exact decision actor and active profile hash; another manager cannot race acceptance merely because it is an orchestrator.
3. Authority transfer is explicit, fenced, and audited.
4. An expired/replaced attempt cannot mutate current assignment state with an old fencing token.
5. Runtime observations cannot cause completion without evidence validation.
6. Advice cannot make an otherwise ineligible bead assignable.
7. Every terminal decision is append-only and carries actor, authority snapshot, reason, and idempotency identity.
8. Provider identity and payload types never appear in domain values.
9. Current time and ID generation are inputs, not ambient calls.
10. A named profile can be added or split without changing authority-class enums or lifecycle schemas.
11. Overlapping exclusive mutation/decision scopes are invalid configuration; shared observation scopes may overlap.
12. Submission refusal, Handoff Rejection, and Assignment cancellation are distinct outcomes with distinct lifecycle effects.
13. Attempt retry is an explicit fenced decision-actor action; no transition or timer creates one automatically. A retry commits its successor Attempt and durable Attempt→Assignment locator relation atomically; a stale predecessor cannot select the successor's authority.
14. Submission refusal produces an auditable outcome even though it creates no Handoff or decision.
15. Every Signal is append-only, sender-fenced, typed, and bound to a validated workflow subject. Decision Signals retain the full exercised authority snapshot; worker Reports retain the Scribe-derived worker binding and never fabricate capability/scope. No Signal type accepts a subject-free body.
16. Every fenced worker response mechanically surfaces the Attempt's current binding Directives as a Scribe protocol property, and current Directive policy gates consequential mutations regardless of delivery/read status.
17. Acceptance fails if the authoritative bead content no longer matches the Assignment's bound hash, the commit diff escapes edit scope, or verification changed the workspace without an allowed and accounted-for result.
18. Signal exposure and discharge are derived from immutable call ordering and linked responding actions, never from delivery metadata, opening, acknowledgement, or a timer. Only substantive fenced worker actions can carry a Directive response link; lease renewal cannot.
19. A required red-green pair is selected by the bead-content-hash-bound Assignment and derives from ordinary, honestly recorded Evidence: assertion-level red against the declared-base implementation using digest-bound verification overlays, and green at the Handoff commit; workers cannot opt into or out of it.
20. A Directive may target only an active Attempt; `Aborted` is a distinct ended state eligible only for explicit decision-authorized retry.
21. Every first-time durable mutation owns exactly one typed audit event at its transaction's final Ledger position; replay and outer refusal own none.

## Dependency rule

`abacus-core` has no internal ABACUS dependencies. Keep third-party dependencies minimal and domain-appropriate. It cannot import adapter crates, CLI configuration, or provider SDKs.

Serialization details are not part of the domain interface. If derives are used for implementation convenience, wire compatibility remains owned by the module that defines the wire/storage format.

## Evolution and blast radius

| Change | Expected validation |
| --- | --- |
| Internal algorithm/refactor with same interface and invariants | Core tests |
| Add/move a named profile using existing capabilities | No core code; profile/config tests only |
| Add a module-local use case/capability | No core code; owning module plus direct composition tests |
| Add/change generic authorization or scope semantics | Core plus full hermetic consumers; ADR if breaking |
| Change a shared lifecycle invariant or public domain value | Core plus full hermetic consumers; ADR required |
| Add provider-specific information | Reject the change; keep it in the owning adapter |

Before adding a shared type or helper, apply the deletion test: if removing it merely moves trivial code into one caller, it is too shallow for core. If removing it duplicates a domain invariant across several modules, core is earning its keep.

## Test contract

All default tests are pure and deterministic:

- table/property tests for valid and invalid transitions;
- authorization tests across actor class, capability, scope, and target;
- fencing and retry tests with generated event sequences;
- submission-refusal versus Handoff-Rejection lifecycle tests;
- Signal subject/sender fencing, closed-kind validation, per-recipient/global linked-resolution queries, and proof that no read/ack state exists;
- Directive binding-from-commit, authorization, mechanical response surfacing, causal-call ordering, lost-response/idempotency, pause/amend/abort transition gates, and responding-action discharge tests;
- explicit `Aborted` lifecycle, decision-driven terminal-action causality, and seam-local typed audit-identity tests;
- Acceptance decision/application-attempt/application-receipt ordering and crash-window tests;
- bead-hash, edit-scope, before/after-workspace-digest, and evidence-policy tests with fake commit verification;
- red-green pairing tests for matching command sets, missing red, wrong-commit red, passing “red,” green-only submission, `execution-error` rejected as red, overlay-digest mismatch rejected as `red-stale`, matching overlay digests accepted, paths outside the policy verification set rejected, and honest outcome interpretation;
- deterministic fallback-order tests;
- profile redistribution tests proving no named role is hard-coded.

No core test may:

- touch the filesystem or environment;
- invoke Git or a provider executable;
- open a socket/database;
- sleep or depend on wall-clock time;
- import fixtures owned by another module.

Warm test target: under five seconds on the baseline development machine.

## Acceptance criteria

- The complete assignment-to-decision lifecycle can be exercised with in-memory ports.
- Two orchestrator profiles can divide existing capabilities without a new Rust type.
- A Directive, Report, and actor-to-actor Request can be created and resolved through typed responding actions without unread/ack state.
- A stale worker is rejected by fencing in deterministic tests.
- Runtime `done` with no handoff remains incomplete.
- An Assignment requiring red-green evidence accepts only assertion-level red with policy-scoped overlay files whose digests match the Handoff commit, and distinctly rejects green-only, wrong-commit-red, passing-red, red-errored, and red-stale submissions while reusing ordinary Evidence values.
- `bv` absence has no effect on eligibility or correctness.
- Provider and persistence types are absent from the public interface.
