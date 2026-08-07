# ADR-0006: Stock `br` is the single durable workflow store

- **Status:** **Accepted by the operator** (2026-08-07), revision 1; Claude C2
  cross-review **PASS** (2026-08-07).
- **Date:** 2026-08-07
- **Decider:** operator (Dylan Delli Colli)
- **Supersedes:** ADR-0001's separate-Ledger and two-store decisions,
  ADR-0002's runtime authorization/occupancy decisions, ADR-0003 in full, and
  the never-accepted ADR-0005 proposal.
- **Companions:** `CONTEXT.md`,
  `docs/compatibility/2026-08-04-br-bv.md`,
  `docs/compatibility/2026-08-07-ledger-write-boundary.md`, and rationale bead
  `ABACUS-9WJ`.

## Context

ABACUS built a second SQLite store, a resident recorder, a Unix protocol, two
Linux carriages, caller-identity machinery, and a two-store reconciliation
saga before it built an executable. Implementation contact then established
that the resident process supplied no needed resident capability: no push,
timer, process-owned truth, connection pool, or shared in-memory state.

The transport complexity was self-created. An ordinary sandboxed Codex worker
can write its worktree but cannot write the linked repository's Git common
directory, where the proposed Ledger lived. The same worker can write a
worktree-resident `.beads` directory. Pinned `br` already supplies daemonless
SQLite transactions plus a Git-portable JSONL representation.

Default `br` discovery is not sufficient: each linked worktree discovers its
own `.beads` directory and therefore its own database. Those stores converge
only through Git/JSONL merge, too late for live coordination and through the
same merge-conflict path that cost the predecessor real time. Pinned `br`
accepts an explicit absolute `BEADS_DIR`, so every process can instead address
one control checkout's store.

The threat model was also wrong. Legacy failures came from accreted machinery
and honest-but-unreliable agents—crashes, hangs, stale observations, and
confidently incorrect actions—not adversarial agents stealing identities.
Cryptographic or process boundaries do not repair bad orchestration machinery;
they add another system that can fail.

The SABLE evidence still demands four protections, all of which are data shape
rather than access control:

1. mutable notes overwrote an operator authorization and left four dangling
   dependents, so authorization and decision facts must be append-only;
2. `CLOSED` was mistaken for `LANDED`, releasing work whose commit was not
   contained, so accepted completion and Publication must remain distinct;
3. one coarse status could not distinguish claimed, launched, parked, dead,
   and successor execution, so those lifecycle facts must remain distinct; and
4. representing Handoffs as work beads produced 22 duplicate P1s in one
   afternoon, so a Handoff is a typed record attached to work, never another
   bead.

## Decision

### 1. One stock store, selected explicitly

Pinned, unforked `br` is the only durable ABACUS store. One control checkout's
absolute `.beads` directory is injected as `BEADS_DIR` into every ABACUS
process and agent launch. Product behavior never relies on current-directory
discovery. Per-worktree databases, Git merge as live arbitration, and a
temporary-directory canonical store are rejected.

Codex launches receive the exact shared `.beads` directory as an additional
writable root. That grant is intentional: direct same-user access to the work
store is the v1 operating model, not a security boundary accidentally widened.
The location contains no second database or secret.

### 2. No Scribe, transport, or second Ledger

ABACUS builds no `state.sqlite3`, Scribe process, socket, relay, state RPC,
framing protocol, credential, decision guard, repository-ID projection, or
state-service lifecycle. SQLite serialization and recovery remain provider
implementation details behind stock `br`.

Agents may use stock `br` directly. ABACUS may provide typed convenience
commands and composition over the same provider, but those commands are not an
exclusive writer, authentication layer, or containment boundary. A raw `br`
call can bypass an ABACUS convention; v1 accepts that trusted-local fact and
does not describe the convention as enforced.

### 3. Mutable current fields, append-only workflow facts

Native issue fields remain the current work-graph view. Workflow facts whose
replacement would destroy history—at minimum authorizations, decisions,
execution lifecycle, Evidence, and Handoffs—use an append-only, structured,
per-bead representation that survives `br`'s JSONL export and rebuild.
Corrections append; they do not overwrite an earlier fact.

