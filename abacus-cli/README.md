# `abacus-cli` module contract

Status: target composition contract; no Rust implementation yet

## Purpose

`abacus-cli` produces the `abacus` binary and optional `abx` alias. It owns
command parsing, host/config discovery, provider construction, typed
convenience workflows, and output formatting.

It is a composition root, not an exclusive writer or security boundary.
Agents may use pinned stock `br` directly against the same shared store.
ABACUS commands exist to make record shapes and multi-provider journeys easier
and less error-prone, not to claim raw provider access is impossible.

## Owns

- command tree and stable machine-readable output;
- repository/config/provider-lock discovery;
- resolution and validation of the one absolute shared `BEADS_DIR`;
- module-specific configuration and process-runner construction;
- composition of `abacus-work`, `abacus-runtime`, Git verification, and core
  policy that survives the ADR-0006 necessity round;
- explicit provider diagnostics and dry-run/confirmation UX;
- project-owned role-card loading;
- exact-commit Evidence wrapper and Handoff rendering; and
- exit-code/error presentation.

It does not own:

- a second database, Scribe, socket, relay, state RPC, or daemon;
- provider-internal SQLite/JSONL schemas;
- capability/scope authentication or protection against same-user `br` calls;
- hidden retries, watchers, or recurring reconciliation;
- Herdr parsing/command behavior;
- Git staging, push, PR, merge, deploy, or Publication policy; or
- workflow record kinds not approved by the necessity round.

## Composition flow

```text
parse request
  -> discover repository/config once
  -> resolve pinned providers and absolute BEADS_DIR once
  -> construct module-specific inputs
  -> invoke one typed use case or provider operation
  -> render normalized result
```

Inner modules do not rediscover cwd, environment, user homes, or provider
paths. Missing/wrong/unwritable `BEADS_DIR` fails before mutation and never
falls back to a linked worktree's local `.beads` directory.

For Codex launch, composition adds the exact canonical `.beads` directory as a
writable root and injects `BEADS_DIR`. It does not inject a credential,
decision guard, socket, state endpoint, or other secret. Claude and Codex use
the same logical store selection.

## Configuration

```text
.abacus/
├── config.toml
├── providers.lock.toml
└── roles/
    ├── orchestrator.md
    └── worker.md
```

Role cards are project-owned authored Markdown. V1 has two authority classes,
orchestrator and worker. Cards describe responsibilities and launch behavior;
they are not cryptographic principals or a runtime capability system.

Configuration precedence:

1. safe compiled defaults;
2. repository config;
3. injected `ABACUS_*` and provider environment;
4. explicit operator flags.

No correctness-critical input comes from SABLE paths, legacy `bd`, global
agent config, shell aliases, or tmux options.

## Initialization

`abacus init`:

1. requires an existing Git repository and resolves its root/control checkout;
2. detects remotes without contacting them and chooses/asks for a base branch;
3. previews every repository-local write;
4. writes `.abacus/config.toml`, the provider lock, and starter role cards;
5. initializes or validates one stock-`br` work graph with the `ABACUS-`
   namespace;
6. records the control checkout's absolute `.beads` path as the shared-store
   source for launch composition; and
7. runs provider diagnostics and prints exact next steps.

It starts no process, creates no second state, installs no global hook, edits
no agent home, and performs no commit/push/publication. Re-running is
idempotent and never imports legacy SABLE/Dolt state automatically.

## First command surface

Exact spelling is held until the ADR-0006 necessity round. The first executable
surface is limited to what one real loop needs:

```text
abacus init
abacus doctor
abacus work <ready|show|claim|comment|close>
abacus runtime <start|prompt|inspect|read|stop>
abacus evidence <capture|show>
abacus handoff <submit|show|accept|reject>
```

Commands over workflow records use only the closed record shapes approved by
the round. There is no `scribe`, `state-rpc`, transport, enrolment, credential,
profile-activation, application-receipt, or two-store reconciliation command.

## Evidence wrapper

The wrapper captures every policy-named command through one path and records:

- argv and bounded diagnostics;
- actual exit and normalized `pass` / `assert-fail` / `execution-error`;
- exact commit/tree and declared-base identity;
- clean-tree and scoped changed-path facts; and
- any policy-approved overlay path/digest facts.

A caller cannot directly assert `Passed`. `--expect-fail` does not flip or
suppress an outcome. Collection/infrastructure failure is never red. Handoff
validation rechecks immutable commit binding, cleanliness, Evidence identity,
and policy; it does not claim to reproduce every command unless policy
explicitly requires a rerun.

Acceptance appends its decision and updates accepted/closed state in the same
shared provider domain. There is no second-store receipt. Acceptance never
means Publication.

## Direct `br` coexistence

Raw stock-`br` access is allowed by the v1 trusted-local model. The CLI must
therefore describe its guarantees honestly:

- typed commands reduce ordinary mistakes and produce the selected append-only
  shapes;
- native atomic claim prevents two simultaneous initial winners;
- another direct call can still mutate provider fields or append malformed
  text; and
- diagnostics surface inconsistent visible facts when observed, but there is
  no hidden authority store to reconcile against.

Role guidance should prefer typed ABACUS commands once they exist, but
validation must not claim raw `br` is mechanically forbidden.

## Output and errors

- Human output is concise and explanatory.
- Machine output is versioned structured stdout with diagnostics on stderr.
- Exit codes distinguish usage, unavailable/busy provider, incompatibility,
  conflict/stale facts, policy refusal/rejection, ambiguity, and internal
  failure.
- No consumer parses colored or table-form human output.
- A result says whether a durable mutation occurred, did not occur, or may
  have occurred.
- Provider output is bounded and secrets are never stored in workflow facts.

## Dependency rule

`abacus-cli` may depend on core, work, and runtime as the composition root. It
must not preserve `abacus-state` merely because current journeys use it; final
placement follows the necessity round. Business transition policy belongs in
the owning use case, not command handlers.

## Test contract

Default tests cover:

- command parsing/help/exit mapping;
- init preview, idempotency, edited-card preservation, and legacy-state
  refusal;
- `BEADS_DIR` resolution, absolute-path validation, cross-worktree identity,
  and no discovery fallback;
- provider-lock and environment construction;
- machine output and stderr separation;
- Evidence capture and truthful three-way outcome normalization;
- exact-commit Handoff validation and Acceptance/Publication distinction;
- one-winner claim composition;
- Herdr launch/inspect/stop composition through fakes; and
- absence of any Scribe/socket/RPC/credential surface.

Handlers use fake module interfaces. The four hermetic journeys cover critical
composition; live providers remain opt-in until the operator-authorized
`ABACUS-2IS` pilot.
