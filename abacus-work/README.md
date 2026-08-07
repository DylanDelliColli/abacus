# `abacus-work` module contract

Status: implemented provider normalization under transition to ADR-0006

## Purpose

`abacus-work` is the normalized adapter over pinned stock `br` and optional
`bv`. ADR-0006 makes `br` the one durable work **and workflow-fact** store, so
this module becomes the provider seam for both current bead fields and the
minimum append-only record shapes selected by the necessity round.

Agents may also invoke stock `br` directly. This facade is a typed convenience
and composition seam, not an exclusive writer, authentication layer, or
security boundary.

## Current implemented surface

The crate already owns:

- normalized work snapshots and provider revisions;
- `ABACUS-`/provider-ID mapping;
- provider-shaped fakes and a portable work contract suite;
- ready/list/show/update/dependency/status/close behavior;
- scope-label normalization into `ScopeMap`;
- ambiguous provider-outcome inspection;
- a `br` subprocess adapter foundation and captured compatibility fixtures;
- optional `bv` advice normalization and deterministic fallback; and
- content hashing over scope-relevant provider facts.

Two-store expected-operation receipts, out-of-band comparisons against a
second Ledger, and application provenance are transitional. They remain only
until their production consumers are replaced in the shared-`br` facade stack.

## Target ownership

- Inject the same validated absolute `BEADS_DIR` on every provider call.
- Normalize stock `br` machine output, IDs, statuses, labels, revisions, and
  errors without exposing provider structs to core.
- Preserve native atomic claim and report its one-winner/loser outcome.
- Read and write only the append-only typed workflow record kinds approved by
  ADR-0006's necessity round.
- Keep current mutable work fields distinct from append-only history.
- Preserve exact-commit Evidence/Handoff inputs supplied by composition.
- Apply bounded process deadlines and return ambiguity honestly when the
  provider may have committed before output was lost.
- Validate provider pins and own minimal fixtures for every consumed shape.
- Supply optional `bv` advice without making it authoritative.

The module does not own:

- orchestration policy or the decision to assign/accept/retry;
- Herdr sessions, panes, prompts, or runtime handles;
- Git verification, staging, commits, Publication, or merge;
- a second database, provider-internal SQL, or SQLite migration;
- capability/scope authorization or actor authentication;
- hidden retry/reconciliation loops; or
- a fork of `br` or `bv`.

## Store topology

One control checkout's `.beads` directory is canonical for live operation.
Composition passes its absolute path as `BEADS_DIR`; the adapter does not walk
up from its working directory. Linked worktrees never open their own local
`.beads` database for coordination.

Codex receives the canonical directory as an additional writable launch root.
This is intentional under the trusted-local direct-provider model. `BEADS_DIR`
missing, relative, inconsistent, or unwritable fails before mutation; there is
no fallback to cwd discovery, `/tmp`, a relay, or Git merge.

JSONL is the provider's portable export/rebuild artifact, not a live
multi-worktree merge protocol. Native `br` audit events are local-only and are
not canonical ABACUS workflow history.

## `br` behavior

The adapter invokes argv directly with an explicit working directory,
sanitized/injected environment, deadline, bounded output, and no shell. It
uses the documented machine interface of the pinned binary.

Required behavior includes:

- stable ready/list/show reads bracketed by provider revision where needed;
- namespace and schema validation before values enter core;
- atomic initial claim with one winner under contention;
- typed handling of provider busy, unavailable, incompatible, corrupt,
  malformed, missing, conflict, and ambiguous outcomes;
- curated completion reasons that cannot conflate Acceptance with Publication;
- explicit JSONL diagnostics/recovery rather than silent rebuild guesses; and
- no assumption of generic compare-and-set or multi-issue transactions.

Stock `br` comments are the available transactional append primitive and are
exported in `issues.jsonl`. The exact ABACUS record schema, ordering rule, and
whether an operation identity is necessary remain blocked on the necessity
round; this contract does not invent them early.

## `bv` behavior

- Analyze one graph revision and optional selector.
- Return ranked eligible IDs, reasons, analyzed hash, and completeness.
- Distinguish unavailable, timeout, partial, incompatible, and malformed.
- Never mutate work.

Core owns fallback: eligible work ordered by explicit priority then stable ID.
Advice naming ineligible work or a stale graph is ignored/refused.

## Dependency rule

`abacus-work` depends only on `abacus-core` within ABACUS. It never imports
runtime, CLI, or transitional state. Cross-provider choreography belongs in
composition over ports rather than a lateral dependency.

## Transition discipline

- Do not remove the current Acceptance/application consumer path until its
  shared-`br` replacement exists in the same C3 stack.
- Do not grow the old receipt/supersession model; it is obsolete by store
  unification.
- Do not add a record codec/reducer before the operator necessity round.
- Do not fork or reach into provider SQL to obtain a missing primitive.
- Keep `ScopeMap` normalization while `abacus-work` consumes it; removal of
  write-time authorization does not itself remove provider label semantics.

## Test contract

Default tests use fake process runners and checked-in fixtures. They cover:

- exact argv, cwd, `BEADS_DIR`, environment allowlist, deadline, and output
  bounds;
- schema validation and unknown fields;
- `ABACUS-` ID mapping;
- dependency direction, ready eligibility, parked facts, and atomic claim;
- normalized success/error/ambiguous outcomes;
- status terminality and curated completion reason;
- content hashing and scope-label normalization;
- provider revision drift and stale advice;
- `bv` unavailable/timeout/partial/malformed fallback; and
- no per-worktree discovery fallback.

`contract::run_work_graph_suite` remains the portable adapter contract. The
four hermetic journeys remain direct-consumer continuity evidence while the
shared-store replacement deletes the second-store saga.

Live `br`/`bv` tests run only for pin changes, explicit compatibility work, or
the operator-authorized vertical pilot. Default tests use no live provider,
network, or user home.

Before any append-only workflow encoding becomes a contract, its explicit live
compatibility lane must prove exact record/order/reference preservation across
JSONL export and database rebuild. Export-only evidence is insufficient.

## Acceptance criteria

- Every invocation addresses the one configured shared store.
- A two-contender claim has one winner.
- Provider types and raw output stop at this module.
- Missing/wrong `BEADS_DIR` fails before mutation.
- Exact-commit Handoff policy is not weakened by the store change.
- No active code requires a second Ledger, application receipt, state RPC, or
  direct provider SQL after the replacement stack completes.
