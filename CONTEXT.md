# ABACUS — Context

**Status:** normative; revised 2026-08-07 by ADR-0006.
**Companions:** `README.md` (purpose and map) · `docs/architecture.md`
(flows) · `docs/migration.md` (build sequence) ·
`docs/adr/0006-stock-br-single-store.md` (current store decision).

This file defines ABACUS domain language, ownership boundaries, invariants,
and failure semantics. Role cards, skills, module contracts, and ADRs must not
contradict it. A conflict is a defect to fix, never an override.

## 1. Why ABACUS exists

ABACUS is a clean successor beside legacy SABLE. SABLE's useful center—role
cards, orchestration policy, a small set of skills, and bead-led execution—was
buried under infrastructure that accumulated around it:

- 263 of 1,712 closed legacy beads were undelivered-message fallback records;
- a small durable coordination need acquired watchers, timers, reconcilers,
  and delivery heuristics;
- completion evidence held only when bound to an exact commit SHA; and
- a 1,007-line terminal-scraping library with 29 state predicates became the
  largest provider-drift surface.

ABACUS is **beads that compute**: work is represented by beads, orchestration
advances it, and local agents execute it. Infrastructure supports that loop;
it is not the product. The governing threat model is honest but unreliable
local agents: they may crash, hang, use stale facts, or be confidently wrong.
V1 does not build a security boundary against a malicious same-user process.

## 2. Domain vocabulary

| Term | Definition | Authority |
|---|---|---|
| **Bead** | One unit of work with an `ABACUS-` ID, content, dependencies, mutable work status, and append-only workflow records. | The shared stock `br` store |
| **Work graph** | Beads, dependency edges, and current work status. | `br` |
| **Shared store** | One control checkout's absolute `.beads` directory, selected by injected `BEADS_DIR` for every ABACUS process and agent. It is the only durable ABACUS store. | Pinned stock `br`; JSONL is the portable representation |
| **Workflow record** | A typed, append-only fact attached to a bead and exported through `br` JSONL. Corrections append rather than overwrite. The necessity round fixes the smallest closed record set before implementation. | ABACUS record schema over stock `br` |
| **Assignment** | A durable fact binding one bead to one worker and naming its decision owner. It is attached to that bead, not stored in a second database. | Shared store |
| **Attempt** | One concrete worker execution under an Assignment. Successors are new Attempts; they never rewrite a predecessor. Evidence and Handoffs identify the Attempt that produced them. | Shared store |
| **Lifecycle fact** | A typed fact that keeps claimed, launched, parked, dead/stalled, and successor execution distinguishable. It is not collapsed into one provider status. | Shared store |
| **Authority class** | Exactly `orchestrator` or `worker`. Specialist behavior is a role/profile concern, never a third core class. | `abacus-core` |
| **Agent profile** | A named role-card configuration describing one agent's class, responsibilities, and provider launch defaults. It guides and routes honest agents; it is not a security principal. | Project-owned role card/configuration |
| **Decision owner** | The actor named by an Assignment as responsible for its decisions. The name provides stable ownership and attribution, not cryptographic authorization. | Assignment record |
| **Agent** | A Claude or Codex provider session occupying a profile. | Runtime provider observation |
| **Runtime handle** | Herdr's opaque, generation-bearing identifier for a live session. It is provider-scoped and never proves workflow completion. | `abacus-runtime` |
| **Prompt** | Transient live content delivered through Herdr. A prompt may be lost; no critical instruction may exist only there (I6). | Herdr |
| **Coordination record** | A durable, typed, subject-bound instruction, progress report, blocker, or decision fact attached to the relevant bead. Exact Signal taxonomy is reopened by the necessity round; no inbox or acknowledgement state is implied. | Shared store |
| **Evidence** | The verification command, observed outcome/exit code, timestamps, and exact commit/tree identity. It is captured by the execution wrapper rather than accepted as a model-supplied boolean. | Shared store |
| **Handoff** | A typed completion claim naming a clean local commit and matching Evidence from one Attempt. A Handoff is never a bead. | Shared store |
| **Submission refusal** | A failed Handoff precondition before a Handoff record exists. The worker can correct it without pretending completion or rejection occurred. | Core policy; visible result/record as selected by the necessity round |
| **Rejection** | An append-only decision against a recorded Handoff. A later retry is a successor Attempt, not an edit to history. | Shared store |
| **Acceptance** | An append-only decision that a Handoff satisfies policy, paired with the bead's accepted/closed work status in the same store. | Shared store |
| **Publication** | Push, PR, merge, deploy, or containment in a downstream branch. It is distinct from Acceptance and never implied by `closed`. | Operator or explicit workflow |
| **Advisor** | Optional, read-only `bv` analysis. It may reorder candidates but is never required for correctness. | `abacus-work` advisor seam |
| **Facade** | Typed ABACUS commands and composition over providers. It improves ergonomics and record shape but is not an exclusive writer or security boundary; agents may also use stock `br` directly. | `abacus-cli` and module adapters |
| **Envelope** | Launch context rendered from durable bead/Assignment/Attempt facts and delivered to a worker. Critical content must remain reconstructible from the shared store after context loss. | Shared store facts plus runtime delivery |

