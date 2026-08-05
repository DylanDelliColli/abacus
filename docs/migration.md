# Migration from legacy SABLE to ABACUS

Status: staged clean-room extraction plan  
Last updated: 2026-08-04

## Objective

Build ABACUS beside legacy SABLE by extracting the small set of product ideas that remain valuable and replacing infrastructure with independently maintainable modules and upstream providers.

This is not an in-place refactor, mass rename, or compatibility release. Legacy SABLE remains available under its existing name while ABACUS proves a smaller execution loop.

## Why a clean extraction

Legacy SABLE demonstrates useful ideas:

- dependency-aware work as durable plan and recovery state;
- orchestrator and worker responsibilities;
- fresh-agent-quality work descriptions;
- isolated worker execution;
- durable coordination and evidence;
- explicit recovery from abandoned work.

Those ideas are currently entangled with a large Python/shell surface:

- many `sable-*` executables and support libraries in `bin/`;
- direct tmux calls, pane metadata, screen scraping, registries, and warm-role topology;
- Dolt-backed `bd` workflows and custom push/sync wrappers;
- global Claude/Codex hooks for bead quality, TDD, messaging, and merge behavior;
- CI gate classification, test-tier selection, cost profiles, promotion queues, and telemetry;
- role-specific merge/push discipline;
- broad test fixtures that couple small behavior changes to large validation runs.

The result is a poor change gradient: a local one-line implementation change can cross several implicit seams and acquire a long system-wide test obligation. Porting the tree would preserve that failure mode.

ABACUS therefore starts from contracts and imports behavior selectively. Nothing moves merely because it already exists.

## Extraction test

A SABLE concept belongs in ABACUS core only when all are true:

1. It is required for the minimal bead-to-verified-commit loop.
2. It is provider-neutral or can be isolated behind one narrow adapter.
3. Its state has one clear owner.
4. Its failure and recovery behavior can be stated without referring to a specific pane layout, hook, CI job, or Git remote.
5. Its implementation and tests can live within one module or one explicit cross-module use case.

Otherwise it is left behind, deferred as an optional module, or delegated to an upstream provider.

## Migration matrix

| Legacy capability | ABACUS treatment | Destination/reason |
| --- | --- | --- |
| Orchestrator and worker concepts | Extract and simplify | Authored role cards + `abacus-core`; exactly two first-class roles initially |
| Beads as durable plan | Preserve the method, replace implementation | `abacus-work` over `br` |
| Fresh Agent Test / actionable work descriptions | Preserve as authored policy | Orchestration instructions; validation may later live behind work interface |
| Dependency-aware ready work | Delegate | `br`, normalized by `abacus-work` |
| Graph centrality/critical-path planning | Add as optional advice | `bv` adapter; never authoritative |
| Assignment, lease, typed Signal (Directive/Report/Request), evidence, decision, and audit semantics | Reimplement minimally | `abacus-state`; Scribe is the sole Ledger writer |
| Live agent messaging, worker launch, persistence, prompt delivery, status observation | Delegate runtime mechanics | `abacus-runtime` over Herdr; Signals are durable first, then Herdr rings a best-effort doorbell; no generic inbox/ack/retry layer |
| tmux commands, pane options, screen regexes, warm-pane registry | Leave behind | Do not wrap tmux under ABACUS names |
| Dolt database and `sable-dolt-push` | Leave behind | Clean `br` SQLite + JSONL work graph; no history migration |
| Worker self-push and merge queue roles | Leave behind | Completion is a verified local commit; publication is separate |
| Global PostToolUse/PreToolUse hooks | Leave behind | No hidden global orchestration |
| Mandatory TDD gate and `[no-test]` escape machinery | Leave behind | Downstream projects choose their verification policy |
| CI preview/promote queues and merge latency optimization | Leave behind | A future optional module/SDK consumer only if independently valuable |
| Test-tier selection, cost profiles, contention benchmarks | Leave behind | Keep ABACUS suites small by module design, not another optimizer |
| Telemetry aggregation across GitHub, Git, beads, and panes | Defer | Add only from demonstrated operator questions and module-owned events |
| Discovery capture/review skills | Evaluate individually | Port authored reasoning only if it uses ABACUS vocabulary and facade |
| Audit/Columbo/Gaudi skills | Do not bulk-port | Review each skill for product value and external dependencies |
| Installer that mutates user-global Claude/Codex config | Leave behind | Repository-local, explicit initialization only |
| SABLE state, beads, and historical role registry | Do not migrate initially | Avoid compatibility complexity and ambiguous authority |

