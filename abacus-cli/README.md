# `abacus-cli` module contract

Status: design contract; no Rust implementation yet

## Purpose

`abacus-cli` is the supported ABACUS product interface and the only composition root. It produces the `abacus` binary, optional `abx` alias, and Scribe launcher while keeping command parsing, host/configuration discovery, module construction, and output formatting out of domain and adapter modules.

It should remain thin. It invokes core use cases through deep module interfaces; it does not restate their rules.

## Owns

- `abacus` command tree and stable machine-readable output mode
- `abx` alias packaging/dispatch
- Repository root/Git common-dir discovery
- `.abacus/config.toml` and named-profile configuration loading
- `ABACUS_*` environment and command-flag precedence
- Provider lock loading and startup diagnostics
- Module-specific resolved configuration projections
- Composition of module-declared capability descriptors into one validation registry
- Construction/wiring of core, state, work, runtime, clock, ID, and commit-verifier adapters
- Scribe start/status/stop command integration
- Exit-code and normalized diagnostic presentation
- Explicit interactive confirmations for initialization/destructive recovery
- Agent-facing context-envelope rendering from core values

## Does not own

- Assignment or authorization rules
- SQLite queries/schema
- `br`, `bv`, or Herdr parsing/command behavior
- Named-agent behavior embedded in Rust branches
- Hidden automatic retries that change domain meaning
- Global hook installation or shell profile mutation
- Push, PR, merge, deploy, or downstream CI orchestration
- A second persistent state store

## Composition flow

Every command follows the same shape:

```text
parse request
  -> discover repository once
  -> resolve config/profile/provider lock once
  -> build module-specific inputs and adapters
  -> invoke one core/module use case
  -> render normalized result
```

Inner modules do not rediscover paths or environment. The CLI passes each module only its required projection rather than a mutable application-wide config object.

For worker startup, the composed use case renders one canonical sanitized Envelope, persists that exact snapshot/hash through the state interface, and then passes the same Envelope to the runtime interface for Herdr delivery. The CLI does not maintain a second copy or let persistence and delivery render independently.

For durable coordination, the composed use case first commits a typed Signal through Scribe and only then asks Herdr for a best-effort, content-free doorbell to the target Attempt/actor when a runtime exists. Signal bodies never ride Herdr prompts. A failed doorbell never rolls back or retries the Signal. Recipients query per-actor or global derived unresolved sets through the state facade. Every fenced worker response mechanically surfaces the Attempt's current binding Directives; the CLI cannot suppress them, manufacture read/ack state, submit a client-asserted Directive head, or bypass the state/core refusal that guards consequential actions.

The initial real Git commit verifier may be a private CLI/composition adapter satisfying the core-owned port. It returns commit/base identity, clean-tree facts, normalized changed paths for Assignment-scope conformance, and before/after workspace digests around verification commands. If it grows beyond a small argv-based implementation, extract it through an ADR rather than allowing CLI to become a Git module.

## Configuration

Required repository configuration root:

```text
.abacus/
├── config.toml
├── providers.lock.toml
└── roles/
    ├── orchestrator.md
    └── worker.md
```

Each role card is authored Markdown with machine-readable profile frontmatter. That frontmatter is the single profile definition: authority class, capabilities, scopes/routes, and stable name. `.abacus/config.toml` references cards and repository routing; it does not duplicate their capability schema.

`abacus init` seeds starter cards once. They immediately become project-owned authored files and are never regenerated over edits. Adding or splitting a manager/watchdog using existing capabilities means adding/editing cards and routing, not changing a Rust enum or command.

Configuration precedence:

1. safe compiled defaults;
2. repository config;
3. `ABACUS_*` environment overrides;
4. explicit flags.

Configuration is parsed into module-specific validated values. Work does not receive runtime config; state does not receive provider executable config; runtime does not receive work mutation config.

No correctness-critical configuration is read from SABLE paths, `~/.claude`, `~/.codex`, tmux options, or shell aliases.

## Profile composition

A profile specifies:

- stable profile name;
- authority class (`orchestrator` or `worker`);
- explicit capability list;
- responsibility scope/routing;
- provider kind and launch defaults where relevant;
- authored role/instruction references.

The CLI gathers namespaced capability descriptors from the owning modules, validates that every profile capability is known, and asks core to validate generic grant/scope semantics. It does not implement authorization itself. Adding a module-owned capability changes that module and a focused registry/composition check, not a core enum.

The content hash of the active card is part of actor registration and every authority snapshot. Loading a changed card records an audit event before it can authorize a new action; historical decisions retain their original hash.

Configuration validation rejects overlaps between exclusive mutation or decision scopes. Shared read/observation/alert scopes may overlap. Runtime authorization and Scribe serialization remain mandatory even after static validation.

Examples of topology evolution:

- move `state:decide_handoff` from `primary-manager` to `delivery-manager`: edit profiles and routing;
- add `watchdog` with `runtime:observe`, `state:read_audit`, and `runtime:prompt`: add a profile and role card;
- add automated `state:revoke_stale_attempt`: implement that new use case in core/state, then grant it explicitly.

No wildcard means a watchdog silently receives future capabilities.

## Project initialization

The source-development install and enrollment flow is:

```bash
git clone <repo-url> abacus
cd abacus
cargo install --path abacus-cli

cd ~/projects/my-app
abacus init
```

`abacus init`:

1. requires an existing Git repository and resolves its root/common directory;
2. detects configured remotes without contacting them;
3. proposes a base branch from explicit config, remote HEAD, or local branch evidence and asks when ambiguous;
4. previews every file/provider mutation before applying it;
5. writes `.abacus/config.toml`, provider lock, and one-time project-owned starter cards;
6. initializes a fresh `ABACUS-` work graph through the work interface;
7. starts/connects to Scribe to create the local repo ID/Ledger safely under the Git common directory;
8. runs `doctor` and prints exact next steps.

No remote is required; `--base <branch>` resolves an otherwise ambiguous local repository. Re-running is idempotent: it validates current state and offers explicit migrations/diffs rather than overwriting.

Initialization refuses to reuse legacy SABLE/Dolt state or clobber an incompatible existing `.beads` directory. It never installs global hooks, edits user agent homes, commits, pushes, or opens a PR.

## Planned command groups

Exact spelling remains subject to focused interface review, but the surface should group outcomes approximately as follows:

```text
abacus init
abacus doctor
abacus scribe <start|status|stop>

abacus work <ready|list|show|create|update|depend|close|reopen>
abacus assignment <create|show|list|sync|retry|revoke>
abacus signal <directive|report|request|list|unresolved>
abacus runtime <start|prompt|inspect|wait|read|stop>
abacus evidence <submit|show>
abacus handoff <submit|show|accept|reject|transfer>
abacus profile <list|show|validate>
abacus reconcile <assignment|all>
```

Authored agents normally receive smaller task-specific instructions, not the whole command catalog.

`abacus assignment retry` requires the Assignment's authorized decision actor and appends a new fenced Attempt; it never runs automatically or rewrites a rejected, expired, or revoked Attempt. `abacus handoff submit` returns and audits a Submission refusal without recording a Handoff when preconditions fail. `accept` and `reject` apply only to a recorded Handoff and produce immutable decisions.

`abacus signal directive` is restricted to the Assignment's exact decision authority and the `state:issue_directive` capability; its closed forms are amend, pause, abort, and answer. `abacus signal report` records structured progress or blocked-with-reason state from the current Attempt under its current lease token and `state:report`. `abacus signal request` carries an in-scope arbitration, authority-transfer, reconciliation, or other bounded decision ask under `state:request`; the responding command resolves it only by recording a linked fenced decision. Every form requires a typed bead/Assignment/Attempt/scope subject and rejects a subject-free body.

`abacus signal list` reads immutable records. `abacus signal unresolved --actor <id>` and its authorized global form derive Signals lacking their typed responding actions; neither command creates an inbox, `read_at`, per-Directive acknowledgement, or escalation state. `abacus assignment sync` is an orientation/latency convenience only. Correctness never depends on a worker remembering to call it because Scribe mechanically includes current binding Directives in every fenced response. A fenced response renders those Directives even when the requested Handoff or mutation is refused; amend/pause produce distinct Submission-refusal reasons at Handoff, and abort permits only abort-consistent mutations.

