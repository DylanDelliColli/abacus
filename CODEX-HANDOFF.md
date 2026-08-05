# ABACUS — Codex implementation handoff

> Current successor handoff: read the late-shift addendum in
> `SHIFT-REPORT-2026-08-05-CODEX.md` first.
> The older material below records the initial bootstrap and is historical.

Date: 2026-08-04  
Repository root: `/home/ddc/dev-environment/abacus`

## Read this first

This document is the durable context for a fresh Codex session. The previous session may not be resumable because this directory was renamed while that session was running.

Start the replacement Codex session **from `/home/ddc/dev-environment/abacus`**. Confirm that an ordinary `pwd`, without a manually supplied working-directory override, reports that exact path before doing any work.

The operator wants ABACUS implemented. The architecture documentation and first Codex–Claude adversarial review are now complete; a fresh session should preserve those decisions and begin with the bounded Phase 1 compatibility spikes, not redraft the system from memory. Do not act as a legacy SABLE agent and do not invoke SABLE's execution machinery merely because it is installed globally. Treat legacy SABLE as a system being audited and selectively replaced.

## Current state

- It is a Git repository on `main`, tracking `origin/main` at the operator's personal [`DylanDelliColli/abacus`](https://github.com/DylanDelliColli/abacus) remote. Phase 0 was committed and pushed as `4e4bb60` (`Phase 0: architecture documentation and module contracts`).
- The documentation pass exists: root purpose/context/handoff files, architecture/migration/ADR documents, one contract in each of the five module folders, and initial provider compatibility records.
- Repo-local Git identity is configured for Dylan Delli Colli. No implementation code or Cargo workspace exists yet.
- No upstream projects have been forked or vendored.
- Claude and Codex completed an adversarial documentation review in the ABACUS directory. The lifecycle, Scribe/Herdr ownership, typed Signal design, test-locality, and change-topology findings were reconciled before this update.
- Phase 1 has begun with bounded `br`/`bv` and Herdr mechanics spikes under `docs/compatibility/`. Remaining hard gates include Scribe socket access from a sandboxed agent, Herdr control access/live Claude+Codex prompting, destructive `br` sync fixtures, and the concrete scope-expression syntax.
- `br v0.1.45` supports a custom prefix but normalizes generated IDs to lowercase. The work seam therefore maps external `ABACUS-<suffix>` IDs losslessly to provider `abacus-<suffix>` IDs; raw provider IDs/commands never escape.

Do not assume that any earlier chat transcript will be available. This file is intended to be sufficient to restart the work.

## Product thesis

ABACUS is a clean successor to, and parallel build beside, legacy SABLE. It is not a wholesale rename of SABLE.

The useful center of SABLE is understood to be:

- role cards;
- orchestration policy and instructions;
- a small set of relevant agent skills; and
- bead-led execution semantics.

Legacy SABLE became ineffective because this product core was entangled with too much process and infrastructure: custom bead tooling, CI-efficiency machinery, rigid TDD machinery, and tmux-specific management. ABACUS should keep a deliberately small orchestration core and integrate replaceable infrastructure through narrow adapters.

The intended product model is “beads that compute”: work is represented by beads, orchestration advances that work, and local agents execute it. Infrastructure supports that loop; infrastructure must not become the product.

## Binding operator decisions

Preserve these unless the operator explicitly changes them:

1. **Name and namespace**
   - Product: `ABACUS`
   - Canonical CLI/binary prefix: `abacus`
   - Optional short alias: `abx`
   - Bead ID prefix: `ABACUS-`
   - Config and state namespaces: `.abacus`, `ABACUS_*`, and `abacus`—never new SABLE-branded roots
   - Legacy SABLE keeps its existing name. Do not mechanically rename it.

2. **Repository shape**
   - For now, every ABACUS module is a separate top-level folder inside this repository.
   - The implementation language is Rust, supplemented by authored Markdown for roles, skills, and orchestration policy.
   - Initial scope is a local, single-repository system used by Claude and Codex agents on behalf of local users. Do not design a remote multi-tenant platform.

3. **Capability modules**
   - `abacus-core`
   - `abacus-state`
   - `abacus-work`
   - `abacus-runtime`
   - `abacus-cli`

