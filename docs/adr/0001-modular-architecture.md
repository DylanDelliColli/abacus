# ADR-0001: Modular architecture — five capability modules, provider adapters, one facade

- **Status:** Proposed (Claude-drafted from binding operator decisions in `CODEX-HANDOFF.md`; pending Codex cross-review and operator sign-off)
- **Date:** 2026-08-04
- **Decider:** operator (Dylan Delli Colli)
- **Companions:** `CONTEXT.md` (vocabulary, invariants, failure semantics — normative), `docs/architecture.md` (flows), `docs/migration.md` (legacy relationship)

## Context

ABACUS rebuilds legacy SABLE's orchestration core — role cards, orchestration policy, skills, bead-led execution — on replaceable infrastructure. The legacy audits (summarized in `CONTEXT.md` §1, with sources) showed that entangling the core with its infrastructure produced compensating machinery at every seam: a messaging stack that was mostly watchers, timers, and reconcilers around a small durable core; a 1,007-line terminal-scraping layer as the provider-drift surface; and evidence semantics that held only where they were bound to exact commit SHAs.

Constraints: Rust plus authored Markdown; local single-repository scope; Claude and Codex must both operate it; exactly two role types; completion is a verified clean-commit handoff, never publication. The dominant change pressure is ABACUS's own evolution — module implementations, adapters, and policy changing at different rates — with host and provider drift as a secondary, slower stream.

## Decision

### 1. Five capability modules in one repository, one workspace version

| Module | Owns | Explicitly excluded |
|---|---|---|
| `abacus-core` | Domain language and pure rules: identifiers, roles, assignments, attempts, evidence, lifecycle states, transition validation, agent-state composition, provider-neutral ports | Any I/O, persistence, subprocess, or provider knowledge. Core is deterministic and minimal — admission requires an invariant genuinely shared by ≥2 modules (CONTEXT I15); nothing speculative or single-consumer lives here |
| `abacus-state` | Scribe (internal name `abacus-scribe`), the SQLite (WAL) Ledger — transactional current-state tables plus append-only audit events and immutable Envelope/evidence/Handoff/decision/application records — schema/migrations, leases, assignment/attempt records, repo identity, local socket transport | Work-graph policy, runtime policy, orchestration decisions, messaging of any kind |
| `abacus-work` | The work-graph facade: `br` subprocess adapter, `ABACUS-` namespace validation, output normalization, decision-gated status application, out-of-band-mutation detection; the optional `bv` advisor port with deterministic fallback | Work status *authority* (that is `br`'s) and decision authority (that is the Ledger's); exposing raw provider output to any consumer |
| `abacus-runtime` | Agent/session lifecycle: spawn, inspect, signal, stop; the Herdr adapter; liveness observations; envelope and live-prompt delivery (Herdr owns all live agent messaging) | Semantic agent state (composed in core from observations); durable identities (runtime handles are ephemeral); durable instruction storage (Assignments/Envelopes live in the Ledger) |
| `abacus-cli` | The single command surface (`abacus`, alias `abx`), configuration loading, environment composition/injection, diagnostics | Business logic; direct provider invocation; any rule not delegated to a module |

All five live in one repository and version together. Independent versioning is deferred until a module *earns* extraction (criteria: Decision §9.5).

### 2. Strictly acyclic dependency direction

```text
authored roles / orchestration / skills
                 |
                 v
             abacus-cli
          /       |       \
         v        v        v
 abacus-work  abacus-state  abacus-runtime
          \       |       /
                  v
             abacus-core

External providers remain outside the domain boundary:
  abacus-work    -> br (mutating), bv (optional, read-only)
  abacus-runtime -> Herdr
  abacus-state   -> SQLite + local socket
```

The graph is acyclic and stays that way: the three mid-tier modules never depend on each other; only `abacus-cli` composes them. **Adding any new cross-module dependency is an ADR-level event.** Authored assets (role cards, skills) depend on the facade vocabulary only — a card that references `br`, `bv`, `herdr`, `tmux`, or legacy `bd` fails validation.

### 3. Provider seams, and the reason for each

Each seam exists for a named, evidenced reason — not for abstraction's sake:

