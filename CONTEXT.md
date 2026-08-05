# ABACUS — Context

**Status:** draft for adversarial review (Claude-authored; Codex to cross-review).
**Companions:** `README.md` (purpose, status, map) · `docs/architecture.md` (components, flows) · `docs/migration.md` (relationship to legacy SABLE) · `docs/adr/0001-modular-architecture.md` (foundational decisions and their rationale).

This file defines the domain language, ownership boundaries, invariants, and failure semantics of ABACUS. Role cards, skills, and module contracts must use this vocabulary and must not contradict these invariants. If a later document conflicts with this one, the conflict is a defect to resolve, not an override.

## 1. Why ABACUS exists

ABACUS is a clean successor to, and parallel build beside, legacy SABLE. SABLE's useful center — role cards, orchestration policy, a small set of skills, and bead-led execution semantics — became entangled with the infrastructure that carried it: a Dolt-backed bead store, tmux screen-scraping, CI-efficiency machinery, and rigid TDD enforcement. The 2026-08 audits of the legacy repository measured the cost of that entanglement:

- 15% of all closed legacy beads (263 of 1,712) were undelivered-message fallback records — a lossy transport's failures became tracker exhaust (source: legacy `sable-potential-improvements.md` §1).
- The messaging stack's durable-payload logic was a small core buried under watchers, timers, reconcilers, and delivery-verification heuristics that existed to compensate for a non-durable transport.
- The only completion evidence that ever held was bound to an exact commit SHA (the merge gate). Evidence not bound to an artifact degraded into "a test-shaped command was about to run."
- A 1,007-line terminal-scraping library carrying 29 state-detection predicates, grown one incident at a time, was the single largest provider-drift surface (source: legacy `HERDR-EVALUATION.md`).

Every invariant in §4 is a refusal to rebuild one of those. The product model is **beads that compute**: work is represented by beads, orchestration advances that work, local agents execute it. Infrastructure supports the loop and is replaceable; infrastructure is not the product.

## 2. Domain vocabulary

The authoritative language of the system. Terms are capitalized as defined here when used normatively.

