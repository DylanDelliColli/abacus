# ABACUS architecture

Status: target contract after ADR-0006; implementation transition in progress
Last updated: 2026-08-07

## Purpose

ABACUS turns a dependency-aware bead graph into locally executed,
evidence-backed agent work. It coordinates one local repository, two authority
classes, and a small number of explicit providers. It is not a general agent
platform or a security boundary between same-user processes.

`CONTEXT.md` is normative. ADR-0006 fixes the current store and trust model.
The existing `abacus-state` source implements the superseded two-store design;
it is transitional and must not be extended while the operator-required
necessity round is open.

## Architectural forces

- The product must run before it acquires speculative recovery machinery.
- Honest agents still crash, hang, act on stale facts, and make confident
  mistakes; durable shapes must prevent the four measured SABLE conflations.
- Provider details and drift stay at adapters.
- Runtime observations never prove semantic completion.
- Exact-commit Evidence and Handoff validation are the completion floor.
- A local change should incur tests proportional to the seam it crosses.
- Linked worktrees need one live work store; Git/JSONL merge is backup and
  portability, not arbitration.
- No ABACUS-owned daemon or hidden loop is justified in v1.

## System context

```text
                                  optional advice
                              +------------------+
                              |       bv         |
                              +--------^---------+
                                       |
+----------+     +-------------------+  |  +----------------------------+
| operator |---->| abacus / stock br |--+->| one shared stock br store  |
+----------+     | composition       |     | SQLite + exported JSONL     |
                 +---------+---------+     +----------------------------+
                           |
                           | normalized runtime operations
                           v
                    +-------------+
                    |    Herdr    |
                    +------+------+
                           |
                           v
                     Claude / Codex
```

There is no Scribe, state socket, relay, RPC, or second database. Agents may
invoke stock `br` directly. Typed ABACUS commands are convenience and
composition, not an exclusive writer or authentication boundary.

## Compile-time dependency direction

The intended stable direction remains acyclic:

```text
abacus-core
   ^       ^
   |       |
 work   runtime
   ^       ^
    \     /
   abacus-cli
```

`abacus-state` remains in the workspace only as transitional source from the
superseded design. The necessity round decides whether any narrow record codec
or reducer survives and where it belongs; no new dependency is inferred from
the old crate.

## Composition and host discovery

The composition root resolves the host once and injects module-specific
values. Inner modules do not inspect the current directory, user homes, global
agent configuration, or ambient provider paths.

### Shared `br` selection

Every ABACUS process and agent launch receives the same absolute `BEADS_DIR`,
pointing at one control checkout's `.beads` directory. The value is required,
validated as absolute, and never replaced by walk-up discovery. A missing,
unwritable, or inconsistent value fails before mutation.

Codex receives that exact `.beads` path as an additional writable root. The
grant is not a secret boundary: v1 intentionally lets local agents use the
stock tracker. Claude and Codex use the same store topology even though their
sandbox mechanisms differ.

Configuration precedence remains:

1. safe compiled defaults;
2. repository configuration;
3. injected `ABACUS_*` / provider environment;
4. explicit operator flags.

No layer may silently replace the configured shared store.

## State ownership

### Shared work/workflow store

Stock `br` is authoritative for:

- bead content, priority, labels, dependencies, and current work status;
- native atomic claim;
- append-only per-bead workflow facts selected by the necessity round; and
- the provider's SQLite state plus portable JSONL export.

ABACUS owns validation and meaning of its typed workflow records, not the
database schema. Native mutable issue fields remain current state. Historical
authorization/decision facts, execution lifecycle, Evidence, and Handoffs are
append-only and exported. Native `br` audit events are local-only diagnostics,
not canonical ABACUS history.

The store is trusted-local. Raw `br` can bypass an ABACUS convention. The
system surfaces inconsistent visible facts when it sees them; it does not
compare them to another authority or claim malicious mutation was prevented.

### Runtime state

Herdr owns processes, panes, sessions, terminal buffers, live prompt delivery,
and provider status. `abacus-runtime` normalizes those observations. Opaque
handles include provider generation so a reused pane/session is not mistaken
for its predecessor.

Runtime state remains non-authoritative. A pane saying `done`, an exited
process, or a screen-derived status can never accept a Handoff or close a bead.
Any durable runtime association that survives the necessity round is attached
to the relevant bead/Attempt in the shared store.

### Source state

Git owns commits and worktrees. Evidence records exact commit/tree identity;
Publication remains a separate operator/workflow action after Acceptance.

## Surviving domain floor

The necessity round may delete concepts, but it cannot cut below these
evidence-backed distinctions:

- one bead-led Assignment naming a worker and stable decision owner;
- distinct claimed, launched, parked, dead/stalled, and successor execution
  facts;
- append-only authorization and decision history;
- wrapper-captured Evidence bound to an exact artifact;
- a typed Handoff attached to work, never represented as another bead;
- accepted completion distinct from Publication; and
- UNKNOWN for contradictory or stale runtime composition.

The current rich Assignment/Attempt/Lease/Signal/Audit model is not the floor.
Lease, numeric fencing, operation idempotency, closed Signal taxonomy, profile
activation, capability/scope authorization, audit indexing, and runtime-handle
CAS must each re-earn their cost before new implementation.

## Initial lifecycle

The first useful loop remains:

```text
ready bead
  -> native atomic claim
  -> append the minimum assignment/execution facts
  -> launch one worker through Herdr
  -> capture exact-commit Evidence
  -> append one typed Handoff
  -> validate and append Acceptance or Rejection
  -> reflect accepted completion in the same bead/store
```

There is no cross-store application step or receipt. The necessity round fixes
which facts are one provider mutation and which are append-only records before
this loop is implemented. Deleting the old Acceptance saga happens in the same
stack as its replacement, so the four existing journeys keep an executable
continuity check.

No automatic retry, reclaim, respawn, publication, or recurring reconciliation
occurs. A successor execution is an explicit visible decision.

## Coordination

Critical instructions and progress must be reconstructible from the shared
store before Herdr is asked to deliver a transient prompt or doorbell. A
work-shaped blocker is a dependency edge. There is no mailbox, unread flag,
acknowledgement, delivery queue, or escalation ladder.

The existing Directive/Report/Request taxonomy is reopened. The necessity
round may retain a smaller typed comment vocabulary, but no source is added
merely to preserve the old API.

## Provider interfaces

### `br`

The adapter/typed facade must:

- invoke the pinned stock binary through its documented machine mode;
- inject the one absolute `BEADS_DIR` on every call;
- normalize IDs, statuses, labels, errors, and provider revisions;
- retain native atomic claim semantics;
- validate any ABACUS-owned append-only record schema selected later;
- keep unknown output loud and never partially ingest it; and
- use a process deadline because the provider's lock-timeout flag is not a
  proven whole-operation deadline.

It must not call provider-internal SQL, fork the provider, or imply direct
stock-`br` access is impossible.

### `bv`

Advice is read-only, version/hash bound, and time-bounded. Missing, stale,
partial, or malformed advice falls back to deterministic ready order.

### Herdr

The runtime adapter owns launch, inspect, prompt/doorbell, wait/read, and stop
normalization. Provider handles stay opaque outside the adapter. Live
compatibility tests remain opt-in and disposable.

## Consistency and recovery

- `br update --claim` is the only currently evidenced conditional mutation;
  two contenders produce one winner.
- No generic compare-and-set or multi-issue transaction is presumed.
- Default linked-worktree discovery is rejected because it creates independent
  databases; JSONL merge is never live coordination.
- JSONL is the portable recovery/export artifact. Provider database/JSONL
  disagreement fails closed and uses explicit pinned-provider recovery.
- A provider call killed or disconnected after submission may be ambiguous.
  No hidden retry is authorized; the necessity round decides whether an
  operation identity is needed for the first live loop.
- Before an append-only workflow encoding is accepted, a disposable pinned-
  provider test must round-trip multiple ordered records through JSONL export
  and database rebuild, proving bodies, order/IDs, and references survive.
  Merely observing a `comments` array in exported JSONL is not enough.
- No ABACUS process owns cache coherence. The superseded `SqliteState`
  constructor cache is not adapted for multiprocess use; it is deleted or
  narrowed with its consumers.

## Change model and tests

Change classes are defined by `CONTEXT.md` §7. Default tests are hermetic:
fakes, temporary Git repositories, and checked-in provider fixtures. Live
`br`, `bv`, Herdr, Claude, and Codex lanes run only for compatibility/pin
evidence or the explicit live pilot.

| Tier | Purpose |
|---|---|
| Module | Pure rules and adapter behavior |
| Consumer contract | The exact seam a direct consumer relies on |
| Hermetic journey | The smallest production composition over canonical fakes |
| Live compatibility/pilot | Provider reality, explicit and disposable |

The existing four journeys remain continuity evidence while replacement and
deletion land together. `ABACUS-2IS`, once the operator lifts the no-new-code
hold, is the preferred implementation-contact pilot because ABACUS has not yet
launched a real worker end to end.

## Operational safety

- No global hooks, shell edits, agent-home edits, or global provider config.
- No hidden daemon, auto-start, watcher, or recurring loop.
- Every destructive/recovery target is explicit and previewed.
- Provider output and paths are bounded and validated.
- Secrets never enter workflow records or prompts; the v1 store contains no
  authentication secret.
- Same-user direct access is an accepted non-goal, stated rather than obscured
  by file modes or facade language.

## Architecture acceptance criteria

- One shared absolute `BEADS_DIR` is used from the control checkout and every
  linked worktree; omission or mismatch fails loudly.
- No active component or command requires Scribe, a second Ledger, socket,
  relay, state RPC, credential, or decision guard.
- A simultaneous initial claim has one winner.
- The four measured SABLE data-shape failures cannot be represented by the
  selected typed record set.
- The selected append-only record encoding passes an exact disposable JSONL
  export/database-rebuild round trip before implementation depends on it.
- A Handoff is bound to exact Evidence/commit identity and is not a bead.
- Acceptance and Publication remain distinct.
- Runtime observations alone cannot complete work.
- The default test suite is hermetic; the first real loop is exercised by an
  explicit live pilot before additional machinery is accepted.

## Open decisions blocked on the necessity round

1. Minimum append-only record kinds and their stock-`br` encoding.
2. Which Assignment/Attempt lifecycle values are data versus derived view.
3. Whether time leases, numeric fencing, or operation idempotency are necessary
   in the single-orchestrator trusted-local loop.
4. Minimum decision-owner attribution retained after capability/scope removal.
5. Whether any durable runtime association or Signal taxonomy survives.
6. Final placement or deletion of the transitional `abacus-state` crate.
