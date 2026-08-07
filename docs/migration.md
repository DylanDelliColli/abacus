# Migration from SABLE

Status: revised build plan after ADR-0006
Last updated: 2026-08-07

## Objective

Extract SABLE's useful center—bead-led work, authored roles, local agents, and
exact-commit completion evidence—without carrying forward its Dolt server,
mail compensation stack, pane scraping, merge-seat machinery, or test-control
system.

ABACUS is a clean build beside SABLE. It does not migrate legacy runtime state
or operate through legacy `bd`/Dolt services.

## Premise correction

The build initially treated agent identity fraud and concurrent workflow
writers as the central risk. The operator corrected that premise after review:
SABLE's damaging failures came from machinery accretion and honest-but-
unreliable agents, not adversarial workers. ABACUS therefore starts with stock
`br`, visible typed records, and one shared store. Enforcement is added only
after a real failure proves it necessary.

This correction obsoletes the proposed second Ledger, Scribe daemon, socket and
relay, caller credentials/guards, capability/scope authorization stack, and
two-store application/receipt reconciliation. Existing source implementing
those ideas is transitional; it is deleted by replacement rather than ahead of
its consumers.

## Extraction matrix

| Legacy capability | ABACUS treatment |
|---|---|
| Beads, dependencies, priority, ready/closed work | Stock pinned `br`, one shared absolute `BEADS_DIR` |
| Assignment/execution/decision history | Minimum typed append-only facts attached to the work bead; exact set chosen by necessity review |
| Completion evidence | Keep and strengthen: wrapper-captured outcome bound to exact commit/tree |
| Handoff | Typed record attached to work, never another bead |
| Runtime sessions and live prompts | Herdr behind `abacus-runtime` |
| Graph advice | Optional `bv`, deterministic fallback |
| Dolt-backed `bd` and daemon | Do not port |
| Scribe/Ledger/socket/relay | Withdraw; no second durable store or ABACUS daemon |
| Durable mailbox, ack, delivery retry, escalation ladder | Do not port |
| tmux scraping and pane metadata as authority | Do not port; runtime observations stay advisory |
| Merge seat / automatic publication | Do not port; Publication remains separate |
| CI-efficiency and universal TDD machinery | Do not port |

## Target repository shape

```text
abacus-core       minimal provider-neutral domain and policy
abacus-work       stock-br normalization plus optional bv advice
abacus-runtime    Herdr normalization
abacus-cli        composition, host discovery, typed convenience commands
abacus-state      transitional pre-ADR-0006 source pending necessity decision
```

The target durable topology is one control checkout's `.beads` directory.
Every process and linked worktree receives its absolute path in `BEADS_DIR`.
Codex receives that exact directory as an additional writable root. Default
per-worktree `br` discovery is never live coordination.

## Completed evidence and source history

The build has useful artifacts even where the architecture was superseded:

- pinned `br`/`bv` and Herdr compatibility records and fixtures;
- pure domain rules for exact-commit Evidence/Handoff validation;
- work-provider normalization, namespace mapping, ready selection, and atomic
  claim evidence;
- generation-bearing Herdr handle work and a fake runtime contract;
- four hermetic journeys through the current production composition; and
- adversarial findings about stale in-memory SQLite caches, ambiguous provider
  effects, provenance, lifecycle conflation, and transport limits.

Those are evidence, not a reason to preserve every type that carried them.

## Revised staged implementation

### Stage 0 — Binding collapse (current)

- accept ADR-0006 and amend normative `CONTEXT.md` in the same landing;
- mark ADR-0001/0002 partially superseded, ADR-0003 fully superseded, and
  ADR-0005 withdrawn;
- mark old state/module contracts transitional so no new work targets them;
- prune only backlog items made unambiguously obsolete.

No source is added in this stage.

### Stage 1 — Proven-inert subtraction

After the normative landing and cross-review, remove only:

- `ValidatedProfileSet::authorize`;
- `AuthorizationTarget`;
- `ActionContext`;
- `AuthorizationRefusal`; and
- `StateError::ScopeUnauthorized`.

These are test-only or producerless. All broader authority, scope, audit,
fencing, and state types remain until their live consumers are replaced.

### Stage 2 — Necessity round

Before new source, the operator reviews every surviving concept against one of:

- a measured SABLE failure;
- one of the four hermetic journeys; or
- the minimum live-provider loop.

The round fixes the minimum append-only record vocabulary and decides whether
Assignment, Attempt, lease, numeric fencing, operation idempotency, Signal
taxonomy, audit index, profile activation, decision-owner metadata, and
runtime association remain. Existing implementation is not evidence by
itself.

For every proposed append-only encoding, the round first runs a disposable
pinned-`br` proof: append multiple ordered records, export JSONL, discard and
rebuild the database, then compare record bodies, IDs/order, and references.
Export presence without rebuild parity does not pass.

The non-negotiable floor is:

- append-only authorization/decision history;
- distinct claimed/launched/parked/dead/successor facts;
- exact-commit Evidence and typed Handoff;
- accepted completion distinct from Publication; and
- Handoff not represented as work.

### Stage 3 — Thin shared-`br` vertical facade

Only after Stage 2:

- inject and validate one absolute `BEADS_DIR` everywhere;
- use native atomic claim for initial ownership;
- implement only the selected typed append-only record shapes over stock `br`;
- retain provider normalization and deterministic advice fallback;
- replace the current Assignment/Acceptance composition; and
- delete two-store application attempts, receipts, supersession, pending
  projection, and their consumers in the same stack.

The four journeys are adapted rather than weakened. A step disappears only
because the underlying cross-store failure mode disappears.

### Stage 4 — Herdr adapter and CLI composition

- complete the pinned Herdr adapter behind `abacus-runtime`;
- compose ready → claim → launch → Evidence → Handoff → Acceptance/close;
- keep runtime output non-authoritative;
- expose concise human and versioned machine output; and
- install no hooks, global config, daemon, or hidden loop.

### Stage 5 — Live-provider vertical pilot

`ABACUS-2IS` is the preferred implementation-contact gate once the operator
lifts the no-new-code hold. It runs one disposable real loop with pinned stock
`br`, one Herdr-launched agent, and the shared-store topology. It must precede
new recovery or attention machinery.

The pilot proves at minimum:

- the primary and linked worktree resolve the same `BEADS_DIR`;
- one native claim wins;
- launch environment carries no state secret or second-store locator;
- exact-commit Evidence and typed Handoff survive model context loss;
- Acceptance does not imply Publication; and
- teardown leaves no session, temporary worktree, or global configuration.

### Stage 6 — Authored roles and optional policy

Seed project-owned orchestrator/worker cards only after the real loop passes.
Additional profiles, Signals, attention, retries, automation, or publication
policy require observed need. Parked ADR-0004 remains non-contractual until a
real stall meets its stated trigger.

## Change-locality rules

- C0 changes run owning-module tests.
- C1 seams add direct-consumer contracts.
- C2 breaking seams/new dependencies require an ADR.
- C3 core changes run the full hermetic workspace.
- Live providers stay out of default tests.
- Test-budget growth requires an ADR, never another selection service.

Replacement and deletion are reviewed as one behavior-preserving stack when
separating them would create a hole. Pure dead-code subtraction may land alone
only when production usage is proven absent.

## Data and provider policy

- Stock `br` is pinned and checksummed; no fork without a measured missing
  primitive, inability to express it as data, and upstream refusal.
- `issues.jsonl` is portable backup/interchange, not a live merge protocol.
- Native `br` audit events are local-only and cannot be canonical ABACUS
  history.
- No legacy SABLE/Dolt state is imported automatically.
- No secrets, credentials, provider transcripts, or terminal output are stored
  as workflow facts.
- Herdr and `bv` remain independently pinned behind their adapters.

## Stop conditions

Stop and return to the operator if implementation requires:

- a second durable store;
- a daemon, socket relay, hidden loop, or auto-start service;
- raw provider SQL or a `br` fork;
- generic CAS/multi-record transactions not evidenced by the first loop;
- a mailbox/ack/retry subsystem;
- treating runtime output as completion; or
- weakening exact-commit Evidence/Handoff binding.

Each is a new architecture decision, not an implementation detail.

## Rollback principle

The tracker choice remains reversible through portable JSONL. A provider fork
or new persistent service does not. Prefer the reversible stock configuration
until concrete use proves it inadequate.
