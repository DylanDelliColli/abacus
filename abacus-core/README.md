# `abacus-core` module contract

Status: implemented domain under ADR-0006 necessity review

## Purpose

`abacus-core` owns the smallest pure, provider-neutral vocabulary and policy
shared by at least two modules. It turns normalized bead/runtime/Git facts into
decisions and typed outcomes without performing I/O.

The crate currently contains the much richer separate-Ledger model built
before ADR-0006. Existing source is transition evidence, not a presumption that
every type survives. No new state/authority machinery may be added before the
operator-required necessity round.

## Binding ownership

Core owns:

- validated product identifiers and the two authority classes, orchestrator
  and worker;
- provider-neutral current-work and ready-selection values;
- `ScopeMap` as a normalized work-label value while direct consumers need it,
  independent of runtime authorization;
- exact-commit Evidence values and honest normalized outcomes;
- Handoff preconditions, Acceptance/Rejection policy, and the distinction
  between completion and Publication;
- pure lifecycle/agent-state composition where the necessity round retains the
  inputs;
- UNKNOWN for contradictory or stale observations;
- provider-neutral ports owned by surviving use cases; and
- typed refusals/outcomes that callers genuinely need to branch on.

Core does not own:

- `br`, `bv`, Herdr, Git, SQLite, JSONL, sockets, processes, filesystem paths,
  clocks, or environment discovery;
- a second persistence model, Scribe protocol, credentials, or provider
  request framing;
- provider schema/argv parsing;
- role-card syntax or configuration loading;
- malicious same-user containment;
- hidden retries, timers, or reconciliation loops; or
- speculative types for a possible future workflow.

## Surviving policy floor

### Bead-led work

Every acted-on unit is a bead. Ready selection validates provider eligibility
and revision facts. Native `br` atomic claim is the initial claim arbiter; core
does not invent a second ownership transaction.

An Assignment remains the minimum durable link from bead to worker and stable
decision owner unless the necessity round proves an even smaller equivalent.
Execution facts must distinguish claimed, launched, parked, dead/stalled, and
successor. Retrying never rewrites a predecessor.

### Evidence and Handoff

Evidence is wrapper-captured, not a caller-supplied pass boolean. It binds:

- exact command/verification identity;
- observed `pass`, `assert-fail`, or `execution-error` outcome;
- exact commit/tree and declared-base facts;
- timestamps and bounded diagnostic identity; and
- optional policy-approved overlay paths/digests.

`execution-error` never satisfies red. A policy-required red-green pair uses
assertion-level red against the declared-base implementation and green at the
Handoff commit. Overlay files must match the Handoff commit and remain inside
the policy verification set.

A Handoff is a typed record attached to a bead/Attempt, never another bead. It
requires a clean commit and matching passing Evidence. Acceptance and Rejection
are append-only decisions. Acceptance closes/completes work in the same stock
`br` provider domain and never implies Publication.

### Runtime composition

Herdr observations are advisory. Pure composition may combine durable workflow
facts, runtime liveness, and worker semantic reports, but stale or
contradictory inputs yield UNKNOWN. Runtime output alone cannot assign,
complete, accept, publish, or reclaim work.

## Reopened concepts

ADR-0006 requires each of these to re-earn its cost before new implementation:

- time leases and numeric `FencingToken`;
- operation-idempotency result storage;
- Directive/Report/Request as a fixed Signal taxonomy;
- audit event/index richness;
- `AuthoritySnapshot`, capability descriptors, grants, check classes, and
  scope authorization;
- profile activation, occupancy, and grant drift;
- actor-to-actor transfer machinery;
- runtime-association persistence/CAS; and
- the breadth and split of current workflow-state ports.

Their current use in source determines deletion sequencing, not target status.
They move only with the consumer replacement selected after the necessity
round.

## Completed inert subtraction

After the ADR-0006/`CONTEXT.md` landing, one C3 subtraction removed the
following test-only or producerless cluster:

- `ValidatedProfileSet::authorize`;
- `AuthorizationTarget`;
- `ActionContext`;
- `AuthorizationRefusal`; and
- `StateError::ScopeUnauthorized`.

That completed subtraction does not authorize adjacent cuts. `ScopeExpr`,
`ScopeMap`, grants, authority snapshots, fencing, and audit still have live
consumers and move only with their replacement.

## Ports and composition

Ports are use-case-shaped and provider-neutral. After the necessity round,
core may define only the views required for:

- shared-`br` ready/read/claim/current-field and approved append-record
  operations;
- Git/verification facts needed by Handoff policy; and
- Herdr launch/observe/prompt/stop operations.

No generic provider framework, SQL/table CRUD, state service, or dynamic
command surface belongs in core. The existing rich state ports remain only
until replacement and deletion land together so the journeys never lose their
production choreography.

## Dependency rule

`abacus-core` depends on no other ABACUS crate and no I/O/provider crate. Pure
utility dependencies require an ADR only when they materially change the
shared contract or build surface.

## Change and test contract

Every core change is C3 and runs the full hermetic workspace because every
adapter depends on it. That cost is acceptable only while core remains small.

Default tests cover surviving pure rules:

- identifier and two-class validation;
- ready-selection and provider-revision consistency;
- normalized scope-label value behavior while consumed;
- exact-commit Evidence and Handoff preconditions;
- red-green truthfulness and overlay binding;
- Acceptance versus Publication;
- lifecycle distinctions retained by the necessity round; and
- UNKNOWN composition under stale/contradictory runtime facts.

Transition tests for superseded state types remain until their consumers are
replaced. They are deleted with the types rather than weakened to keep obsolete
machinery green.

No core test uses files, processes, environment, network, wall-clock time,
SQLite, `br`, `bv`, Herdr, Claude, or Codex.

## Acceptance criteria

- Provider types and I/O cannot enter core.
- Exact-commit Evidence/Handoff policy remains enforceable as a pure rule.
- Runtime observations alone cannot complete work.
- No new type is admitted solely because the old Ledger used it.
- The five-item inert authorization cluster is absent without breaking a
  production consumer.
- The post-necessity public surface contains only concepts demonstrated by the
  first runnable loop or measured predecessor failure.
