# ABACUS

**Beads that compute.** ABACUS is a small, local orchestration system for turning a dependency-aware work graph into verified agent execution.

ABACUS is being built beside legacy SABLE, not as a rename or in-place rewrite. It extracts the durable product idea—role-guided, bead-led execution—and leaves behind infrastructure that made small changes expensive and system behavior difficult to reason about.

> **Status:** architecture and contracts are being established. No production binary exists yet.

## What ABACUS is

ABACUS has two stable authority classes:

- An **orchestrator** may select and assign ready work, validate handoffs, observe execution, or advance the work graph according to explicit capabilities.
- A **worker** executes one assignment in an isolated worktree and returns a verified local commit plus structured evidence.

Those classes are not two hard-coded named agents. Named managers, reviewers, and watchdogs are profiles composed from capabilities and responsibility scopes. Moving an existing responsibility between profiles should normally be a configuration and role-card change, not a core rewrite.

The first useful loop is deliberately narrow:

```text
ready bead -> assignment -> worker -> verified commit -> accepted handoff -> closed bead
```

Pushes, pull requests, merges, deployments, CI optimization, and universal TDD enforcement are separate concerns. They are not part of the definition of completed ABACUS work.

## Design posture

ABACUS is designed around four constraints:

1. **The work graph leads.** Beads hold planned work and dependencies; runtime panes do not become the source of truth.
2. **Infrastructure is replaceable.** `br`, `bv`, Herdr, SQLite, Git, and agent providers meet ABACUS at narrow, normalized seams.
3. **ABACUS will change.** Each module hides substantial behavior behind a small, stable interface and owns its implementation, fixtures, and tests. Internal changes should remain internal.
4. **The blast radius matches the interface change.** Module dependencies are acyclic, provider types do not leak, and tests follow direct dependencies. Full-system and live-provider validation is reserved for changes that actually cross those seams.

Host paths and installed tools are still discovered once at startup and passed explicitly, but host abstraction is supporting hygiene—not the main modularity strategy.

## Modules

Each capability is a separate top-level Rust crate so it can be maintained and tested with a local blast radius and eventually extracted independently. The initial repository uses one workspace version; independent release versions wait for demonstrated cadence differences.

| Module | Responsibility |
| --- | --- |
| [`abacus-core`](abacus-core/README.md) | Provider-neutral domain types, invariants, transitions, and use-case ports |
| [`abacus-state`](abacus-state/README.md) | Scribe, the local service that owns Ledger writes for assignments, leases, typed coordination Signals, evidence, decisions, and audit history |
| [`abacus-work`](abacus-work/README.md) | Normalized work-graph interface, `br` mutation adapter, optional `bv` advice |
| [`abacus-runtime`](abacus-runtime/README.md) | Normalized agent runtime interface and Herdr adapter |
| [`abacus-cli`](abacus-cli/README.md) | `abacus`/`abx` commands, configuration, host discovery, and dependency composition |

The dependency direction is intentionally one-way:

```text
authored roles / orchestration / skills
                 |
                 v
             abacus-cli                 composition root
          /       |       \
         v        v        v
 abacus-work  abacus-state  abacus-runtime      adapters
          \       |       /
                  v
             abacus-core                domain
```

Sibling adapter modules do not depend on one another. `abacus-cli` wires them together through interfaces owned by the use cases that consume them.

## Initial providers