- **`br` behind `abacus-work`, as a subprocess.** `br` is an actively developed upstream whose interface we do not control; the legacy system's direct coupling to its predecessor `bd` spread argv-parsing and CLI-gotcha workarounds through hooks and docs. The seam confines drift to one adapter with fixture-backed contract tests, enforces the single-write-path invariant (CONTEXT I2) and the decision-gated status sequence (CONTEXT §3, I3), and preserves the operator's option to substitute a fork or native store later without touching domain code. **v1 drives `br` as a subprocess via its documented machine interface** — process isolation contains upstream churn and avoids coupling our build to internal crate APIs; the seam keeps a library or native swap available later.
- **`bv` behind an advisor port.** Advisor output is ranking advice, not state. The seam guarantees `bv`'s absence or breakage cannot affect correctness (deterministic fallback, CONTEXT I8) and keeps upstream argv/schema out of role cards and prompts — where version drift is least testable and was, in legacy, effectively permanent.
- **Herdr behind `abacus-runtime`.** Herdr owns session/process mechanics **and all live agent messaging/prompt delivery**; its observations (which may be screen-manifest-derived) enter composition strictly as non-authoritative liveness/advisory signals, while semantic phase arrives only by facade self-report (CONTEXT §5). Prompts are transient transport: nothing critical exists only in a prompt — critical instructions are durable in Assignments and Envelopes (CONTEXT I6), so v1 needs no durable mail, acknowledgement, or delivery-retry machinery. Runtime handles never become durable identities. The seam keeps a replacement runtime (e.g., headless provider invocation) a one-module swap, specified against the same contract tests.
- **SQLite + socket behind `abacus-state`.** Agent sandboxes may permit worktree edits while denying direct `.git` writes — so agents reach durable state through Scribe over a socket rather than writing files under `.git`. WAL mode gives concurrent local readers; the git-common-dir location makes all worktrees of a repo share one Ledger. Scribe (internal process name `abacus-scribe`; operated via `abacus scribe status|start|stop`) is deliberately a *recorder of durable workflow facts* — assignments, attempts, leases, sanitized Envelope snapshots, evidence, Handoffs, decisions, work-status application attempts/receipts, audit, repository identity — and nothing else: not a manager, messenger, watcher, scheduler, retry loop, or policy engine. It is stateless across restarts (all durable state is in SQLite) and its scope is constitutionally capped to keep it from becoming the resident-machinery accretion point the legacy system suffered (CONTEXT I12). **Repo identity** is a local UUID minted by Scribe on first start and stored under `<git-common-dir>/abacus/` — never in tracked config, so independent clones of the same repository cannot collide on identity or socket paths, and directory renames (the failure that killed the predecessor session) are harmless.

Acceptance across the Ledger and `br` is one explicit saga, not two competing decisions: Scribe first commits the immutable Acceptance decision/operation identity and terminal `accepted` transition; `abacus-work` then closes the bead; Scribe finally records the application receipt. The first record authorizes the provider mutation; the last only confirms its projection. Accepted decisions lacking a successful receipt form a derived, lazily reconciled set—there is no `accepting` state, mutable pending queue, or background worker.

### 4. Change isolation: deep seams, classed blast radius

The primary change pressure on ABACUS is ABACUS itself evolving. The module structure exists so a well-placed change stays small:

- **Deep, stable module interfaces.** Each module exposes a small public seam over a substantial implementation. Stability is ranked: `abacus-core` most stable, provider adapters least. Seam changes are deliberate events, never refactoring side effects.
- **Module-owned state and tests; no lateral dependencies.** The mid-tier modules (`work`, `state`, `runtime`) never depend on each other and each owns its persistent state, test suite, and fixtures outright.
- **Explicit blast-radius classes** (normative table in CONTEXT §7): **C0** internal change → the owning module's tests only; **C1** additive seam extension → plus direct consumers' contract checks; **C2** breaking seam change or any new cross-module dependency → ADR-gated before the change; **C3** core change → full workspace fan-out, legitimate only because core admission is restricted to truly shared domain invariants (CONTEXT I15). The binding worked example: an internal `abacus-work` change runs `abacus-work`'s tests in the edit loop — not `abacus-state`, not `abacus-runtime`, not live agents, not an end-to-end rig.
- **The handoff gate backstops classification.** A bounded, fully hermetic workspace suite runs at verified handoff, before acceptance. This is what keeps the module-only C0 loop honest: a misclassified C0 that altered seam-visible behavior fails at the gate, not in production — and the edit loop never widens to compensate.
- **Contract checks live at direct consumers.** Each consumer pins exactly what it relies on from a seam with small compile-plus-behavior checks, exercised on C1+ changes and at the handoff gate.
- **Core is minimal by rule.** Admission requires: domain invariant or vocabulary shared by at least two modules, pure, stable. Single-consumer code lives in its consumer. This is the anti-dumping-ground rule that makes C3 fan-out acceptable.
- **Environment injection as adapter hygiene.** Modules receive paths, clocks, sockets, and provider binary locations from their composer rather than reading ambient state (CONTEXT I13). Retained because it makes hermetic, module-local testing structural — host mutability is a secondary concern, not the design center.
- **Hermetic by default; live lanes exceptional.** Module and contract tests use fakes and captured fixtures; live `br`/`bv`/Herdr compatibility tests run only on provider upgrades or scheduled/manual lanes.
- **No custom CI-efficiency subsystem.** Locality comes from module structure and ordinary Cargo test targets. Test-budget growth is answered by a recorded architecture decision, not by selection/caching machinery (CONTEXT §7).