| Term | Definition | Authority |
|---|---|---|
| **Bead** | One unit of work, with an `ABACUS-` prefixed ID, description, and dependencies. | Content, dependencies, work status: `br` (via the work facade). Decision lifecycle around it: the Ledger. |
| **Work graph** | The set of beads, their dependency edges, and their work status. | `br`, accessed exclusively through `abacus-work`. |
| **Assignment** | The durable record binding one bead to one worker and naming its exact decision actor. An Assignment may contain several sequential Attempts; a failed, rejected, expired, or revoked Attempt does not rewrite or implicitly terminate the Assignment. Created through Scribe. Assignments are ABACUS state, never stored in `br`. | Ledger |
| **Attempt** | One worker execution under an Assignment, carrying a Lease. Reclamation, rejection, expiry, or revocation ends that Attempt; an authorized decision actor may explicitly retry by appending a new one. Evidence and Handoffs bind to the Attempt that produced them. | Ledger |
| **Lease** | Time-bounded exclusivity held by the current Attempt of an Assignment, with fencing tokens monotonic across that Assignment's Attempts. Expiry makes the Attempt *reclaimable*; nothing silently reassigns. Every mutating facade call from a worker carries its lease token; a lost lease fails the call. | Ledger |
| **Authority class** | One of exactly two first-class types: `orchestrator` or `worker`. The only role taxonomy core knows. Specialist behaviors are skills or profiles, never new classes. | `abacus-core` |
| **Agent profile** | A named, configured agent identity — including any future manager or watchdog — composed from an authority class plus explicit capabilities and responsibility scopes/routes. A profile is defined solely by its role card's frontmatter; repository config *references* cards and defines no second profile schema. Profile load/activation is an audited Ledger event. | Role card frontmatter, validated by core |
| **Role card** | Authored Markdown with machine-readable frontmatter declaring a profile: its authority class, capabilities, and responsibility scopes/routes. Cards are authored and validated, never generated. Prose in a card teaches; it never enforces. | `abacus-core` (validation) |
| **Decision actor** | The concrete identity recorded on every assignment, acceptance, reclamation, and automated action: ActorId plus authority class, profile name, and the profile content hash in force at decision time. The fencing identity that prevents races between managers. | Ledger |
| **Agent** | A provider session (Claude or Codex) occupying an Agent profile. | — |
| **Runtime handle** | The runtime substrate's identifier for a running agent session (e.g., a Herdr session). Ephemeral, provider-scoped, and never used as a durable ABACUS identity. | `abacus-runtime` |
| **Prompt** | A live message delivered to a running agent session by Herdr, which owns all live agent messaging and prompt delivery. Transient transport: a prompt may be lost, and nothing critical may exist *only* in a prompt (I6). | Herdr, via `abacus-runtime` |
| **Signal** | A durable, immutable, typed communication record: fenced sender (full decision-actor identity) plus a **required subject reference** — bead, assignment, attempt, or scope. Exactly three types: Directive, Report, Request. Signals carry no read/ack state; an unresolved Signal is a derived query (a Signal lacking its linked responding action), never an inbox flag (I19). Attention is a Herdr doorbell; content lives only in the Ledger. | Ledger |
| **Directive** | Orchestrator→Attempt Signal: amended instructions, pause, abort, or an answer to a Report. Binding on the current Attempt from the moment it commits — a worker acts on current Directives at each facade interaction; unread is still in force. | Orchestrator-class actor, via facade |
| **Report** | Worker→decision-actor Signal: structured progress or blocked-with-reason from the current Attempt. Resolved by a responding Directive or decision. | Worker, via facade |
| **Request** | Actor→actor Signal — the orchestrator↔orchestrator channel: arbitration, authority transfer, reconciliation, or another decision-shaped ask. Resolved by the responding fenced decision, never by an acknowledgement. | Any decision actor, via facade |
| **Evidence** | An outcome record: the verification command, its exit code, timestamps, and the commit/tree identity it ran against. Recorded by the facade's verification wrapper, appended to the Ledger. | Ledger |
| **Handoff** | A worker's completion claim: a clean local commit plus Evidence bound to that exact commit, recorded against its Attempt. | Ledger |
| **Submission refusal** | Failure of a Handoff precondition before a Handoff is recorded, such as a dirty tree or missing evidence. It records an audit event but no Handoff or decision, and leaves the current Attempt active so the worker can correct the defect. | `abacus-core` policy, persisted by Scribe |
| **Rejection** | An immutable orchestrator decision against an already recorded Handoff. Rejection is terminal for that Attempt, not for its Assignment; an authorized decision actor may explicitly retry by appending a new Attempt, which may remain assigned to the same worker. | Orchestrator-class actor, via facade |
| **Acceptance** | An immutable orchestrator decision authorizing a valid Handoff. Scribe commits that decision and terminally accepts the Assignment/Attempt before the facade changes `br`; a later immutable application receipt confirms the work-status mutation (§3). | Orchestrator-class actor, via facade |
| **Ledger** | The SQLite (WAL) database at `<git-common-dir>/abacus/state.sqlite3`: transactional current-state tables (assignments, attempts, leases) plus append-only audit events and immutable Envelope, evidence, Handoff, decision, application-attempt, and application-receipt records. Sole authority for assignment, attempt, lease, evidence, handoff, and decision lifecycle. | `abacus-state` |
| **Scribe** | The recorder process (internal name `abacus-scribe`; operated via `abacus scribe status\|start\|stop`) — the only writer to the Ledger, listening on `$XDG_RUNTIME_DIR/abacus/<repo-id>.sock`. Scribe persists durable workflow facts — assignments, attempts, leases, sanitized Envelope snapshots, evidence, handoffs, decisions, application attempts/receipts, audit, repository identity — and nothing else. It is **not** a manager, messenger, watcher, scheduler, retry loop, or policy engine. Agents reach durable state only through it, never by writing under `.git` directly. | `abacus-state` |
| **Advisor** | `bv`. Optional, read-only analysis that may inform ordering. Never authoritative, never required for correctness. | `abacus-work` (advisor port) |
| **Facade** | The `abacus` CLI (alias `abx`): the only interface roles and skills may use. | `abacus-cli` |
| **Envelope** | The ABACUS-generated context handed to a spawned worker: assignment, bead content, worktree path, constraints, lease token. Envelopes are durable — recorded with their Assignment in the Ledger; the live prompt that delivers one is transient transport. | Ledger (persistence), `abacus-runtime` (delivery), `abacus-core` (shape) |
| **Publication** | Push, PR creation, merge, or deploy. Always a separate, explicitly authorized action — never part of completion. | Operator / explicit workflow |

