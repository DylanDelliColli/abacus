# `abacus-core` module contract

Status: design contract; no Rust implementation yet

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
- Authorization and transition decisions
- Deterministic ready-work fallback policy
- Evidence acceptance policy evaluation
- Provider-neutral ports required by core use cases
- Normalized error categories that callers can act on

## Does not own

- Profile file syntax or configuration loading
- Named managers, watchdogs, or historical SABLE roles
- The complete vocabulary of module-owned capabilities
- SQLite tables, migrations, or transport encoding
- `br`/`bv` commands, schemas, paths, or types
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
- Submit a handoff from the current attempt.
- Accept or reject a handoff through the explicitly authorized decision actor.

A submission-precondition failure is an auditable refusal before a Handoff exists and leaves the Attempt active. An explicit Rejection applies only to a recorded Handoff and is terminal for that Attempt, not its Assignment. ABACUS never retries automatically: the Assignment's authorized decision actor may explicitly append a new fenced Attempt under the active Assignment. It does not edit failed history. Accepted and cancelled Assignments are terminal.

Acceptance is one immutable authorizing decision that terminally accepts the Assignment/Attempt before the work-status mutation. A later application receipt confirms that mutation and is not a second decision or lifecycle transition. Decisions lacking a successful receipt are a derived reconciliation set, not a mutable queue or `accepting` state.

### Typed coordination Signals

Every Signal is immutable, idempotently appended, and sender-fenced with the actor's full identity snapshot: ActorId, authority class, profile name, profile content hash, exercised capability, and scope. It also carries exactly one validated subject reference: Bead, Assignment, Attempt, or responsibility scope. Subject validation proves that the referenced record exists where applicable, that its type is valid for the Signal variant, and that the sender is authorized for it. No Signal variant accepts a subject-free body.

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

Core refuses consequential actions that conflict with the effective Directive sequence. A Handoff submission made while an amend or pause Directive remains undischarged is an ordinary Submission refusal with a distinct reason; it records no Handoff and leaves the Attempt active. After an abort Directive, further mutating calls are refused except abort-consistent actions.

No `read_at` columns, per-Directive acknowledgement rows/state, client-asserted “seen head,” mutable inbox, delivery retry, or escalation-on-silence exists. Per-recipient and global unresolved sets are derived from immutable Signals lacking their typed linked responding actions. Lost-response and concurrent-call behavior uses normal idempotency, fencing, and causal protocol ordering; a call cannot leapfrog the response that surfaced a binding Directive.

Signal volume stays bounded by routing doctrine rather than caps: progress is queried; work-shaped blockers become dependency edges in `br`; decision-shaped asks become Requests; and everything else is transient Herdr chatter. Herdr may ring a content-free live doorbell after a Signal commits, but a Signal body never rides the prompt and notification never owns exposure, discharge, or resolution.

If ABACUS ever needs untyped subject-free messages, per-message read/ack state, or escalation-on-silence machinery, that is mail and requires its own ADR rather than another Signal variant.

### Evidence and handoff use cases

- Validate the shape and identity binding of evidence.
- Require a commit object, expected base, clean-tree proof, and policy-required command outcomes.
- Bind the Assignment to the bead-content hash used to authorize it and require Acceptance to recheck that hash.
- Require each verification command to record before/after workspace digests so test-induced mutations are visible.
- Require the handed-off commit's changed paths to conform to the Assignment's normalized edit scope.
- Distinguish submitted evidence from independently verified evidence.
- Decide accept/reject without consulting raw pane output.

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
13. Attempt retry is an explicit fenced decision-actor action; no transition or timer creates one automatically.
14. Submission refusal produces an auditable outcome even though it creates no Handoff or decision.
15. Every Signal is append-only, sender-fenced with full actor identity, typed, and bound to a validated workflow subject; no Signal type accepts a subject-free body.
16. Every fenced worker response mechanically surfaces the Attempt's current binding Directives as a Scribe protocol property, and current Directive policy gates consequential mutations regardless of delivery/read status.
17. Acceptance fails if the authoritative bead content no longer matches the Assignment's bound hash, the commit diff escapes edit scope, or verification changed the workspace without an allowed and accounted-for result.
18. Signal exposure and discharge are derived from immutable call ordering and linked responding actions, never from delivery metadata, opening, acknowledgement, or a timer.

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
- Acceptance decision/application-attempt/application-receipt ordering and crash-window tests;
- bead-hash, edit-scope, before/after-workspace-digest, and evidence-policy tests with fake commit verification;
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
- `bv` absence has no effect on eligibility or correctness.
- Provider and persistence types are absent from the public interface.