## 3. Ownership boundaries

| ABACUS owns | Provider owns |
|---|---|
| Typed workflow-record shapes, evidence/Handoff policy, lifecycle vocabulary, and composition | SQLite transactions, JSONL import/export, issue fields, comments, dependencies, and atomic claim (`br`) |
| Role definitions and orchestration conventions | Session/process mechanics and live prompt delivery (Herdr) |
| Completion versus Publication semantics | Optional graph analytics and prioritization (`bv`) |
| Provider-neutral composed agent state | Claude/Codex model behavior |

There is one durable store. Current issue fields and append-only workflow facts
coexist on the same bead; there is no application projection, receipt,
supersession record, or reconciliation saga between two authorities.

All linked worktrees address the same explicit absolute `BEADS_DIR`. Default
walk-up discovery is forbidden for product composition because it selects a
different `.beads` directory in each worktree. Direct access is intentional:
ABACUS conventions make mistakes visible and its typed shapes make common
conflations harder, but v1 does not claim raw `br` calls are impossible.

## 4. Invariants

I1. **All work is bead-led.** No agent acts without an Assignment naming a
bead. Discovery that is not acted on is captured, not executed.

I2. **One shared durable store.** Pinned stock `br`, selected through one
injected absolute `BEADS_DIR`, holds both the work graph and durable workflow
facts. ABACUS builds no second Ledger, Scribe, state RPC, or per-worktree
coordination store.

I3. **V1 uses conventions plus visibility, not an access boundary.** Native
atomic claim arbitrates the initial claim race. Other lifecycle and decision
rules are expressed through typed data and orchestration convention unless a
measured failure earns enforcement. A convenience facade never describes
direct provider access as impossible or authenticated.

I4. **Evidence records outcomes bound to artifacts.** Command, observed exit
and normalized outcome, and exact commit/tree identity are captured by the
execution wrapper. Prose, intent, and file presence are not evidence. Policy
may require red against the declared-base implementation and green at the
Handoff commit; execution/collection failure never satisfies red.

I5. **Completion is an accepted Handoff.** It requires a clean commit, matching
bound Evidence, and a passing policy outcome. Completion never implies
Publication, and Handoffs never enter the ready graph as beads.

I6. **Nothing critical rides only the transient channel.** Herdr prompts may
be lost. Assignments, launch facts, critical direction, Evidence, Handoffs,
and decisions remain reconstructible from the shared store. V1 has no durable
mail, inbox, acknowledgement, or delivery-retry subsystem.

I7. **UNKNOWN is a first-class agent state.** Contradictory, stale, missing, or
ambiguous runtime facts never become inferred idle, success, or safe reuse.

I8. **Advisors advise.** Invalid, stale, or absent `bv` output falls back to a
deterministic ordering and cannot block correct work.

I9. **Provider output is untrusted input.** Adapters validate and normalize
machine output. Provider versions are pinned and checksummed; `br` and Herdr
mismatches fail closed, while `bv` degrades per I8.

I10. **Mutable current state is distinct from append-only history.** Native
issue fields may change. Authorization/decision facts, execution lifecycle,
Evidence, and Handoffs use a selected append-only provider shape—never mutable
notes—and compliant corrections append rather than overwrite. This is a data-
shape convention, not a security boundary: direct local access can still
delete or corrupt provider state, and ABACUS refuses/surfaces a malformed or
broken record chain when it can observe one rather than claiming overwrite is
impossible. Before any encoding is adopted, a disposable compatibility test
must prove its records and order survive JSONL export **and database rebuild**.
Native `br` audit events are local-only diagnostics and are not ABACUS's
canonical history.

I11. **Namespace discipline.** External IDs use `ABACUS-`; configuration uses
`.abacus/` and `ABACUS_*`; binaries use `abacus`/`abx`. New state paths never
reuse SABLE names.

I12. **No ABACUS-owned resident process or hidden loop.** Herdr is the external
persistent runtime provider. ABACUS adds no daemon, watcher, sweeper, retry
loop, or polling service. Explicit operator actions may reconcile visible
facts; recurring machinery requires its own evidence and ADR.

I13. **Environment is injected, never ambient.** The composition root resolves
and passes `BEADS_DIR`, provider binaries, clocks, paths, and policy inputs.
Inner modules do not discover current directories or global agent config on
their own.

I14. **Blast radius is classed and bounded.** C0 is internal, C1 is an additive
seam extension, C2 is a breaking seam or new cross-module dependency, and C3
changes core. C2 needs an ADR; C3 runs the full hermetic workspace gate.

I15. **Core is minimal by rule.** A type enters `abacus-core` only when it is a
pure, stable domain invariant shared by at least two modules. Existing types do
not survive the necessity round merely because code already uses them.