## 3. Ownership boundaries

| ABACUS owns | Provider owns |
|---|---|
| Assignment, attempt, lease, Envelope, evidence, handoff, Signal, and decision/application lifecycle; audit; repository identity (Ledger) | Work-graph content, dependencies, and work status (`br`) |
| Role definitions, privileges, lifecycle transitions | Session/process mechanics and all live agent messaging/prompt delivery (Herdr) |
| Completion and acceptance semantics | Graph analytics and prioritization signals (`bv`) |
| Agent-state composition (§5) | Model/provider behavior (Claude, Codex) |
| The command surface agents see (`abacus`) | Their own CLI surfaces — which agents never see |

The two stores split cleanly: `br` is canonical for what the work **is** — content, dependencies, work status; the Ledger is canonical for what was **decided** about it — assignments, attempts, leases, evidence, handoffs, decisions. Neither is a projection of the other.

Acceptance/close is an explicit two-store sequence: (1) one Scribe transaction commits the immutable Acceptance decision — actor, evidence reference, commit identity, operation identity — and terminally moves the Assignment/Attempt to `accepted`; (2) the work facade attempts the corresponding `br` status change; (3) Scribe appends the normalized application attempt and, on confirmed success, an immutable application receipt. The step-1 decision is authoritative; step 3 records its projection outcome and is not a second decision. The reconciliation set is derived as accepted decisions lacking a successful application receipt—there is no `accepting` state or background queue. If step 2 or 3 fails, explicit retry/reconciliation uses the committed operation identity idempotently, the Assignment remains accepted, and the divergence stays loud until resolved. A `br` status change with no corresponding Ledger decision (an out-of-band mutation) is a detectable anomaly surfaced to the operator (§6) — never silently adopted, never silently reverted.

## 4. Invariants

A standing design assumption underlies all of these: **ABACUS itself is the thing that changes most**. Module implementations, adapters, and policy will evolve continuously and at different rates; the host environment (tools, paths, shells, agent TUIs, provider versions) drifts too, but as a secondary, slower stream. The architecture is therefore judged by how small a well-placed change can stay: interfaces are deep and stable, state and tests are module-owned, and blast radius is an explicit, classed property of every change (§7) — never an emergent surprise.

I1. **All work is bead-led.** No agent acts without an Assignment naming a bead. Discovery that isn't acted on is captured, not executed.

I2. **One write path per store.** `br` is mutated only by `abacus-work`. The Ledger is written only by Scribe. Roles and skills use the facade exclusively; a role card or skill containing raw `br`, `bv`, `herdr`, or legacy `bd` invocations fails validation (mechanically checked, not prose-policed).

I3. **Authority is split by store; decisions gate status.** `br` is canonical for work-graph content, dependencies, and work status; the Ledger is canonical for assignment, attempt, lease, Envelope, evidence, handoff, decision, and application lifecycle. A facade-mediated status change happens only after its authorizing decision has committed to the Ledger (§3). `abacus-work` returns revisioned provider observations/anomaly signals; the core use case correlates those with Ledger decisions through ports. Out-of-band `br` mutations are surfaced, never silently adopted or repaired, and `abacus-work` never reads the Ledger directly.

I4. **Evidence records outcomes bound to artifacts.** Command, exit code, commit identity — captured by the facade wrapper at execution time. Prose claims, file existence, and intent records are not evidence. A verdict or receipt must be derivable and checkable; the presence of a file is never proof. An acceptance policy may require a **red-green evidence pair** — the Handoff commit's policy-named verification failing *at assertion level* against the declared-base implementation (overlay capture), and passing at the handoff commit — as the structural counter to tests that cannot fail. Execution and collection errors are a distinct honest outcome that never satisfies red. Unsupervised autonomous runs default to this form.

I5. **Completion is an accepted Handoff.** Clean commit, matching bound evidence, passing outcome. Completion never implies Publication.

I6. **Nothing critical rides the transient channel.** Herdr owns all live agent messaging; prompts are transient transport and may be lost without loss of correctness. Every critical instruction lives durably in an Assignment, Envelope, or Signal (I19) in the Ledger, and an agent can always re-derive its obligations from Scribe state. v1 deliberately has no durable mail, inbox, acknowledgement, or delivery-retry machinery — the legacy compensation stack existed to make a transient channel pretend to be durable; ABACUS instead keeps durable facts in the Ledger and lets the transient channel be transient. Encoding a critical instruction only in a prompt is a defect, not a reason to add mail.