## Target repository shape

During the initial build, modules remain separate top-level folders in one repository:

```text
abacus/
├── abacus-core/
├── abacus-state/
├── abacus-work/
├── abacus-runtime/
├── abacus-cli/
├── roles/                 # added after contracts stabilize
├── orchestration/         # added after contracts stabilize
├── skills/                # selected, authored assets only
└── docs/
```

Each module owns its interface, implementation, fixtures, tests, and change log. No module reads another module's private fixtures or internal source. This shape allows a later repository split without first untangling lateral imports.

## Staged implementation

### Phase 0 — contracts and adversarial review

Deliver:

- product/context documents;
- foundational ADR;
- system architecture and migration plan;
- one contract per module;
- explicit change/blast-radius and test-tier rules.

Acceptance:

- Codex and Claude have cross-reviewed the documents;
- every durable fact has one owner;
- module dependencies are acyclic;
- migration exclusions are unambiguous;
- unresolved provider facts are listed as spikes rather than asserted.

Rollback: documentation only; revise or discard without runtime impact.

### Phase 1 — disposable compatibility spikes

Run bounded spikes in temporary repositories and redirected configuration roots.

`br` spike:

- pin a candidate release/binary checksum;
- initialize an `ABACUS-` prefix;
- exercise create/show/ready/update/dependency/close through JSON output;
- verify JSONL flush, locking, graph identity, and Git-worktree behavior;
- capture minimal sanitized fixtures.

`bv` spike:

- verify it reads the exact pinned `br` representation;
- exercise only robot/non-interactive modes;
- bind advice to graph/data hash;
- demonstrate deterministic fallback for absent, timed-out, partial, and malformed advice.

Herdr spike:

- pin a candidate release/binary checksum;
- launch disposable Claude and Codex sessions;
- verify prompt delivery, generation-fenced handles, wait/events, output reads, process exit, and restart recovery;
- validate the high-level CLI/JSON facade first; use the socket directly only if a measured gap remains;
- isolate the provider with an exact disposable named-session namespace and pre/post manifest when its state root cannot be redirected;
- confirm ABACUS role/assignment state remains outside Herdr.

Acceptance:

- fixtures and compatibility records are sufficient for hermetic adapter tests;
- no spike mutates live global Claude/Codex configuration;
- failures produce an adapter-local design change, not a new cross-module dependency.

Rollback: delete temporary roots and candidate binaries; no ABACUS durable state exists yet.

Initial bounded observations are recorded under [`docs/compatibility/`](compatibility/README.md). They resolve the basic `br`/`bv` shape and Herdr headless mechanics while leaving destructive sync fixtures, real-agent prompting, and sandbox access as explicit remaining gates.

### Phase 2 — `abacus-core` and `abacus-state`

Implement:

- minimal shared identities and lifecycle invariants;
- assignment/attempt separation, lease/fencing, sanitized Envelope snapshots, evidence, Handoff refusal/rejection, and Acceptance application rules;
- immutable subject-bound Signals (Directives, Reports, Requests), derived unresolved queries, and stale-Directive-head fencing;
- bead-content-hash binding, edit-scope conformance, and verification before/after workspace digests;
- versioned Scribe protocol;
- SQLite schema/migrations and `abacus-scribe` lifecycle;
- in-memory/fake state implementation for use-case tests.

Acceptance:

- core tests are pure and deterministic;
- state tests use only temporary directories/databases;
- Scribe restart, idempotent request retry, lease expiry, stale fencing, Acceptance interruption/reconciliation, and migration failure are covered;
- the client/server transport implements the ADR-0003 two-carriage design (credentialed actors, no protocol enrolment surface, relay framing) once its gate clears;
- no `br`, `bv`, Herdr, live agent, network, or user-home dependency exists;
- module and workspace hermetic budgets are recorded.