I16. **Topology is configuration.** Core knows only orchestrator and worker.
Named agents and responsibility routing live in project-owned cards/config.
V1 does not require capability grants, exclusive scope algebra, profile
occupancy, or activation state to authenticate local calls.

I17. **Decision ownership and attribution stay visible.** Each Assignment names
one stable decision owner and append-only decisions identify their actor when
known. This prevents ordinary ownership confusion; it is not a claim that
another same-user process cannot write the store. Scope authorization and
cryptographic actor authentication are not v1 guarantees.

I18. **Execution churn is explicit.** Claimed, launched, parked, dead/stalled,
and successor facts remain distinguishable; retries append new Attempts and
never rewrite predecessors. No retry, reclaim, or respawn occurs automatically.
Whether leases or numeric fencing survive is decided by the necessity round,
not assumed by this invariant.

I19. **Durable coordination is typed, subject-bound, and ack-free.** Critical
direction and workflow progress attach to the relevant bead/Assignment/
Attempt as append-only facts. Work-shaped blockers remain dependency edges.
There is no unread flag, acknowledgement state, delivery queue, or escalation
ladder. The exact Directive/Report/Request taxonomy must re-earn itself in the
necessity round.

## 5. Agent-state model

Three inputs remain distinct:

1. **Workflow facts** from the shared `br` store: what was assigned, launched,
   parked, ended, handed off, or accepted.
2. **Process liveness** from Herdr: whether a session appears live, exited,
   stale, or unavailable. It is advisory.
3. **Semantic phase** from append-only worker records. Silence is not a phase.

Composition is pure and provider-neutral. Contradictory or stale inputs yield
UNKNOWN. Runtime output never completes work. A stalled or dead worker is
preserved for an explicit orchestrator decision; no timer silently reassigns
it. The necessity round chooses the smallest persisted facts needed by the
first live loop.

## 6. Failure semantics

“Loud” means a distinct machine-readable result presented to the caller, with
no hidden retry or fallback. A durable refusal record is required only if the
necessity round retains one; failure reporting must not manufacture a second
audit system.

| Condition | Required behavior |
|---|---|
| `BEADS_DIR` missing, relative, wrong, or not writable | Refuse before mutation. Never fall back to current-directory discovery or a per-worktree store. |
| Shared `br` unavailable or busy | Return a distinct provider failure. A process deadline may bound the call; a lost post-submit response remains ambiguous rather than guessed. |
| Concurrent claim | Native atomic claim yields one winner; the loser rereads and reports the current assignee. |
| Lost live prompt | Durable critical facts remain in the shared store; no delivery queue or retry daemon appears. |
| Worker stalled, dead, or contradictory | Compose UNKNOWN/stalled state and preserve the worktree. Any successor is an explicit decision and append-only Attempt. |
| Malformed provider output | Reject the whole response; never partially ingest or infer missing fields. |
| Advisor unavailable or invalid | Use deterministic fallback ordering (I8). |
| Dirty Handoff candidate | Refuse submission with the dirt enumerated; record no Handoff. |
| Missing, mismatched, or failing Evidence | Refuse submission or reject the recorded Handoff with a distinct reason; never close the bead. |
| Direct `br` mutation violates an ABACUS convention | Surface the inconsistent visible facts when observed and require an explicit correction. It is not reconciled against another store and is not described as a security breach. |
| Provider version mismatch | `br`/Herdr operations fail closed; `bv` degrades per I8. |
| `br` database/JSONL disagreement or corruption | Fail closed, preserve provider recovery artifacts, and require explicit diagnosis/rebuild under the pinned provider contract. |

## 7. Change classes and testing doctrine

| Class | Change | Edit-loop scope | Gate |
|---|---|---|---|
| **C0** | One module, no public contract change | Owning module tests | Ordinary review |
| **C1** | Additive compatible seam | C0 plus direct-consumer checks | Ordinary cross-review |
| **C2** | Breaking seam or new cross-module dependency | Planned affected fan-out | ADR first |
| **C3** | `abacus-core` change | Full workspace | Core-admission justification and cross-review |

Default tests are hermetic: fakes and pinned fixtures, no live providers,
network, or user home. Provider compatibility lanes are explicit. The bounded
workspace suite is the Handoff backstop; it does not justify widening every
edit loop. Test-budget growth is an ADR event, never a reason to build a test
selection subsystem.

## 8. Non-goals

- a second Ledger, Scribe, state socket/RPC, or ABACUS daemon;
- per-worktree coordination databases or Git merge as live arbitration;
- returning to Dolt-backed `bd` or pre-emptively forking stock `br`;
- protection against a malicious same-user process or raw-`br` bypass;
- migration of legacy SABLE workflow history;
- automatic push, PR, merge, deploy, or Publication on Acceptance;
- a mailbox, acknowledgement protocol, delivery retry system, or attention
  service without an observed need;
- universal TDD or CI-efficiency machinery; and
- compatibility layers for failures not yet observed.