- [`beads_rust`](https://github.com/Dicklesworthstone/beads_rust) (`br`) supplies the mutable dependency-aware work graph. Its SQLite + JSONL, explicit-git design is a better fit than SABLE's Dolt-backed `bd` machinery.
- [`beads_viewer`](https://github.com/Dicklesworthstone/beads_viewer) (`bv`) may advise prioritization using graph analysis. Advice is optional, time-bounded, and never authoritative.
- [Herdr](https://github.com/herdrdev/herdr) supplies persistent terminal and agent runtime mechanics through its CLI/socket interface. ABACUS does not treat pane state as domain completion.

ABACUS starts with pinned upstream releases, not forks. A fork becomes reasonable only after a concrete, recurring requirement cannot be expressed through the adapter seam.

## State ownership

There are three intentionally distinct kinds of state:

| State | Owner |
| --- | --- |
| Work items, dependencies, priority, ready/closed status | `br`, accessed through `abacus-work` |
| Assignments, attempts, leases, sanitized Envelope snapshots, typed Signals (Directives, Reports, Requests), evidence, Handoff decisions/application attempts and receipts, audit events | ABACUS, persisted in the Ledger through Scribe |
| Processes, panes, sessions, terminal output, observed agent status | Herdr, accessed through `abacus-runtime` |

Herdr owns live agent prompting and messaging. Workflow-critical facts now include immutable, subject-bound Signals in the Ledger: Directives bind worker direction, Reports record worker progress/blockers, and Requests carry orchestrator-to-orchestrator decision asks. Every fenced worker response mechanically surfaces the Attempt's current binding Directives through the Scribe protocol. Herdr carries only transient conversation or a best-effort content-free doorbell after a Signal is durable; Signal bodies never ride prompts. Unresolved work derives from immutable call ordering and missing typed responding actions, never an inbox, `read_at`, per-Directive acknowledgement, delivery retry, or escalation-on-silence state.

ABACUS durable state lives at `<git-common-dir>/abacus/state.sqlite3` in WAL mode. Clients reach it through `$XDG_RUNTIME_DIR/abacus/<repo-id>.sock`; agents do not need direct write access to `.git`.

Repository configuration lives at `.abacus/config.toml`, and environment overrides use the `ABACUS_*` namespace. New ABACUS paths must not use SABLE names.

## Repository map

```text
.
├── CONTEXT.md                  domain language and invariants
├── CODEX-HANDOFF.md            recovery context for the initial build
├── docs/
│   ├── architecture.md         system design and execution flows
│   ├── migration.md            surgical extraction from legacy SABLE
│   ├── compatibility/          pinned provider-spike evidence
│   └── adr/                    durable architecture decisions
├── abacus-core/README.md       core module contract
├── abacus-state/README.md      state module contract
├── abacus-work/README.md       work module contract
├── abacus-runtime/README.md    runtime module contract
└── abacus-cli/README.md        CLI/composition contract
```

Authored role cards, orchestration instructions, and selected skills will be added after these contracts survive adversarial review. They will use only the ABACUS interface and domain vocabulary.

## Install and initialize

ABACUS is installed once, then initialized explicitly in each Git repository.

During source development, the intended shape is:

```bash
git clone <repo-url> abacus
cd abacus
cargo install --path abacus-cli

cd ~/projects/my-app
abacus init
```

The repository does not contain buildable crates yet; this is the agreed onboarding contract for implementation. Releases may provide signed/prebuilt binaries, but they preserve the same `install once -> abacus init per project` model.

`abacus init` detects the Git root, remote when present, and likely base branch, then previews the configuration before writing. It creates repository-local `.abacus/` configuration and starter role cards, initializes a fresh `ABACUS-` work graph, asks Scribe to create local repository identity/Ledger state, and runs compatibility diagnostics.

Initialization is idempotent and works without a remote when a base branch can be selected explicitly. It never installs global hooks, edits Claude/Codex homes, commits, pushes, or imports legacy SABLE state.

## Testing model

The default feedback loop must remain fast as ABACUS evolves:

- Pure domain tests exercise `abacus-core` without files, processes, time, or network access.
- Adapter contract tests use fake process/socket implementations and checked-in provider fixtures.
- State tests use temporary repositories and temporary SQLite databases.
- Cross-module acceptance tests compose fakes; they do not launch real agents.
- Live `br`, `bv`, and Herdr smoke tests are opt-in compatibility lanes run when a provider pin changes and on scheduled/manual validation.

A normal internal change should run the owning module's tests. A change to its public interface additionally runs direct-consumer contract/composition tests. Only a truly shared domain change should fan out across the workspace, which is why `abacus-core` must stay small and stable. The complete hermetic workspace suite must remain small enough to run at every verified handoff; live integrations are separate. Growth that breaks the documented budgets requires an ADR rather than another layer of test-selection machinery.

See [Architecture](docs/architecture.md#test-architecture) for the detailed test tiers and change-locality rules.

## Non-goals for the first release

- Migrating legacy SABLE beads or execution history
- Maintaining compatibility with Dolt-based `bd`
- Recreating tmux pane scripts behind a new command name
- Installing global agent hooks
- A remote or multi-tenant Scribe service
- A generic mailbox, unread/acknowledgement state machine, or delivery-retry system layered over Herdr
- Automatic push, PR, merge, or deployment
- A mandatory testing methodology for downstream repositories
- A general CI optimization framework
- Forking all upstream providers preemptively

## Build sequence

1. Agree on domain language, invariants, seams, and migration exclusions.
2. Run bounded compatibility spikes against pinned `br`, `bv`, and Herdr versions.
3. Implement the pure domain and local state protocol.
4. Implement work and runtime adapters behind fixture-tested contracts.
5. Implement the install/init path and seed project-owned orchestrator/worker cards.
6. Compose one vertical slice through `abacus`.
7. Extract optional policy modules only when repeated use demonstrates a real seam.

The detailed staging and rollback strategy is in [Migration from SABLE](docs/migration.md).