### 5. Upstream handling

Pinned and checksummed versions of `br`, `bv`, and Herdr; checked at adapter startup; mismatch fails closed (advisor degrades instead, per CONTEXT I9). No forks initially — the seams preserve fork-substitution as a later option justified by concrete extension needs, not taken preemptively.

### 6. Authored orchestration assets

Role cards, orchestration instructions, and skills are authored Markdown with machine-readable frontmatter, validated by core — never generated from templates. The legacy system's generated/dual-path card installation was a recurring drift and incident source; authored-and-validated inverts that: the file in the repo *is* the artifact, and validation (frontmatter schema, vocabulary lint, privilege checks) is mechanical.

### 7. Topology as configured profiles

Legacy SABLE hard-coded named agents into code: hooks keyed privileges on literal names (`lincoln`, `chuck`), spawners enumerated valid roles, and libraries carried per-name behavior sets — so every topology change was a code-and-test sweep, and privileges drifted per name. ABACUS inverts this:

- Core defines exactly two authority classes (`orchestrator`, `worker`) and a **profile schema**: explicit capabilities and responsibility scopes/routes composed onto a class.
- Named agents — including any future manager or dedicated watchdog — are profiles **defined solely by role-card frontmatter**; repository config references cards and introduces no second profile schema. Profile load/activation is an audited Ledger event. No module code names an agent or assumes a singleton orchestrator. A watchdog is a spawned profile, never another daemon (CONTEXT I12).
- **Redistributing existing responsibilities between profiles is a card/config-only change.** Genuinely new behavior is a C0/C1 change in its owning module plus direct consumers.
- Every assignment, acceptance, reclamation, and automated action records its concrete **decision actor** — ActorId, authority class, profile name, and profile content hash in force — and its scope. **Scope overlap is resolved now, at two layers:** config validation rejects overlapping exclusive mutation/decision scopes; shared read/observation scopes may overlap; Scribe authorization/serialization at write time remains the backstop, and each assignment names its exact decision actor (CONTEXT I17).

Cost-of-change target this decision must keep true: adding a named manager or watchdog a month from now is a new profile plus at most one owning-module change — never a workspace fan-out.

### 8. Typed Signals, not mail

Mid-execution coordination — orchestrator→worker amendments and answers, worker→orchestrator progress and blockers, orchestrator↔orchestrator decision asks — is durable but deliberately not a messaging subsystem. One record family, **Signals**, with exactly three types: **Directive** (orchestrator→attempt, binding), **Report** (worker→decision actor), **Request** (actor→actor). Properties: immutable, sender-fenced with the full decision-actor identity, and a **required subject reference** (bead, assignment, attempt, or scope). No read/ack state exists; an unresolved Signal is a derived query — a Signal lacking its linked responding action — the same pattern as accepted-decisions-lacking-receipts. Herdr remains the content-free doorbell; a recipient discovers its unresolved set on its next facade activation (role cards make this the session-start move), and stalled workers stay bounded by lease expiry.

Routing doctrine (CONTEXT I19) keeps Signal volume low enough that the ack-free model suffices: progress is queried, never messaged; work-shaped blockers become dependency edges in the work graph; decision-shaped asks become Requests; the rest is transient chatter.

Rationale: the no-mail decision (Alternatives) left critical *mid-flight* instructions with no durable home — the Envelope is minted at spawn — quietly pressuring critical content into prompts, which I6 defines as a defect. Signals close that gap using only machinery classes the design already has: immutable records, decision-actor fencing, derived sets, doorbells. Cost class: C1 — record types and resolution linkage in `abacus-core`, tables and derived queries in `abacus-state`, facade verbs, one architecture flow amendment. No new module, daemon, protocol, or inbox.