The exact record set and encoding are intentionally **not** fixed by this ADR.
They are the subject of the operator-required necessity round before new
source is written. Stock `br` comments are the available append primitive and
their IDs can provide order, but adopting a versioned comment schema or a
reducer is a separate implementation decision. Mutable notes are never the
canonical carrier for an append-only fact. Native `br` audit events are not
canonical for ABACUS: upstream documents them as local-database-only and they
are not exported to JSONL.

No generic compare-and-set or multi-issue transaction is presumed. The one
provider guarantee already evidenced and retained is atomic claim: two
simultaneous `br update --claim` calls yield one winner. Any stronger
transition must first prove it is necessary. If it can be expressed as data
shape, it stays data shape; if it needs enforcement, that is a new architecture
decision rather than a hidden wrapper.

### 4. Keep stock `br`; do not return to `bd`

`bd` resolves one store across linked worktrees, but does so through a Dolt
server. Returning to it would restore the resident process and measured legacy
tax: deadlocks, 15/15 read timeouts in a 15-worker test, and 419 commits of
remote drift. One injected directory is cheaper.

The tracker choice remains a two-way door because both tools export portable
JSONL. A `br` fork is a one-way maintenance commitment and is deferred. Fork
only after a real failure proves a missing primitive, the need cannot be
expressed as data shape, and upstream will not accept the change.

### 5. Delete by replacement, not by creating holes

The separate-store application-attempt, receipt, causal-supersession, pending
projection, and reconciliation subsystem is obsolete in the target design.
It is not deleted ahead of its consumer replacement: the current four journeys
still execute that composition, and removing it first would erase behavior
rather than simplify it.

The only immediate source subtraction authorized before the necessity round is
the proven test-only authorization cluster:

- `ValidatedProfileSet::authorize`;
- `AuthorizationTarget`;
- `ActionContext`;
- `AuthorizationRefusal`; and
- `StateError::ScopeUnauthorized`.

Even that subtraction follows this normative landing. Broader authority,
scope, audit, fencing, and state types have live consumers and move only with
the facade stack that replaces those consumers.

## Required necessity round

No new implementation follows directly from this ADR. Before source is added,
the operator reviews each surviving concept against an observed failure or the
smallest runnable journey. The round must decide the minimum record set and
whether each of Assignment, Attempt, lease, numeric fencing, operation
idempotency, Signal taxonomy, audit index, profile activation, decision-owner
metadata, and runtime association earns its cost.

The four measured data-shape protections above and exact-commit Evidence/
Handoff binding are the floor. Existing Rust types are evidence about prior
implementation, not proof that a concept survives. The live-provider pilot
`ABACUS-2IS` is the preferred implementation-contact check once the operator
lifts the no-new-code hold; it is not authorized by this ADR.

Before the round accepts any append-only encoding, a disposable provider lane
must append multiple ordered records, export JSONL, discard/rebuild the
database through the pinned stock provider, and prove record bodies, IDs/order,
and references survive exactly. Export presence alone is insufficient. A
failed or lossy round trip rejects that encoding and reopens the data-shape
decision; it is never waived as a backup limitation.

## Consequences

- `br` becomes both work graph and workflow-fact substrate, so the entire
  two-store consistency problem disappears.
- There is no ABACUS-owned resident process or state transport to install,
  monitor, authenticate, migrate, or recover.
- Shared-store selection and the exact Codex writable-root grant become
  launch/configuration obligations and must fail loudly when absent or wrong.
- Direct provider access means ABACUS cannot claim that arbitrary transitions
  are mechanically prevented. Visibility, append-only shape, exact-commit
  checks, and native atomic claim are the v1 controls.
- The existing `abacus-state` source is transitional and has no production
  data to migrate because ABACUS has not shipped an executable. It is removed
  or narrowed only alongside its replacement.
- Compatibility evidence about Codex Unix sockets and Git-common-directory
  writes remains historically valid, but it no longer imposes a product
  transport.

## Change class and validation

This is a C2 architecture replacement with eventual C3 fan-out. This document,
the same-commit `CONTEXT.md` amendment, explicit supersession markers in
ADRs 0001–0003, and withdrawal of ADR-0005 form the binding decision. Source
changes require their own cross-reviewed tranche after the necessity round.

Documentation validation must prove there is no active claim that ABACUS uses
a second Ledger, Scribe, a state socket/RPC, exclusive state writer,
write-time scope authorization, or per-worktree `br` discovery. Historical
compatibility records and explicitly superseded/parked ADR text may retain
those terms when clearly labeled as history.
