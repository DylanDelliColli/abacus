# `abacus-state` module contract

Status: migration foundation landed (`ABACUS-9NH.7`, commit `6e91149`) —
transactional SQLite schema v1/v2 under `src/migrations.rs` with WAL and
busy-timeout configuration, exported from `src/lib.rs`. Scribe itself
(process, socket transport, client/server, lease and Ledger operations)
is not yet implemented; everything below describing those remains a
design contract.

## Purpose

`abacus-state` provides the durable local workflow-state module and the **Scribe** service (`abacus-scribe`). Scribe is the only writer to the Ledger. It concentrates transactions, idempotency, fencing, and recovery behind a versioned local interface so agents never coordinate by editing ad hoc files or writing directly under `.git`.

Scribe is not a manager, mailbox, scheduler, watcher, or policy engine. It records typed workflow facts; Herdr owns all live prompting and doorbell delivery.

## Owns

- Scribe process lifecycle and single-instance write ownership
- Repository-instance identity
- SQLite schema, migrations, transactions, WAL, and recovery
- Versioned local client/server protocol
- Actor registrations and profile/authority snapshots, including role-card content hashes
- Assignments and immutable execution attempts
- Assignment-bound bead content hashes/revisions and normalized edit scopes
- Leases and fencing tokens
- Canonical sanitized Envelope snapshots
- Immutable typed Signals: attempt-scoped Directives/Reports and actor-to-actor Requests
- Evidence, Handoff submissions, decisions, work-status application attempts, and application receipts
- Runtime-handle associations and audit events for observations explicitly reported by an actor
- Idempotency results and structured audit events
- Read/query surfaces needed by managers and watchdog profiles
- Namespaced capability descriptors for state-owned use cases

## Does not own

- Bead descriptions, dependencies, priority, or canonical work status
- Work ranking or `bv` scores
- Agent process/pane lifecycle
- Live runtime status maintenance or polling
- Git commit contents or provider execution
- Authorization policy definitions; those are core rules applied by the state operations
- Named profile configuration files or authored role cards
- Push, PR, merge, deployment, or CI state
- Global user configuration

## Paths

Durable state:

```text
<git-common-dir>/abacus/state.sqlite3
<git-common-dir>/abacus/repo-id
```

Local transport:

```text
$XDG_RUNTIME_DIR/abacus/<repo-id>.sock
```

`repo-id` is generated locally and stored beside the database. It survives a directory rename and differs across independent clones.

Only Scribe mutates the database. The database and socket directory are user-only. No fallback silently writes to a user home or worktree when these paths are unavailable.

**Credential authentication is this module's own:** Scribe verifies a presented bearer against the launch subject's active bound provisioning — the Attempt's for a worker, the actor activation's for a spawned orchestrator/watchdog — using a **vetted constant-time comparison implemented here** (core holds no cryptographic primitive, I15) and returns only a typed outcome — the stored digest never leaves Scribe. Wrong-token/right-ID must be refused before any provider mutation.

**Agent transport (ADR-0003, Proposed rev 5):** one versioned Scribe protocol on two carriages selected by injected configuration — direct UDS where the sandbox permits, or a per-call operator-owned `scribe-rpc` relay (exact two-token argv; one bounded newline-terminated typed-JSON request line on stdin; private 4-byte framing performed by the host client; one structured stdout line; fixed deadline; strictly one request per process and connection, batching forbidden). Actors are credentialed, never asserted: no enrolment verb exists anywhere in the agent-facing protocol; initial enrolment happens on a one-shot pre-listen operator channel, worker credential minting is an authenticated effect of `AssignmentOpening`, bearers are CSPRNG ≥128-bit with digest-only storage and constant-time comparison, and plaintext rides only an ephemeral launch secret — never the persisted Envelope. Host-approval denial is an agent-boundary fact; this client observes only connect failure (`Unavailable`) and protocol errors.

## Deep interface

The public client exposes workflow outcomes rather than database-shaped CRUD.

### Repository and actors