I7. **UNKNOWN is a first-class agent state** (§5). It is never coerced to idle, never dispatched to, and never silently reclaimed from.

I8. **Advisors advise.** `bv` output may reorder ready work. Its absence, staleness, or malformed output changes nothing about correctness — a deterministic fallback ordering always exists.

I9. **Provider output is untrusted input.** Validated and normalized at the adapter boundary. Upstream versions are pinned and checksummed; a mismatch causes the adapter to refuse loudly (fail closed). The advisor is the one exception: `bv` mismatch degrades to the deterministic fallback instead.

I10. **The Ledger separates current state from immutable record.** Current state (assignments, attempts, leases) lives in transactional tables; every state-changing operation appends an audit event; Envelope snapshots, evidence, Handoffs, decisions, application attempts, and application receipts are immutable once written. Pending projection work is a query over decisions lacking a successful receipt, not another mutable queue. Corrections are new records — history is never rewritten. Full event sourcing is deliberately not v1 machinery.

I11. **Namespace discipline.** `ABACUS-` bead IDs, `.abacus/` config root, `ABACUS_*` environment, `abacus`/`abx` binaries. No new SABLE-branded roots, and no reuse of legacy SABLE state paths.

I12. **No additional ABACUS-owned resident processes or loops beyond Scribe.** Herdr is the external persistent runtime provider; ABACUS adds no timers, watchers, sweepers, or polling daemons alongside it. Explicit, on-demand reconciliation — invoked by an operator or an authorized decision actor — is permitted; a *recurring* need for it indicates a transport or authority defect to fix, not a schedule to automate. A watchdog is a spawned Herdr-managed profile (I16), never another ABACUS daemon. (This is the anti-compensation invariant; it is the legacy failure mode stated as a prohibition.)

Boundary reading (2026-08-05, operator-ratified): this invariant is a kernel-correctness guarantee and a v1 placement rule, not a permanent ecosystem prohibition. Kernel correctness never depends on a recurring process — no core or state behavior may require hidden recurring machinery to be correct. A later, explicitly authorized policy/operations module may provide liveness (scheduled reconciliation, CI observation, alerting, delivery follow-up); its absence or failure may delay progress but can never corrupt or invent workflow state. Such a module crosses this boundary the way mail crosses I19's — by its own ADR, consuming existing seams — and Scribe remains a recorder throughout. Safety is kernel; liveness policy is modular.

I13. **Environment is injected, never ambient** — adapter hygiene, not the design center. Modules receive paths, clocks, sockets, and provider binary locations explicitly from their composer (`abacus-cli`, or Scribe at startup); no module reads process environment or current-directory context on its own initiative. The payoff is that module-local tests are trustworthy without any live host arrangement.

I14. **Blast radius is classed and bounded.** Every change has an explicit class (§7). An internal module change exercises that module's own tests in the edit loop — never other modules, live agents, or a monolithic end-to-end suite — with the bounded hermetic workspace suite at the handoff gate as the backstop. Breaking a public seam or adding any cross-module dependency is an ADR-level event. Dependency direction is strictly acyclic with no lateral dependencies among the mid-tier modules; only core is shared.

I15. **Core is minimal by rule, not by taste.** A definition enters `abacus-core` only if it is a domain invariant or vocabulary genuinely shared by at least two modules, pure, and stable. Anything used by one module lives in that module. Core fan-out on change is legitimate precisely because core membership is restricted — the moment convenience code lands in core, every change to it taxes the whole system.