**Amendment (2026-08-05 — worker response-link carriage at the state seam).** Decision 8's linked responding action is carried by a `FencedAction`: the existing `FencedCall` identity plus an optional `responds_to: SignalId`. Only substantive worker actions — Report append, Evidence append, and Handoff submission — accept this wrapper. Lease renewal continues to accept the bare `FencedCall`; a response link on lease machinery is therefore structurally unrepresentable.

The optional link is part of the complete idempotent request identity. An exact replay is absorbed and returns the stored outcome without adding another ordered action; reuse of the same operation with a different link is a conflicting duplicate and fails loudly. A present link must name an already-committed Directive addressed to the same Attempt. An unknown target or a Directive for another Attempt is a typed refusal that commits nothing. Kind-specific discharge remains derived in core: carrying a link to a Pause or Abort records the input fact but does not make an otherwise inapplicable worker action discharge it.

Scribe commits the permitted worker action in Ledger order and derives the returned `FencedResponse` from post-commit state. Consequently, the response to the action that validly discharges an amend/answer Directive already excludes that Directive from `binding_directives`; no acknowledgement state or client-asserted head is introduced. This amendment completes the already-accepted Decision 8 rather than adding a new subsystem. Because it breaks the state seam and changes `abacus-core`, its implementation is C2/ADR-first and C3/full-workspace validation under CONTEXT §7.

**Amendment (2026-08-05 — response-bearing Abort gate outcomes).** A validly fenced Report or Evidence append can be refused by a binding Abort without becoming a protocol/authority failure. Each method therefore returns its own concrete in-band outcome — `Recorded` or `Refused { reason: AbortInForce }` — together with `FencedResponse`. The refusal is an audited, operation-owned domain outcome: it advances fenced call ordering, records neither payload nor `WorkerAction`, replays without duplication, and mechanically returns the causally current binding set and Ledger head. Pre-commit validation and authority failures remain outer `StateError`s and claim no operation.

One pure worker-append gate beside the Handoff gate owns this policy. It refuses only a binding Abort. Pause and Amend continue to permit honest Report and Evidence records; the Handoff gate remains stricter and continues to refuse Abort, Pause, and undischarged Amend. Lease renewal remains exempt because it is lease machinery, not a substantive response action: keeping the lease alive lets the worker discover the Abort in the renewal response and reach an abort-consistent terminal path instead of expiring while uninformed.

The carrying operation for that abort-consistent terminal worker path is explicitly deferred to the transactional-lifecycle scope of `abacus-9nh.10`; none of the four current fenced methods is silently treated as that terminal action. That bead must name the carrying operation and prove it records `TerminalAttemptAction { abort_consistent: true }` before Phase 2 closes. This is another C2/C3 completion of Decision 8, not a new subsystem or a new ADR.

### 9. Ratified extractions from the legacy improvement log (operator, 2026-08-04)

Five small mechanisms from legacy `sable-potential-improvements.md` are adopted by explicit operator ratification. Mechanics for 1–4 live in the module contracts; this section is the decision record:

1. **Bead-content-hash binding.** The Assignment records the content hash of the bead that authorized it; Acceptance rechecks it — "the bead closed is the bead planned." Beyond mid-flight drift detection, this hash is the deliberate foundation for future planning and backlog-freshness tooling (sable-plan-style planning flows; victor/sherlock-style sweep skills), all of which need a cheap, stable answer to "has this bead's contract changed since it was assessed."
2. **Before/after workspace digests** around every verification command — a verify step that mutates the tree it verifies becomes visible instead of laundering a fix into a green outcome.
3. **Acceptance-time edit-scope conformance.** The handoff commit's normalized changed paths are checked against the Assignment's edit scope at decision time — closing the legacy declared-vs-touched gap server-side, with no hooks.
4. **Curated close reasons** written to `br` on every close, keeping the closed graph legible to humans and `bv`.
5. **Module-extraction criteria.** Extraction or independent versioning of a module is *earned, never scheduled*: a module qualifies when it has zero sibling-module imports, a stable CLI/JSON contract, fully isolated tests, and versioned checksummed consumption. Until all four hold, it stays in the workspace (Decision §1).

### 10. Red-green evidence pairs: the default unsupervised acceptance form (operator, 2026-08-04)