## Output and errors

- Human mode is concise and explanatory.
- Machine mode is versioned structured output on stdout with diagnostics on stderr.
- Exit codes distinguish usage, authorization, unavailable dependency, incompatibility, conflict/stale state, rejected policy, and internal failure.
- No consumer should parse colored/table human output.
- A command reports whether it made a durable change, made no change, or has an ambiguous provider outcome requiring reconciliation.
- Secrets and unbounded provider output are redacted/truncated.

## Scribe behavior

The CLI may start or connect to Scribe, but the state module owns `abacus-scribe` and its protocol.

- `abacus scribe start --foreground` runs Scribe explicitly for a supervisor or diagnosis without adding another command.
- Normal commands connect to the repository-specific socket.
- Automatic start, if later supported, must be visible, bounded, and race-safe.
- Scribe is not installed as a global background service or started for repositories without ABACUS config.

## Dependency rule

`abacus-cli` may depend on all four ABACUS modules because it is the composition root. No other module may depend on it.

Cross-module business flow belongs in core use cases; CLI only resolves and supplies the required interfaces. If command handlers begin encoding transition order or compensating actions, move that behavior behind the owning use-case interface.

## Evolution and blast radius

| Change | Expected validation |
| --- | --- |
| Formatting/help/internal command refactor | CLI tests |
| Add/move a profile using existing capabilities | Profile validation and authored-asset checks |
| Add a command exposing an existing use case | CLI tests only |
| Add a new use case/capability | Owning module/core tests plus focused CLI composition tests |
| Change an adapter's private implementation | No CLI tests unless its normalized interface/constructed config changed |
| Break machine output or config schema | Versioned migration/ADR plus direct consumers |

The CLI suite does not become a proxy full-system suite. Each handler is tested with fake module interfaces. A few critical hermetic journeys cover composition; live providers remain in their owning compatibility lanes.

## Test contract

Default tests cover:

- command parsing, help, and exit-code mapping;
- `init` dry-run, detection precedence, idempotency, edited-card preservation, no-remote repositories, and legacy-state refusal;
- config precedence and module-specific projections;
- repository and Git common-dir discovery using temporary repositories;
- provider-lock validation with fake executable identities;
- profile validation, capability redistribution, and scope errors;
- known-capability registry composition and unknown-capability refusal;
- machine-output schema and stderr separation;
- context-envelope rendering;
- Signal validation/subject routing, commit-before-content-free-doorbell ordering, per-actor/global derived unresolved presentation, and absence of client read/ack/head inputs;
- mechanical Directive surfacing on every fenced success/refusal response, amend/pause Handoff-refusal rendering, abort mutation refusal, and structured Report parsing;
- command handlers with fake core/state/work/runtime/commit interfaces;
- Scribe connection/start races through a fake state client;
- no implicit push, global hook, or user-home mutation.

Hermetic vertical journeys should remain few:

1. select/assign/start/submit/accept;
2. submit/reject/retry;
3. stale lease/runtime loss/reconcile.

These journeys use fakes and deterministic clocks. The live-agent release journey is separate.

Warm hermetic target: under fifteen seconds for CLI tests; complete T0–T2 workspace under ninety seconds on the baseline development machine.

## Acceptance criteria

- Every supported action is reachable through `abacus` without raw provider commands.
- `abx` behaves as an alias, not a divergent command implementation.
- Adding a named manager/watchdog with existing capabilities requires no Rust build.
- Operators and agents can issue/query typed Signals through the CLI, while Herdr doorbell failure leaves the durable derived-unresolved result intact and no Signal body enters a prompt.
- Module-specific configuration prevents unrelated settings from coupling adapters.
- Internal work/runtime/state changes do not require CLI tests when their public interface is unchanged.
- Machine output is versioned and human output is not a parsing contract.
- No command performs implicit publication or global installation.