Rollback: stop Scribe and remove only the explicitly initialized `<git-common-dir>/abacus/` state for the disposable test repository.

### Phase 3 — `abacus-work`

Implement:

- deep normalized work interface;
- pinned `br` process adapter;
- optional pinned `bv` advisor;
- graph revision/hash binding;
- provider diagnostics and compatibility checks;
- fake provider implementation and fixture contract suite.

Acceptance:

- internal adapter changes run only work-module tests during the edit loop;
- raw provider types and errors do not appear in other modules;
- `bv` removal has no effect on correctness;
- worker role cannot close work through the supported interface;
- live compatibility is a separate explicit command.

Rollback: keep ABACUS state; disable work provider configuration. No legacy data is modified.

### Phase 4 — `abacus-runtime`

Implement:

- normalized launch/inspect/wait/read/signal/stop interface;
- pinned Herdr adapter;
- high-level CLI/JSON control with a generation-fenced session/pane/terminal handle;
- complete live prompt/doorbell delivery through Herdr, with no generic ABACUS inbox or acknowledgement protocol;
- explicit launch environment and working directory;
- runtime-handle recovery and uncertainty semantics;
- fake protocol peer and fixture contract suite.

Acceptance:

- runtime tests do not launch Herdr by default;
- Herdr status cannot accept a handoff;
- Herdr types do not leak into core, state, or authored roles;
- one explicit live compatibility lane exercises the pinned provider;
- a restored pane ID with a changed terminal/process generation becomes stale/unknown until explicitly re-associated;
- a runtime implementation change does not run work/state live tests.

Rollback: stop disposable Herdr sessions; durable assignments remain inspectable and recoverable.

### Phase 5 — onboarding, base profiles, and one CLI vertical slice

Implement the source install/enrollment contract:

```bash
git clone <repo-url> abacus
cd abacus
cargo install --path abacus-cli

cd <target-git-repository>
abacus init
```

`abacus init` detects Git root, optional remote, and base branch; previews all writes; seeds project-owned orchestrator/worker cards; initializes fresh providers/state; and remains idempotent. It refuses to reuse legacy state and performs no global installation or publication.

Then implement only the commands required for:

```text
doctor -> ready -> assign -> spawn -> signal/doorbell/sync -> evidence -> submit -> accept/reject -> close
```

Acceptance:

- a local repository with no remote can initialize with an explicit base branch;
- re-running initialization preserves edited role cards and produces no unintended changes;
- one disposable repository completes the full local loop;
- deterministic operation still works with `bv` disabled;
- worker death, Scribe restart, stale lease, dirty handoff, and provider mismatch have explicit recovery;
- no push, PR, merge, CI, or global hook occurs;
- the default T0–T2 workspace suite remains within budget.

Rollback: stop local processes and return to legacy SABLE for existing work. ABACUS and SABLE must never share mutable state stores.

### Phase 6 — authored profiles and orchestration expansion

Refine the seeded orchestrator/worker cards and add named profile cards or selected skills against the proven CLI interface.

Acceptance:

- no raw provider command appears in authored assets;
- each instruction corresponds to a supported, tested ABACUS outcome;
- roles add judgment and policy rather than duplicating CLI reference material;
- existing capabilities can move from one manager profile to another without Rust or database-schema changes;
- a read-only watchdog profile can observe and alert without receiving graph-mutation or handoff-decision authority;
- orchestrators query progress, encode work-shaped cross-scope blockers as dependency edges, and use typed Requests only for decision-shaped coordination;
- every role checks its derived unresolved Signal set on session start without creating an inbox/ack protocol;
- removing an optional skill does not break execution correctness.

Rollback: roles are Markdown assets; revert independently from modules.

### Phase 7 — optional extraction/SDK work

Only after repeated use:

- identify policies that have independent value (for example CI feedback or a testing-discipline skill);
- define their ABACUS-facing interface;
- place them in separate repositories/packages;
- consume them as optional SDK clients, not core dependencies.

Acceptance: ABACUS's minimal loop works identically when every optional extension is absent.

An extracted module is ready for a separate repository only when it has zero imports from lateral sibling modules, a stable versioned CLI/JSON or library contract, isolated hermetic tests, and a pinned/checksummed consumption path back into ABACUS. A folder boundary alone is not an extraction seam.

If publication is later added, prefer provider-native pull requests, branch protection, and merge queues consuming an already completed Handoff. Do not recreate SABLE's custom merge seat or make publication part of ABACUS completion.

## Change-locality migration rules

The migration must not reproduce SABLE's validation coupling.

1. Port behavior, never its entire historical test harness.
2. Place a migrated test with the module that owns the invariant it proves.
3. Rewrite provider-facing tests as small adapter fixtures; do not retain live shell orchestration in default tests.
4. Keep only a few hermetic vertical journeys. Do not translate every old integration test into a root end-to-end test.
5. Do not import SABLE helper libraries to make early tests pass.
6. Do not create a broad common fixture package.
7. Do not make one module's private behavior observable solely so another module can test it.
8. If a ported behavior requires tests in three unrelated modules, reconsider its owner or interface before proceeding.
9. Record a baseline for every module and reject unexplained test-runtime growth.
10. A breaking module interface or new lateral dependency requires an ADR.

## Parallel operation with legacy SABLE

ABACUS is opt-in per repository through `.abacus/config.toml`. Legacy SABLE markers, `.beads` state, hooks, and sessions do not activate ABACUS automatically.

During the proving period:

- use legacy SABLE only for repositories already governed by it;
- use ABACUS only in explicit disposable/pilot repositories;
- never point both systems at the same work graph or worker assignment;
- do not install ABACUS global hooks to emulate SABLE behavior;
- compare outcomes and operator effort, not feature-count parity.

The success criterion is that ABACUS performs the useful loop with less machinery and lower change cost—not that every SABLE command has an equivalent.

## Data policy

Initial ABACUS repositories start with a new `ABACUS-` graph and empty Ledger.

No initial migration of:

- SABLE/Dolt bead history;
- open/in-progress SABLE assignments;
- SABLE role registry or pane identity;
- messaging/reconciliation files;
- telemetry, test-cost profiles, or merge receipts;
- global hook state.

If historical import becomes valuable, design a read-only, one-shot importer later. It must produce provenance-tagged records and cannot create a permanently dual-written system.

## Upstream fork policy

Begin with unmodified, pinned upstream `br`, `bv`, and Herdr releases.

Consider an ABACUS-owned fork only when:

- a concrete required capability is absent upstream;
- the need has recurred or blocks the minimal loop;
- the adapter seam cannot express a safe workaround;
- the maintenance and security update burden is understood;
- the fork can remain compatible with the normalized ABACUS interface;
- an ADR compares fork, contribution, wrapper, and replacement options.

SABLE-specific branding or convenience is not sufficient reason to fork.

## Migration stop conditions

Pause and revisit the design if any phase causes:

- a provider type to appear in `abacus-core`;
- an adapter module to depend on another adapter module;
- live provider setup in `cargo test --workspace`;
- a module's internal refactor to require unrelated module fixtures;
- a global hook or user-home mutation for correctness;
- dual authority for a mutable fact;
- automatic push/merge becoming part of handoff acceptance;
- a compatibility shim larger than the behavior being extracted;
- a default hermetic suite exceeding its budget without an explicit review.

These are signs that ABACUS is rebuilding the coupling it exists to remove.

## Rollback principle

Every phase must be independently reversible. ABACUS does not modify legacy SABLE state, so rollback means stopping ABACUS processes and ceasing use of its explicitly scoped repository state—not reconstructing SABLE from converted data.

Do not create a `/home/ddc/dev-environment/sable-v2` compatibility path or symlink. The earlier live-session rename issue was solved by restarting sessions with the correct ABACUS root; retaining obsolete identity would conceal configuration leaks.