A vacuous test — one that cannot fail — defeats outcome-bound evidence while satisfying any "run this suite" policy. The closure costs no new machinery: an acceptance policy may require a **red-green evidence pair** — the specified verification *failing* at the declared base commit (red) and *passing* at the handoff commit (green), both recorded through the standard evidence wrapper with commit bindings and workspace digests. A test that cannot fail cannot produce the red half. This is an acceptance-policy *form*, not a universal gate: assignments choose it per policy (downstream verification choice remains a non-goal), and the orchestrator role card makes it the **default for unsupervised autonomous runs**. Mechanics reuse existing classes — core policy vocabulary plus ordinary evidence records; no new record types, no coverage machinery.

**Overlay-capture refinement (operator, 2026-08-04, option a).** *Red* is satisfied only by an **assertion-level failure**: the verification ran to completion and asserted failure. Execution and collection errors are a distinct, honestly recorded outcome that never satisfies red and produces its own distinct policy refusal — otherwise a verification file that is *new* in the handoff commit would satisfy red by mere absence from the base checkout, laundering exactly the vacuous case the pair exists to catch. Because new verification files do not exist at the base commit, the wrapper captures red by **overlay**: an isolated checkout of the Assignment's declared-base implementation with only the policy-named verification files overlaid from the worker's current work. The Evidence record binds the declared base commit, the overlaid path set, and per-file content digests of the overlaid files, alongside the usual workspace digests and the honest outcome. At acceptance, the red half is valid only if those overlay digests match the same files in the Handoff commit — verification edited after red capture yields a distinct red-stale refusal and requires recapture. All of this extends the existing Evidence value with overlay metadata; still no new record class.

### 11. Planning machinery: deferred, with its hooks already in place

SABLE's complementary planning layer (test-contract interviews, backlog-freshness sweeps, audit-driven decomposition) will eventually have an ABACUS analog. It is deliberately **not designed now**: planning tooling consumes the facade and the bead corpus, so designing it before the loop exists would be speculative interface-making. What Phases 2–5 must guarantee — and already do — is that nothing forecloses it: bead-content hashes (§9.1) give sweeps a stable "has this contract changed since assessment" anchor; revisioned work reads and per-bead content hashes are in the work facade contract; evidence, decisions, and audit are queryable through Scribe. Future planning machinery arrives as authored skills or optional Phase 7 modules over those surfaces, and never as a gate inside the core loop. Anything found missing from those surfaces during planning-tool development is an ordinary C1 seam extension, not a redesign.

## Consequences

**Positive**

- A module-internal change's edit loop is the owning module's test suite alone; the bounded hermetic workspace suite at the handoff gate is the safety net that keeps that cheap loop honest.
- Topology changes are configuration: a new named manager or watchdog is a profile plus, at most, a C0/C1 change in one owning module; redistributing existing responsibilities is card/config-only.
- Mid-execution coordination is durable, typed, and subject-bound (Signals, Decision §8) with zero inbox/ack machinery — unresolved items are derived queries.
- Provider drift concentrates in two adapters and their fixtures; a TUI or CLI change upstream becomes a fixture diff, not a production incident.
- The domain model is testable without any infrastructure present.
- Replacing `br`, `bv`, or Herdr is a one-module change by construction, verified by reusing the same contract tests.
- The smallest useful loop (orchestrator → assignment → worker → evidence-bound handoff → acceptance) needs nothing outside this repository plus pinned binaries.

**Negative / accepted costs**

