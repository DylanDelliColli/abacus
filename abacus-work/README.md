# `abacus-work` module contract

Status: crate skeleton landed (ABACUS-omw.1) — internal provider seam,
facade over the core work ports, hermetic fakes, and the reusable
contract suite. The `br`/`bv` process adapters (ABACUS-omw.2/.5) and the
remaining normalization beads are not yet implemented; everything below
that names a provider command still describes intended behavior.

## Purpose

`abacus-work` is the only ABACUS module allowed to speak to the work-graph providers. It implements a small normalized work interface over pinned `br` and an optional advice interface over pinned `bv`.

Its depth comes from hiding provider command sequences, schemas, sync behavior, version quirks, and failure interpretation. Callers reason about beads and outcomes, not subprocesses.

## Owns

- Normalized work snapshots and graph revisions required by core use cases
- `br` argv/process adapter
- `br` output/schema validation and error mapping
- `ABACUS-` identifier-prefix enforcement
- Ready/list/show/create/update/dependency/status/close/reopen operations
- Work-mutation serialization/reconciliation behavior
- Revisioned status observations and provider-side anomaly signals used to detect out-of-band mutations
- Explicit JSONL flush behavior required by the pinned provider
- `bv` robot-mode advisor adapter
- Advice validation, deadlines, graph-hash binding, and deterministic fallback signaling
- Provider capability detection and fixture compatibility records
- Work-provider-specific diagnostics surfaced through `abacus doctor`
- Namespaced capability descriptors for work-owned use cases

## Does not own

- Assignment, lease, evidence, or handoff state
- Actor profiles or authorization policy
- Worker processes, panes, or prompts
- Git commit verification, staging, commit, pull, or push
- The canonical SQLite/JSONL implementation inside `br`
- A copy of `br` or `bv` domain models in core
- Automatic prioritization policy that can override eligibility
- General subprocess helpers for unrelated modules

## Initial providers

### `br`