- initialize/inspect repository workflow state;
- five closed activation cases and nothing else — **four are `ActivationCase` variants** (`OperatorBootstrap`, `ActorAuthorizedRotation`, `OperatorRecovery`, `OperatorOrchestratorEnrolment`); the fifth, first-worker registration, is deliberately **not** a variant because it is an authenticated effect of a transaction rather than a request in its own right, so the enum's cardinality is four by design and not drift. The cases: operator-channel-only bootstrap of the initial/orchestrator actor; actor-authorized rotation for an already-registered same ActorId and class; first-worker registration as an atomic effect of `AssignmentOpening`; operator-channel recovery/root rotation for an already-registered orchestrator (never creates an actor); and operator-channel enrolment of an additional orchestrator actor into another validated profile (unknown actors only, orchestrator class only, occupancy still enforced, bootstrap sentinel untouched) — all operator-channel cases are absent from the agent protocol. No standalone or general enrolment verb exists on the public client;
- authenticated activation/resume for an already-provisioned actor;
- atomic worker credential **binding** solely as an effect of `AssignmentOpening` or retry `AttemptOpening`: the caller passes opaque id+digest (`CredentialProvisioning`), Scribe persists digest-only and returns idempotent `StateApplied`, plaintext never crosses in either direction;
- audit profile activation/change before it authorizes new actions;
- inspect current actor/runtime associations;
- record explicit authority transfer.

### Assignments and attempts

- atomically create an Assignment/initial Attempt with its authorizing bead content hash/revision, normalized edit scope, decision, and work-status operation identity;
- inspect current and historical attempts;
- transition an attempt using core validation;
- enforce the core-validated optional per-Assignment Attempt cap on explicit retry;
- persist/read the canonical sanitized Envelope snapshot keyed by the closed launch subject (worker Attempt or actor activation);
- append an authorized Directive, Report, or Request with a validated bead/Assignment/Attempt/scope subject and full fenced-sender snapshot;
- query immutable Signals by subject, sender, recipient, and causal order;
- query per-actor and global derived unresolved sets: Signals lacking the typed responding action that resolves or discharges them;
- return the active Attempt's current binding Directives in every fenced worker response;
- persist/read the canonical Envelope and bind/unbind an opaque runtime handle, both keyed by the closed launch subject (worker Attempt or actor activation) so spawned orchestrator/watchdog profiles are first-class;
- reconcile an uncertain runtime association.

### Leases and fencing

- acquire, renew, release, expire, and supersede a lease;
- issue monotonically increasing fencing tokens;
- reject mutations carrying a stale token;
- expose expiry/ownership facts to authorized observers.

### Evidence and decisions

- append ordinary structured Evidence records with the normalized verification command set, honest raw command/exit details, the closed normalized outcome (`pass`, `assert-fail`, or `execution-error`), exact commit binding, and artifact references;
- persist verification-command before/after workspace digests and final changed-path evidence;
- persist optional red-overlay metadata on that same Evidence value: declared-base binding, exact overlaid path set, and per-file content digests;
- query Evidence by Assignment/Attempt, verification set, commit, and overlay metadata so core can validate a required red-green pair without a pair row or new record class;
- audit a Submission refusal without creating a Handoff or ending the active Attempt;
- submit an immutable Handoff;
- record an authorized accept/reject/revoke/cancel decision;
- atomically record an Acceptance decision/operation identity and terminal `accepted` transition;
- record immutable application attempts/receipts after the caller applies the `br` mutation;
- derive decisions lacking a successful application receipt without maintaining a queue;
- query the complete audit lineage.

### Observation and watchdog access

- query filtered audit events, including runtime observations explicitly reported by an actor;
- never grant graph mutation or handoff authority merely because an actor can observe.

A watchdog profile without workflow-mutation authority should require no schema change. New automated recovery behavior adds a focused core use case and a state operation only if existing transitions cannot express it.

Watchdogs are ordinary Herdr-managed agent profiles, never additional daemon processes. Shared observation scopes may overlap; exclusive decision/mutation scopes must already have passed configuration validation, and Scribe still enforces exact-actor authorization and serialization.

## Protocol rules