- **Facade indirection tax:** every work-graph operation crosses a normalization boundary. Accepted — the legacy alternative (raw provider output in prompts and hooks) is the documented failure.
- **Scribe is a new single point of failure.** Mitigated: fail-loud clients (CONTEXT §6), stateless restart from SQLite, and a constitutionally capped scope. It is the only ABACUS-owned resident process; Herdr remains the external persistent runtime provider.
- **Two stores exist** (`br`'s database and the Ledger). Accepted with a defined authority split: `br` owns content, dependencies, and work status; the Ledger owns assignment/attempt/lease/evidence/handoff/decision lifecycle; the facade applies status only after its authorizing decision has committed, and the adapter detects out-of-band mutations (CONTEXT §3, I3). This hazard is the price of using `br` at all; the alternative (native graph) is recorded below as the standing fallback.
- **Five modules before any code** risks speculative structure. Accepted as documentation-first boundary-setting, with a recorded merge trigger: if after the first vertical slice a module has a single consumer, no independent test surface, and no independent change cadence, it is folded — by ADR, not silently.
- **Contract-test fixture maintenance** is a standing cost on every provider upgrade. Accepted — the fixture diff is the drift report, and this cost was previously paid as incident archaeology.

## Alternatives considered

- **Continue on tmux.** Rejected: the 1,007-line pane-predicate layer and its incident history are the direct evidence that terminal scraping as a state channel does not converge.
- **Native work graph in `abacus-state` (no `br`).** Rejected for now: `br` provides graph semantics, a maintained classic-beads format, and `bv` compatibility for free. Standing fallback trigger: if upstream churn breaks the adapter contract repeatedly (order of twice a quarter), revisit — the facade makes this a swap, not a rewrite.
- **Roles/skills invoking provider CLIs directly.** Rejected: the legacy role cards did exactly this (13+ raw `bd` invocations per card), which made every provider change a fleet-wide prose-editing exercise and every card a hidden coupling surface.
- **Generated role cards.** Rejected: legacy's generation/install pipeline produced silent drift (shadowed cards, divergent install paths). Authored + validated replaces build-time generation with check-time enforcement.
- **A durable ABACUS-owned mail subsystem.** Earlier drafts made durable mail a core pillar, motivated by the legacy finding that 15% of closed beads were undelivered-message fallback exhaust. Rejected for v1 with a different answer to the same evidence: the legacy failure was *critical content on a lossy channel*, and ABACUS removes the criticality rather than hardening the channel — every critical instruction is durable in Assignments/Envelopes, so live prompts (Herdr-owned) can be transient without correctness loss (CONTEXT I6). Mid-flight coordination is covered by typed Signals (Decision §8). The mail boundary is three checkable conditions (CONTEXT I19): a need for untyped subject-free messages, per-message read/ack state, or escalation-on-silence machinery. Crossing any one of them means designing mail by its own ADR — using AgentMailMCP's metadata-addressing schema as the template, never its transport or security posture — rather than growing Signals into an inbox.
- **Name-keyed authority in code.** Rejected: the legacy pattern (`if agent == "lincoln"` in hooks, per-name role sets in libraries) is the documented reason topology changes were expensive and privileges drifted; profiles + Ledger-fenced decision actors replace it (Decision §7).
- **Forking `br`/`bv`/Herdr up front.** Rejected: maintenance cost before demonstrated need; pinning + seams preserve the option.
- **A single monolithic crate.** Rejected: it makes the change-locality and hermetic-testing requirements unenforceable by structure, leaving them as discipline — which the legacy record shows does not hold.

## Open questions (for cross-review and operator decision)

1. **Codex sandbox vs. the socket.** Confirm a sandboxed Codex agent can connect to `$XDG_RUNTIME_DIR/abacus/<repo-id>.sock` under `workspace-write`. If not, the Scribe-mediated model needs a permitted transport (spike gate below). *Status 2026-08-04: partially resolved — the Claude session sandbox passed all probes; the Codex default sandbox denied Unix sockets, and exact-grant permission-profile validation from a fresh Codex session remains open (`docs/compatibility/2026-08-04-scribe-socket.md`, bead `ABACUS-HPG.5`).*
2. **`ABACUS-` prefix support in `br`.** Confirm custom ID prefixes are first-class upstream before the namespace validation is specified. *Resolved 2026-08-04: supported with lowercase normalization; the work facade owns a lossless `ABACUS-`/`abacus-` ID seam (`docs/compatibility/2026-08-04-br-bv.md`).*
3. **Scope expression.** How responsibility scopes/routes are expressed and checked (labels, bead subtrees, explicit routes) — must be specified with the profile schema in `abacus-core` before `abacus-state` implements decision-actor authorization. (Overlap *policy* is decided: exclusive scopes are rejected at config validation; Scribe fencing is the backstop — Decision §7.) *Resolved by ADR-0002 (label-selector algebra over declared keys), Accepted 2026-08-04 after Codex C2 cross-review.*

### Spike gates (pre-implementation)

**Herdr** (verifying the liveness/advisory stream and mechanics, not semantic state — semantic phase arrives via the facade, CONTEXT §5): truthful atomic prompt delivery into a busy session; crash/exit event latency (`kill -9` never composes to idle); stable session key across pane/window moves; integration-install coexistence with existing hook files; schema/version pinning across one Herdr upgrade; sandboxed-agent socket/state access (question 1).
**`br`:** concurrent claim/close races (exactly one winner, no corruption); `bv` reads `br`'s store correctly at pinned versions; documented machine-interface stability across an upgrade, or a vendored-binary policy; out-of-band-mutation detectability (supports CONTEXT I3).