4. **External infrastructure**
   - Replace the Dolt-based/custom SABLE beads implementation with [`beads_rust`](https://github.com/Dicklesworthstone/beads_rust), referred to as `br`.
   - Use [`beads_viewer`](https://github.com/Dicklesworthstone/beads_viewer), referred to as `bv`, as an optional analysis/advisory tool alongside `br`.
   - Move from tmux-specific orchestration toward [Herdr](https://github.com/herdrdev/herdr) as the agent runtime/session substrate.
   - All three tools must sit behind ABACUS-owned facades. Domain behavior must not depend directly on their CLI output or internal data types.
   - Pin and checksum upstream versions. Do **not** begin with forks. Preserve adapter seams so an ABACUS-specific fork can be substituted later if concrete extension needs justify its maintenance cost.
   - The operator does not consider the current `br`/`bv` license question a blocker.

5. **Ownership boundaries**
   - `br` owns the mutable work/dependency graph.
   - Only the ABACUS work facade may mutate `br`; roles, skills, and other modules must not shell out to it directly.
   - `bv` is an optional advisor. Its output can inform prioritization but is not authoritative state and cannot be required for correctness.
   - Herdr owns runtime/session mechanics and all live agent messaging/prompt delivery, not ABACUS domain state.
   - ABACUS owns durable assignments, execution evidence, leases/workflow metadata, decisions, and audit history.
   - Do not build a generic inbox, acknowledgement, or delivery-retry system. Durable coordination uses immutable, subject-bound Signals: Directives (orchestrator→Attempt), Reports (worker progress/blocker state), and Requests (actor→actor decision asks). Herdr rings a best-effort content-free doorbell only after the Signal is durable.
   - The mail boundary has three checkable triggers: untyped subject-free messages, per-message read/ack state, or escalation-on-silence machinery. If any is needed, stop and write a mail ADR; do not grow Signals into an inbox. Signal bodies never ride Herdr prompts.

6. **Roles and completion semantics**
   - Begin with exactly two first-class role types: `orchestrator` and `worker`.
   - A worker completing an assignment produces a verified, clean local commit handoff with evidence.
   - An Assignment may have sequential Attempts. Audited Submission refusal leaves an Attempt active; explicit Rejection ends only that Attempt; only an authorized decision actor may explicitly retry by appending a new fenced Attempt.
   - Acceptance commits one immutable authorizing decision and terminal `accepted` state, then closes `br`, then records an application receipt. Decisions lacking a successful receipt are derived for explicit reconciliation; there is no `accepting` state or mutable queue.
   - A Report comes from the current Attempt under its lease; its exact decision actor may answer or amend direction with an immutable typed Directive. Orchestrator-to-orchestrator arbitration, transfer, and reconciliation asks use Requests, whose resolution is the linked fenced decision.
   - Every fenced worker response mechanically surfaces the Attempt's current binding Directives as a Scribe protocol property, never worker discipline. Handoff is refused while pause/amend remains undischarged; after abort, only abort-consistent mutations are allowed.
   - Exposure and discharge derive from immutable call ordering and responding workflow actions. No `read_at` columns, per-Directive acknowledgement state, or client-asserted Directive head exists.
   - Progress is queried from Ledger state, work-shaped blockers become `br` dependency edges, decision-shaped asks become Requests, and everything else is transient Herdr chatter. Unresolved Signals are derived from missing typed responding actions; no inbox, acknowledgement, retry, or escalation-on-silence machinery exists.
   - Assignments bind the authorizing bead-content hash. Acceptance also enforces edit-scope conformance and verification-command before/after workspace digests, and `br` closure uses a bounded curated reason.
   - An Assignment may select a red-green evidence-pair policy: ordinary wrapper-captured Evidence must show `assert-fail` against the declared-base implementation using only policy-named verification files overlaid from current work, then `pass` for the same verification set at the Handoff commit. The red record binds the declared base plus per-file overlay digests that must match those files in the Handoff commit; `execution-error` and stale overlays produce the distinct `red-errored` and `red-stale` refusals. Outcomes and before/after digests remain honest. This is never a universal gate or coverage threshold; the Phase 6 orchestrator role card—not Rust—defaults unsupervised autonomous runs to this form.
   - Completion does not inherently include push, pull request creation, merge, or deployment.
   - External publication is an explicit separate action.

7. **Scribe and the Ledger**
   - Use an ABACUS-owned local state service named **Scribe**, with internal process name `abacus-scribe` and operator commands `abacus scribe start|status|stop` (`start --foreground` when needed).
   - Durable state is SQLite in WAL mode at `<git-common-dir>/abacus/state.sqlite3`.
   - The repository config root is `.abacus/`, with primary configuration at `.abacus/config.toml`.
   - The local socket is `$XDG_RUNTIME_DIR/abacus/<repo-id>.sock`.
   - The environment namespace is `ABACUS_*`.
   - Agents communicate with Scribe rather than writing Ledger state directly under `.git`. This is important because agent sandboxes may allow worktree edits while denying direct `.git` writes.
   - Scribe is only a transactional state service and sole Ledger writer. It is not a manager, messenger, watcher, scheduler, retry loop, or policy engine.
   - Scribe persists the canonical sanitized Envelope snapshot before Herdr delivers it. Runtime observations enter the Ledger only as audit events explicitly reported by an actor; Scribe never polls providers.
   - Scribe is the only ABACUS-owned resident process. Herdr is the external persistent runtime provider.
   - Start clean. Do not migrate legacy SABLE bead/state history into ABACUS during the initial build.

## Documented module contracts

The first deliverable was documentation rather than a premature Cargo scaffold. The following boundaries are now recorded in the module READMEs and are constraints on implementation.

### `abacus-core`

Owns domain language and pure rules: identifiers, roles, assignments, typed Signals (Directives, Reports, Requests), evidence, lifecycle states, transition validation, and provider-neutral ports. It should be deterministic and have no knowledge of SQLite, subprocesses, `br`, `bv`, Herdr, GitHub, or terminal panes.

### `abacus-state`

Owns Scribe (`abacus-scribe`), SQLite persistence, schema/migrations, transactions, leases, sanitized Envelope snapshots, immutable typed Signals and derived unresolved queries, evidence/Handoff records, decisions/application receipts, assignment/Attempt records, audit events, repository identity, and local client/server transport. It implements core persistence ports without absorbing work-graph, mailbox, or runtime policy.

### `abacus-work`

Owns the provider-neutral work graph facade and the initial `br` adapter. It validates the `ABACUS-` namespace, normalizes provider output, controls every mutation, and degrades safely when optional `bv` advice is unavailable or invalid. No consumer should parse raw `br` or `bv` output.

### `abacus-runtime`

Owns provider-neutral agent/session lifecycle and the initial Herdr adapter: spawn, inspect, signal, stop, and collect runtime observations. Runtime handles are not durable ABACUS identities, and Herdr session/pane concepts must not leak into core policy.

### `abacus-cli`

Owns the human/agent command surface, configuration loading, diagnostics, and composition of the other modules. It is the supported facade (`abacus`, optionally `abx`) and should contain little business logic. Commands should call typed module interfaces rather than reimplementing rules or invoking providers ad hoc.

### Authored orchestration assets

Keep role cards, orchestration instructions, and selected skills visible as authored assets rather than embedding them in Rust source. Their exact directory names can be proposed in the docs, but they must depend on the ABACUS facade and vocabulary—not on raw `br`, `bv`, Herdr, tmux, Dolt, or legacy `bd` commands.

## Completed documentation pass

These documents were created and adversarially reviewed before production code:

- `README.md`: concise product purpose, status, repository map, and non-goals;
- `CONTEXT.md`: domain language, system boundaries, invariants, and why ABACUS exists;
- `docs/architecture.md`: components, dependency direction, state ownership, and primary execution flows;
- `docs/migration.md`: what is extracted from legacy SABLE, what is replaced, what is explicitly left behind, and staged rollout/rollback strategy;
- an ADR recording the foundational modular architecture and provider boundaries;
- a contract/readme inside each of the five module folders describing responsibilities, dependencies, public seams, and exclusions.

The documentation should make this dependency direction explicit:

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
  abacus-work    -> br, optional bv
  abacus-runtime -> Herdr
  abacus-state   -> SQLite + local socket
```

No infrastructure adapter may become an implicit second orchestration API.

## Execution flow that the architecture must support

The documentation should define a minimal end-to-end path without inventing elaborate process:

1. The orchestrator reads normalized ready work through `abacus-work`.
2. Optional `bv` analysis may advise ordering; deterministic fallback behavior remains available.
3. The orchestrator records an assignment through Scribe.
4. `abacus-runtime` asks Herdr to start a worker with an ABACUS-generated context envelope.
5. The worker operates in its assigned worktree and mutates workflow only through the ABACUS facade. Reports persist through Scribe; every fenced response mechanically surfaces current binding Directives; Herdr carries only live conversation/content-free doorbells; managers use Requests for decision-shaped cross-manager asks.
6. The worker runs policy-named verification through the standard wrapper, records honest outcomes plus before/after workspace digests, creates a clean local commit, and records structured evidence plus the commit identity. When the Assignment requires red-green evidence, the ordinary red record captures assertion-level failure by overlaying the policy-named verification files onto the declared-base implementation and records per-file digests that must later match the Handoff commit; the ordinary green record is bound to the Handoff commit. Execution errors never satisfy red, and acceptance refuses stale overlay digests.
7. The orchestrator rechecks the bead-content hash, current binding Directive constraints, evidence policy—including any required red-green pair—and edit scope, then advances the bead through `abacus-work` with a curated close reason.
8. Push/PR/merge/deploy remain separate, explicitly authorized workflows.

Specify failure behavior for at least: Scribe unavailable, stale lease, worker death, malformed provider output, optional advisor unavailable, dirty handoff, missing evidence, and provider version mismatch.

## What ABACUS should leave behind

Do not casually port these legacy SABLE subsystems into the core:

- Dolt-backed/custom `bd` implementation;
- tmux-specific pane/session assumptions;
- CI-efficiency machinery;
- universal or rigid TDD enforcement machinery;
- global shell hooks as hidden orchestration;
- push/PR/merge automation as the definition of worker completion;
- compatibility layers for historical state before a demonstrated need exists.

Useful policies may later become optional modules, skills, or SDK consumers. They should not be prerequisites for the first useful ABACUS loop.

## Adversarial review record

The operator explicitly asked Codex and the active Claude agent to act as adversarial reviewers before code is written.

The completed division of work was:

- Claude drafts/reviews `CONTEXT.md` and the foundational ADR.
- Codex drafts/reviews the root `README.md`, architecture and migration documents, and five module contracts.
- Each agent reviews the other’s work for hidden coupling, excess process, missing failure semantics, and accidental recreation of SABLE bloat.

Claude challenged the two-store Acceptance saga, Assignment-versus-Attempt rejection semantics, out-of-band `br` mutation ownership, Envelope persistence, potential watcher language, and the initial no-mail claim. The mail challenge exposed real gaps for durable mid-attempt direction and orchestrator-to-orchestrator decision asks. The approved resolution is one typed immutable Signal family—Directive, Report, Request—not a generic mailbox, with Herdr retained as transient conversation transport and doorbell. Progress remains queryable state; work blockers remain graph edges; unresolved coordination derives from linked workflow actions. Codex reconciled those findings in the normative context, ADR, architecture, and module contracts. Future architectural changes should repeat the same no-shared-file-at-once review discipline, but a fresh session need not recreate this first pass.

## Known session/hook failure

The directory was renamed from:

`/home/ddc/dev-environment/sable-v2`

to:

`/home/ddc/dev-environment/abacus`

while the earlier Codex session was live. Its thread metadata retained the removed old path as both `cwd` and workspace root. Commands succeeded only when given an explicit ABACUS workdir. A command using the implicit workdir reproduced an immediate process-spawn `ENOENT`.

This was the direct cause of the repeated failed PostToolUse hooks: the hook runner attempted to spawn from the nonexistent old directory before any hook script could execute.

There is a separate configuration concern: `/home/ddc/.codex/hooks.json` globally registered legacy SABLE PostToolUse hooks, including:

- `bead-quality.sh`;
- `multi-manager/post-push-merge-notify.sh`;
- `multi-manager/seat-sighting-gate.sh`.

A fresh session rooted correctly in ABACUS should remove the immediate `ENOENT`. It does not solve the broader leakage of legacy SABLE hooks into unrelated repositories. Diagnose and propose project scoping separately; do not mutate user-global hook configuration without explicit authorization. Do not recreate the old directory or add a compatibility symlink merely to hide the stale-session problem.

## Git and GitHub constraints

- Do not push anything merely because implementation work has begun.
- The existing remote is the operator's personal repository, **`DylanDelliColli/abacus`**; verify authentication still resolves to that identity before any later publication.
- Do not replace the remote with an organization or another identity.
- Preserve the repo-local identity and a clean, reviewable commit history.

## First-session checklist

1. Launch with `/home/ddc/dev-environment/abacus` as the real session/workspace root.
2. Run plain `pwd`; if it fails or reports `sable-v2`, restart rather than compensating with per-command workdir overrides.
3. Read this entire file.
4. Read `CONTEXT.md`, `docs/adr/0001-modular-architecture.md`, `docs/architecture.md`, and the contract for any module being touched.
5. Confirm the workspace still contains documentation only and preserve unrelated user changes.
6. Resolve the three ADR spike questions with bounded, disposable compatibility probes; do not turn exploratory scripts into permanent orchestration.
7. Record sanitized provider fixtures/compatibility evidence in the owning module only.
8. Re-run a Codex–Claude adversarial review if a spike changes a public seam or contradicts the accepted ADR.
9. Scaffold Rust only after the relevant spike gate passes, beginning with the smallest vertical slice described in `docs/migration.md`.
10. Do not push or otherwise publish without explicit authorization; when authorized, preserve the existing `DylanDelliColli/abacus` target.

## Review standard

At every design decision, ask:

- Is this part of bead-led orchestration, or is it optional infrastructure?
- Does ABACUS own this state, or should a replaceable provider own it?
- Can `br`, `bv`, or Herdr be replaced without rewriting the domain model or authored roles?
- Is this process required for the smallest useful local loop?
- Does this create a second, hidden API outside the `abacus` facade?
- Are failure, recovery, idempotency, and evidence semantics explicit?
- Are we extracting SABLE’s useful core—or rebuilding its bloat under a new name?

The preferred result is a small system with strong contracts, not a broad framework with speculative machinery.