- Every agent request carries protocol version, repository identity, ActorId **plus its bearer credential**, request ID, and idempotency key where mutating. Scribe authenticates the credential digest and its (actor, class, profile-hash, activation-generation) binding **before** `authorize` runs; an invalid, binding-mismatched, or revoked credential is a distinct refusal, and any enrolment-shaped request on the agent protocol is unknown/forbidden and audited.
- Fenced operations carry assignment, attempt, and current fencing token.
- Every fenced worker response mechanically surfaces the Attempt's current binding Directives. This is a protocol property of Scribe responses, never worker discipline; the response envelope contains the field even when the set is empty.
- Scribe commits immutable per-Attempt call/response ordering and applies core's Directive gate before mutation. Exposure and discharge are derived from that ordering and responding workflow actions: a Directive committed before the worker's latest fenced call was surfaced in that call's response.
- The client protocol is causally ordered and idempotent so concurrent or lost-response retries cannot leapfrog a response that first surfaced a binding Directive.
- Signal appends carry a closed type/kind, validated mandatory subject, full sender identity/profile/capability/scope snapshot, and the typed link required when the record responds to another Signal.
- No `read_at` columns and no per-Directive acknowledgement state exist in the schema or public interface. Scribe never accepts a client-asserted read marker or Directive head.
- Handoff is refused with a distinct ordinary Submission-refusal reason while an amend or pause Directive remains undischarged. After abort, Scribe refuses every further mutating call for the Attempt except an abort-consistent action.
- Responses are typed success or normalized error; clients do not parse Scribe log text.
- Unknown fields are handled according to an explicit version policy.
- Incompatible major versions fail before mutation.
- Envelope and evidence payload limits are explicit and bounded.
- Client disconnect does not imply transaction failure or success; retry returns the stored idempotent result.

ADR-0003 settles the wire: a public bounded typed-JSON **facade command envelope**, distinct from the **internal versioned Scribe request schema** for credential-creating calls, with private 4-byte length framing performed by the composing transport layer (the facade process directly, or the fixed-function `scribe-rpc` composer on the relay carriage), strictly one request per process and connection. Framing and internal-schema details remain private to this module and its client; lost/ambiguous exchanges surface as typed Ambiguous with replayable provisioning, never silent retry.

## Persistence model

The conceptual schema contains:

- repository metadata and migrations;
- actors and sessions;
- immutable profile/capability snapshots;
- assignments and attempts;
- Assignment-bound bead hashes/revisions and normalized edit scopes;
- leases/fencing sequence;
- immutable sanitized Envelope snapshots;
- immutable typed Signals: attempt-scoped Directives/Reports and actor-to-actor Requests;
- immutable per-Attempt fenced call/response ordering used to derive Directive exposure and discharge;
- runtime-handle associations and explicitly reported observation audit events;
- evidence items and artifact digests;
- verification before/after workspace digests and changed-path evidence;
- normalized verification outcomes and optional red-overlay path/digest metadata on ordinary Evidence;
- Submission-refusal audit events, Handoff submissions, decisions, application attempts, and application receipts;
- idempotency results;
- append-only audit events.

This is not permission to expose each table through the client. Tables are implementation; domain outcomes are interface.

Red-green pairing does not add a table, record kind, mutable pair status, coverage field, or threshold. The existing immutable Evidence value is extended with the closed normalized outcome and optional overlay metadata: the declared base commit, exact overlaid path set, and per-file content digests. Core derives red only from `assert-fail` produced against the declared-base implementation, validates that overlay paths are policy-scoped and their digests match the same files in the Handoff commit, and derives green from `pass` for the same verification set at that commit. `execution-error` remains an honest non-red outcome. “Red” and “green” are query/policy roles, not persistence classes.

Canonical `br` fields are not copied into mutable state. An audit record may store a provider-revision-tagged bead snapshot used for a decision.

## Consistency and recovery