I16. **Topology is configuration.** Core knows only the two authority classes; no named agent, profile identity, or singleton-orchestrator assumption appears in core or any module. Named agents are profiles — an authority class plus explicit capabilities and responsibility scopes/routes — defined solely by role-card frontmatter, with repository config referencing cards (no second profile schema). Profile load/activation is an audited Ledger event. Redistributing existing responsibilities between profiles is a card/config-only change with no code or test fan-out. Genuinely new behavior (e.g., a watchdog's new check) lands as a C0/C1 change in its owning module plus direct consumers (§7).

I17. **Decisions are fenced to an actor and scope, and audited.** Every assignment, acceptance, reclamation, and automated action records its concrete decision actor — ActorId, authority class, profile name, and the profile content hash in force — and the scope it acted under. Overlap is resolved at two layers: config validation **rejects** overlapping exclusive mutation/decision scopes up front; shared read/observation scopes may overlap freely; and Scribe authorization/serialization at write time remains the backstop — the losing actor receives a distinct refusal, never last-writer-wins. Automated actions carry the same fencing and appear in the audit stream like any other decision.

I18. **Attempt churn is explicit and visible.** ABACUS never creates a retry Attempt automatically. Every retry is a fenced action by the Assignment's authorized decision actor, and every Attempt plus every Submission refusal is audited and operator-visible. v1 imposes no hidden global retry loop or arbitrary universal cap; an Assignment's authored policy may set a maximum, and without one Attempts are unbounded only through repeated explicit decisions.

I19. **Coordination is typed, subject-bound, and ack-free.** Durable communication exists only as Signals: typed records (Directive, Report, Request) with a fenced sender and a required subject reference. No read/ack state exists anywhere — resolution is the linked responding workflow action, and "unresolved" is a derived query, exactly like pending applications (I10). Routing doctrine: progress is *queried*, never messaged; a work-shaped blocker becomes a dependency edge in the work graph; a decision-shaped ask becomes a Request; everything else is transient chatter on Herdr (I6). The mail boundary is explicit and checkable: the day ABACUS needs untyped subject-free messages, per-message read/ack state, or escalation-on-silence machinery, that is mail — stop and write its ADR rather than growing Signals into it.

## 5. Agent-state model

Three observation streams, three distinct authorities, one composer:

1. **Assignment state** — Ledger. What *should* be happening: which agent holds which bead under which lease.
2. **Process liveness** — `abacus-runtime` observations from the runtime substrate: the session/process is running, exited, or unreachable. These may be screen-manifest-derived (as Herdr's are) and are always non-authoritative.
3. **Semantic phase** — the agent's own self-reports through facade calls (claimed, verifying, handing off). This is the *only* way semantic phase enters the system. An agent that says nothing has no semantic phase.

Composition is a pure function in `abacus-core`: `(assignment, liveness, phase, staleness) → composed state`. Rules:

- Contradictory or stale observations compose to **UNKNOWN**, never to an inferred idle or done.
- Nothing is dispatched to an UNKNOWN agent.
- An UNKNOWN agent's assignment is recoverable only through lease expiry followed by explicit reclamation by an orchestrator-class decision actor authorized for the scope; the worker's partial product is preserved.
- Runtime-supplied observations — including screen-manifest-derived ones — are advisory. They may inform liveness and UNKNOWN-composition, but they can never be the *sole* basis for treating an agent as idle or dispatchable, never override a facade self-report, and never produce a completion. (The legacy failure was screen content as *authority*; as a demoted advisory input it is acceptable.)

## 6. Failure semantics

Required behavior on each failure. "Loud" means: distinct machine-readable error, surfaced to the caller and recorded in the Ledger when the Ledger is reachable.

| Condition | Detected by | Required behavior |
|---|---|---|
| Scribe unavailable | Facade call fails to connect | Loud failure with a distinct code. Workers halt and preserve their worktree; nothing buffers locally or retries silently. |
| Lost or undelivered live prompt | Herdr delivery report, or not detected at all | Correctness unaffected by design (I6): critical instructions are durable in Assignments/Envelopes/Signals; an agent re-derives its obligations from Scribe state when it next acts. No retry machinery exists or compensates. |
| Signal unresolved while its recipient is inactive | Derived unresolved-set query (I19) | No escalation machinery. The recipient's next facade activation surfaces its unresolved set (role cards make this the session-start move); the operator's status view shows the global unresolved set; a stalled worker remains bounded by lease expiry. |
| Stale lease | Lease token rejected on a facade call | The call fails; the worker stops mutating and reports. The Attempt becomes reclaimable — reclamation is an explicit, fenced action by an orchestrator-class decision actor (I17). |
| Worker death | Runtime liveness observation | State composes per §5. The lease runs to expiry; no auto-respawn in the initial system. |
| Malformed provider output | Adapter validation | The entire response is rejected — no partial ingest. Diagnostics carry the raw output reference for the operator. |
| Advisor unavailable / invalid | Advisor port | Deterministic fallback ordering; the degradation is noted in output; never blocks or errors the caller. |
| Dirty Handoff candidate | Submission precondition (uncommitted/untracked changes in scope) | Submission refusal with the dirt enumerated: Scribe audits the refusal, no Handoff or decision is recorded, and the Attempt stays active. If a recorded Handoff later violates the policy at decision time, the orchestrator records an explicit Rejection; that Attempt ends and only an explicit fenced actor action may retry the Assignment. |
| Missing or mismatched evidence | Submission/decision precondition | Audited Submission refusal before a Handoff exists, or explicit Rejection of an already recorded Handoff, with a distinct reason: no evidence, evidence bound to a different commit, or failing outcome. Rejection ends the Attempt, not the Assignment. |
| Out-of-band `br` mutation | Core use case correlating revisioned `abacus-work` observation with Ledger decisions during ordinary `ready`, `show`, acceptance-validation, or `doctor` reads | Refuse affected dispatch/status application, surface the exact anomaly to the operator, and require explicit reconciliation. Never silently adopt or revert the provider state; no watcher is added. |
| Decision outside actor's scope, or conflicting concurrent claims | Ledger authorization/serialization at write time | Distinct refusal to the unauthorized or losing actor; the attempt is audited; nothing partially applies. |
| Provider version mismatch | Adapter startup checksum/schema check | `br`/Herdr: fail closed — operations requiring the provider refuse. `bv`: degrade per I8. |
| Ledger unreadable/corrupt | Scribe startup or transaction failure | Fail closed. Refuse all writes, surface to operator. Never auto-repair or rebuild silently. |

## 7. Change classes and testing doctrine

Locality is preserved by module structure and ordinary Cargo test targets — not by a CI-efficiency subsystem. Legacy SABLE built one; it became a product of its own. ABACUS does not.

**Every change belongs to a class, and the class fixes what must run:**

| Class | Change | Edit loop must run | Gate |
|---|---|---|---|
| **C0** | Internal to one module; public seam and contracted behavior intended unchanged | That module's own tests | Ordinary review |
| **C1** | Additive, compatible seam extension | C0 set + direct consumers' contract checks | Ordinary review |
| **C2** | Breaking seam change, or any new cross-module dependency | Planned fan-out to affected consumers | **ADR before the change** |
| **C3** | `abacus-core` change | Full workspace fan-out | Legitimate only because core admission is restricted (I15) |

The worked example this table must keep true: an internal `abacus-work` change runs `abacus-work`'s own tests in the edit loop — not `abacus-state`, not `abacus-runtime`, not live agents, not an end-to-end rig.

- **The handoff gate is the backstop.** Acceptance of any Handoff runs the bounded, fully hermetic workspace suite (fakes and fixtures only — no live providers). This is what makes the cheap C0 edit loop safe: a change misclassified as C0 that in fact altered seam-visible behavior is caught at handoff, before acceptance — not by widening every edit loop.
- **Module-owned state and tests.** Each module owns its persistent state, its test suite, and its fixtures outright. There is no shared test harness; the only cross-module test artifacts are the contract checks a consumer owns against the seams it consumes.
- **Contract checks live at direct consumers.** A consumer pins exactly what it relies on from a seam — small, fast, compile-plus-behavior checks, exercised on C1+ changes and at the handoff gate.
- **Hermetic by default.** Module and contract tests run with fakes and captured fixtures — no live `br`, `bv`, Herdr, network, or real home directory. Injected environment (I13) makes this structural rather than disciplinary.
- **Provider fixtures are owned by the adapter module.** Live `br`/`bv`/Herdr compatibility tests run only on provider upgrades or scheduled/manual lanes; when a provider changes, the fixture diff in that one module is the drift report.
- **Test-budget growth is an ADR-level event.** If the default path stops being fast enough to run on every change, the response is a recorded architecture decision — split, delete, or reclass — never a caching/selection subsystem.

## 8. Non-goals

- Remote or multi-tenant operation; this is a local, single-repository system.
- Migration of legacy SABLE bead or state history; ABACUS starts clean.
- Push/PR/merge/deploy automation as part of the core loop.
- Universal TDD enforcement machinery; verification requirements are policy carried by beads and role cards, evidenced per I4.
- A custom CI-efficiency subsystem of any kind — locality comes from structure (§7), not machinery.
- More than two authority classes; specialists and future named managers or watchdogs ship as skills and configured profiles (I16), never as new classes.
- Compatibility layers for problems that have not yet occurred.