[`beads_rust`](https://github.com/Dicklesworthstone/beads_rust) is a local-first issue tracker using SQLite with JSONL export and explicit Git handoff. It deliberately avoids automatic commits, hooks, background daemons, and Dolt. That makes it a suitable work graph, not ABACUS's workflow-state service.

The initial adapter runs a pinned `br` executable. ABACUS does not initially fork or link against its internal Rust crates. Process isolation keeps the provider seam honest and allows exact executable/schema compatibility tests.

### `bv`

[`beads_viewer`](https://github.com/Dicklesworthstone/beads_viewer) supplies graph analysis through non-interactive robot output. It is an advisor only. Bare `bv` is never launched by an agent-facing operation because that opens an interactive TUI.

The pinned `br` and `bv` representations must be proven compatible in a disposable spike. Shared authorship/ecosystem is not a compatibility guarantee.

## Deep work interface

### Read behavior

- Return a normalized work item without exposing provider payloads.
- Return a stable content hash/revision over the bead fields that define authorized requirements.
- Return ready/blocked work with an explicit graph revision/content hash.
- Inspect dependency relationships with unambiguous edge direction.
- Distinguish provider unavailable, incompatible, corrupt, busy, and malformed-output failures.
- Compare observed status/revision with an expected authorized-operation receipt supplied by the caller and return a normalized out-of-band-mutation anomaly.

Read results may include bounded provider diagnostics for troubleshooting, but callers never parse them.

### Mutation behavior

- Create a bead using the configured `ABACUS-` prefix.
- Update only supported normalized fields.
- Add/remove dependencies using requirement-oriented arguments.
- Transition open/in-progress/closed state through validated operations.
- Return before/after graph revision and an audit-safe summary.
- Require the committed authorizing decision/operation identity for decision-gated status changes and return the normalized facts needed for an immutable application receipt.
- Require a bounded curated `close_reason` supplied by the authorizing Acceptance operation; for normal completion the stable reason code is `accepted_handoff`, with any provider string rendered by the adapter.
- Reconcile an ambiguous outcome through read-before-write inspection.

The interface accepts an authorized core operation context. In the initial policy, orchestrator profiles may receive work-mutation capabilities and workers do not. No manager name is hard-coded. Moving graph-closing responsibility from one manager profile to another is configuration-only.

`abacus-work` does not decide whether a Ledger decision exists and never reads the Ledger. It exposes revisioned provider facts and compares them with the expected operation context supplied through its interface; the core use case correlates work observations with Ledger decisions through separate ports. Correlation runs lazily during ordinary `ready`, `show`, acceptance-validation, and `doctor` reads—never in a watcher. A mismatch fails loud and requires explicit reconciliation, never silent adoption or reversal.

The adapter invokes argv directly with an explicit working directory, sanitized environment, deadline, output bound, and no shell.

### Advice behavior

- Analyze one graph revision and optional scope.
- Return ranked eligible bead IDs, scores/reasons, analyzed hash, and per-metric completeness.
- Return explicit unavailable, timeout, partial, incompatible, or malformed results.
- Never mutate work.

Core owns the deterministic fallback: eligible work sorted by explicit priority and stable ID. Advice that names an ineligible bead or stale graph revision is rejected.

## Canonical work checkout

The exact `br` behavior across Git worktrees is a required spike. Until that evidence exists, the architecture assumes one configured control checkout owns work-graph mutations while worker worktrees report durable execution facts through Scribe.

The adapter, not callers, resolves the provider working directory from explicit configuration. A later topology change remains internal if the normalized interface is unchanged.

## Provider contract

`.abacus/providers.lock.toml` will pin:

- executable version/release identity;
- binary checksum;
- expected JSON/schema/capability fingerprint;
- fixture-set version;
- required command/features.

Startup/doctor verifies the pin. An incompatible provider fails before mutation rather than attempting to parse human output.

Fixtures contain minimal sanitized outputs for every consumed operation and error class. They are owned by this module; no other module imports them.

## Idempotency and concurrency

`br` provides its own workspace locking, but ABACUS still needs operation-level semantics:

- every work mutation carries an ABACUS operation/idempotency identity;
- only the supported work interface may mutate;
- callers do not run several low-level provider commands as one assumed transaction;
- partial multi-step operations return a reconciliation record;
- before retrying an ambiguous mutation, inspect current normalized state;
- advice is tied to a graph revision and expires when the graph changes.

No attempt is made to create distributed real-time collaboration. The first scope is one local repository/control checkout.

## Dependency rule

`abacus-work` depends only on `abacus-core` within ABACUS. It cannot import state, runtime, or CLI modules. Workflow and authorization contexts are passed through core-owned values; work does not open the Ledger.

## Evolution and blast radius

| Change | Expected validation |
| --- | --- |
| Refactor command construction/parsing with same normalized outcomes | Work tests |
| Update pinned `br` or `bv` with same normalized interface | Work fixture/contract tests plus affected live compatibility lane |
| Add an existing mutation capability to a different manager profile | Profile/config tests; no work code |
| Add a new normalized work operation | Work tests plus direct core/use-case and CLI composition tests |
| Change normalized work values or semantics | ADR if breaking; direct consumers only |

A provider upgrade does not run state/runtime live tests. A work implementation change does not import their fixtures. The adapter interface, not a monolithic end-to-end suite, is the test surface.

## Test contract

Default tests use a fake argv process runner and checked-in fixtures. They cover:

- exact argv, working directory, environment allowlist, deadlines, and output bounds;
- schema validation and forward/unknown fields;
- normalized success/error mapping;
- `ABACUS-` prefix validation;
- dependency direction and ready-state behavior;
- ambiguous mutation reconciliation;
- curated close-reason rendering and refusal of arbitrary/unbounded provider text;
- stable bead-content hashing and changed-content detection across assignment/acceptance reads;
- out-of-band status/revision anomaly detection from supplied expected-operation context;
- graph revision changes and stale advice rejection;
- `bv` unavailable/timeout/partial/malformed paths and deterministic fallback;
- capability authorization without named-manager assumptions.

### The portable contract suite

`contract::run_work_graph_suite` is generic over any `WorkProvider` and
names no concrete provider. An adapter proves conformance by calling it
with a factory that materializes a `Scenario`, rather than restating the
expectations — `FakeWorkProvider::from_scenario` is the reference
implementation of that factory.

It enforces, for every adapter: the expected-revision precondition; the
idempotent already-present effect; single-re-inspection reconciliation of
an ambiguous outcome; loud failure when an ambiguous mutation did not
land; terminality of a closed bead in **both** directions (never
re-closed under a different reason, never reopened); and the
`set_status` `Err` contract — that an error left the bead and its
revision untouched.

That last one is the reason the suite exists rather than a checklist.
`Err` asserts the mutation *definitively did not take effect*, and the
facade skips reconciliation on that basis; an adapter that reports a
landed mutation as `Err` would make a retry look safe and double-apply.
Conformance is checked, not trusted.

No default test invokes installed `br`/`bv`, reads live `.beads`, touches user config, runs Git, or launches a TUI.

The explicit live compatibility lane uses a temporary repository and redirected provider state. It runs when the provider lock changes and on scheduled/manual validation.

Warm hermetic target: under fifteen seconds on the baseline development machine.

## Acceptance criteria

- Every work mutation is possible only through the normalized interface.
- Raw `br`/`bv` JSON and error strings do not escape the module.
- `bv` can be removed and all correct workflows still function.
- Advice is rejected when its graph hash is stale.
- Moving close authority between manager profiles requires no adapter code.
- A provider-version change with an unchanged normalized interface affects only work tests and its compatibility lane.
- No operation performs implicit Git network or hook actions.