- Mutations and their audit/idempotency records commit in one transaction.
- WAL and busy-timeout behavior are configured centrally.
- Scribe alone owns writes; clients never compete for SQLite locks.
- Lease decisions use the Scribe clock.
- Restart preserves repo identity, attempts, fencing, and idempotency.
- Restart re-derives per-actor/global unresolved Signals and current binding Directives from immutable records and call ordering; it rebuilds no inbox, acknowledgement state, notifier, or retry queue.
- When an authorized caller explicitly inspects and reports a missing runtime handle, Scribe records an `unknown` observation audit event; Scribe never polls or refreshes runtime state itself.
- Migrations are ordered, transactional where SQLite permits, and backed up before destructive transforms.
- A failed migration leaves the prior database readable and produces a recovery diagnostic.

## Dependency rule

`abacus-state` depends only on `abacus-core` within ABACUS. It cannot depend on work, runtime, or CLI modules. It stores their opaque references without importing provider types.

## Evolution and blast radius

| Change | Expected validation |
| --- | --- |
| Query/index/internal schema optimization with unchanged interface | State tests |
| Wire encoding/internal client change with same Rust interface | State protocol tests |
| Add an existing-capability profile/watchdog | Configuration and state authorization/query tests; no migration if snapshots already suffice |
| Add a new state outcome | State plus direct core/use-case and CLI composition tests |
| Break the public state interface | ADR plus direct consumers |

Database schema changes do not automatically require work or runtime tests. Only a changed public outcome creates downstream validation.

## Test contract

Default tests use temporary Git common directories, sockets, and SQLite files. They cover:

- credential authentication: no agent-protocol enrolment request exists; invalid, binding-mismatched, and revoked credentials refuse distinctly; digest-only persistence; worker binding atomic with `AssignmentOpening` and retry `AttemptOpening` (id and digest conflicts both refused); expiry/revocation at attempt end and deactivation;

- initialization and single-Scribe behavior;
- migrations and rollback/backup paths;
- concurrent client requests and idempotent retry;
- disconnect-after-commit ambiguity;
- lease renewal, expiry, supersession, and stale fencing;
- Envelope snapshot immutability, exact fencing identity, and provider-secret exclusion;
- Signal append/idempotency, subject/sender fencing, closed-kind validation, per-actor/global unresolved derivation, and schema/public-interface proof that no read/ack state exists;
- Directive exact-decision-actor authorization, binding-from-commit, mechanical response surfacing on every fenced response, immutable call ordering under concurrent/lost responses, and restart recovery;
- amend/pause Handoff refusal and abort-consistent mutation gating;
- structured Reports and linked Directive/decision resolution integrity;
- actor-to-actor Request authorization and linked fenced-decision resolution;
- red-green candidate derivation from ordinary Evidence, including matching verification sets, wrong-commit red, passing “red,” green-only submission, `execution-error` rejected as red, overlay-digest mismatch rejected, digest match accepted, and an overlay path outside the policy verification set rejected;
- Acceptance decision/application-attempt/application-receipt recovery across every interruption point;
- explicit fenced Attempt retry, optional per-Assignment attempt caps, and audited refusal/attempt churn;
- profile snapshots and decision-authority transfer;
- watchdog queries and read-only authorization;
- restart/recovery and corrupt/incompatible state;
- protocol version and malformed-payload handling.

Tests use fake clocks and do not sleep. They do not launch `br`, `bv`, Herdr, GitHub, Claude, or Codex and never read live user state.

Warm hermetic target: under fifteen seconds on the baseline development machine.

## Acceptance criteria

- A client can retry every mutating operation without duplication.
- A replaced worker cannot write with its prior fencing token.
- No fenced worker response omits a current binding Directive, and no incompatible consequential mutation can bypass an unread Directive.
- Two managers cannot both decide the same handoff.
- Signal unresolved queries derive only from immutable records and typed responding actions; no `read_at` column, per-Directive acknowledgement state, or mutable inbox exists anywhere in the schema or public interface.
- A required red-green pair, including overlay path/digest validation and the closed outcome taxonomy, can be evaluated from ordinary Evidence records after restart; no red/green/pair record class or mutable satisfaction flag exists.
- A watchdog without workflow-mutation authority can inspect audit and explicitly reported runtime observations after Scribe restarts.
- Moving an existing capability between profiles requires no schema migration.
- All durable transitions have one transactionally linked audit event.
- Work and runtime provider types are absent from schema and public interface.
