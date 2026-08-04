# ABACUS architecture

Status: initial contract for adversarial review  
Last updated: 2026-08-04

## Purpose

ABACUS turns a dependency-aware bead graph into locally executed, evidence-backed agent work. Its architecture is optimized for a small trustworthy loop, replaceable infrastructure, and cheap change.

The system is intentionally not a general agent platform. It coordinates one local repository, two role types, and a small number of explicit providers.

## Architectural forces

The design responds to these forces:

- ABACUS implementations and modules will evolve repeatedly; most changes must remain local to their owning module.
- Public interfaces should change much less often than implementations, and a change should incur tests proportional to the seam it crosses.
- Beads are an effective durable plan and recovery mechanism.
- Agent runtimes, terminal interfaces, shells, paths, and provider versions change frequently.
- Agent sandboxes may edit worktrees while denying writes to `.git` or user-global configuration.
- Runtime observations are fallible and must not be confused with domain completion.
- Upstream tools already solve work graphs, graph analysis, and terminal persistence better than ABACUS should.
- A one-line adapter change must not trigger an exhaustive live fleet test.
- Legacy SABLE showed that hidden hooks, lateral coupling, and cross-cutting test machinery make local change progressively more expensive.

## Invariants

These are architectural rules, not implementation suggestions.

1. **One supported product interface.** Authored roles, skills, operators, and workers use `abacus` (or `abx`). They do not invoke `br`, `bv`, Herdr, SQLite, or tmux directly.
2. **Deep module interfaces.** Each module owns its implementation, data details, fixtures, and tests behind a small interface. Callers cannot depend on internals.
3. **Proportional validation.** Internal changes run owning-module tests; interface changes add direct-consumer checks; only shared domain changes justify workspace-wide fan-out.
4. **Minimal shared core.** `abacus-core` contains only genuinely shared domain invariants and use-case ports. Convenience helpers and provider-shaped types cannot accumulate there.
5. **Topology is configuration.** Core knows two authority classes, not a fixed list of named managers. Existing capabilities can move between named profiles without Rust changes.
6. **Explicit environment.** Only the composition root discovers the host. Inner modules receive resolved paths, executable identities, clocks, process runners, and sockets explicitly.
7. **Acyclic module dependencies.** Adapter modules depend on `abacus-core`, never on one another. `abacus-cli` is the composition root.
8. **Unambiguous state ownership.** The work graph, ABACUS coordination records, and runtime state have different owners. Cached observations never silently become authoritative.
9. **Evidence-gated completion.** A pane reporting `done` is not completion. An accepted handoff requires a verified commit, a clean worker tree, and policy-satisfying evidence.
10. **Provider containment.** External types, paths, exit codes, and raw JSON stop at their adapter. Every adapter returns normalized ABACUS results and errors.
11. **Optional advice stays optional.** Loss, timeout, or malformed output from `bv` cannot prevent deterministic ready-work selection.
12. **Idempotent workflow writes.** Retried client requests cannot create duplicate assignments, evidence, or terminal transitions.
13. **Fenced ownership.** A stale worker cannot write as the current assignment holder after its lease or execution attempt has been replaced.
14. **No hidden global machinery.** ABACUS does not require global hooks, shell aliases, global state files, or mutations under another tool's configuration directory.
15. **Hermetic default tests.** The default workspace suite cannot require installed providers, a live multiplexer, GitHub, network access, user home configuration, or real agent sessions.
16. **Structural growth is visible.** A new cross-module dependency, breaking interface change, live test in a default lane, or material increase in the hermetic test budget requires an ADR.
17. **Coordination is typed and durable before notification.** Decision-shaped coordination is an immutable, subject-bound Signal (`Directive`, `Report`, or `Request`) in the Ledger. Herdr can announce it, but notification success never owns, acknowledges, or resolves it.
18. **Verification policy is Assignment-local.** A policy may require a red-green pair from ordinary Evidence records: assertion-level red against the declared-base implementation using digest-bound verification overlays, then green at the Handoff commit. No universal red-green gate, coverage machinery, or threshold exists. An authored role card may choose that form by default; Rust never does.

## System context

```text
                                      optional, advisory
                                  +-----------------------+
                                  | beads_viewer (`bv`)   |
                                  +-----------^-----------+
                                              |
+----------+       +-------------------+       |       +-------------------+
| operator |------>| `abacus` / `abx` |-------+------>| beads_rust (`br`) |
+----------+       | composition root  |               | work graph        |
                   +----+----------+----+               +-------------------+
                        |          |
              local RPC|          |normalized runtime operations
                        v          v
                 +----------------+ +-------------------+
                 | Scribe         | | Herdr             |
                 | `abacus-scribe`| | runtime + doorbell|
                 +-------+--------+ +---------+---------+
                        |                    |
                        v                    v
                 SQLite state         Claude / Codex
                        ^                    workers
                        |                    |
                        +------evidence------+ 
```

All arrows crossing into an external provider pass through an adapter. The diagram shows runtime relationships; compile-time dependencies are narrower.

## Compile-time dependency direction

```text
abacus-core
   ^       ^       ^
   |       |       |
 state    work   runtime
   ^       ^       ^
    \      |      /
       abacus-cli
```

| Module | Internal dependencies allowed |
| --- | --- |
| `abacus-core` | None |
| `abacus-state` | `abacus-core` |
| `abacus-work` | `abacus-core` |
| `abacus-runtime` | `abacus-core` |
| `abacus-cli` | All four modules; this is the only composition root |

No shared `utils` module is planned. Small duplication is preferable to a shallow common module that couples every crate. Promote duplicated behavior only after the deletion test shows that one deep module would restore locality.

Authored Markdown assets depend on the documented `abacus` command interface and domain vocabulary. They are not Rust dependencies and cannot reach provider commands around the facade.

## Change model and blast radius

Every module is accountable for four things in the same folder: its interface, implementation, fixtures, and tests. Tests in another module exercise only its public interface. They do not import internal helpers or reuse private fixtures.

The expected validation depends on the kind of change:

| Change kind | Expected blast radius |
| --- | --- |
| Internal implementation or refactor with unchanged behavior | Owning module tests only during the edit loop |
| New behavior behind an unchanged interface | Owning module tests; focused CLI/use-case composition check if that behavior is exposed there |
| Backward-compatible interface extension | Owning module tests plus tests of direct consumers using the new surface |
| Provider version/output change with normalized interface unchanged | Owning adapter contract tests plus that provider's live compatibility lane |
| Breaking module interface change | Explicit design review/ADR, owning module, and all direct consumers |
| Shared domain invariant change in `abacus-core` | Full hermetic workspace; this should be uncommon |
| End-to-end workflow semantic change | Affected modules plus the smallest relevant hermetic acceptance journey |

This is not implemented by a large custom test selector. The repository uses ordinary Cargo package targets while editing and a bounded hermetic workspace suite at handoff. The structure itself supplies locality.

Module interfaces should be deep: they express outcomes and invariants, not a mirror of internal steps. If a caller must know a provider command sequence, database table, socket message, or fixture layout, the seam is leaking and the module has lost its leverage.

There is no shared integration-test dumping ground. Cross-module journeys live with the use case that owns them and use public fakes supplied for that interface. The root suite contains only a few critical vertical journeys, not one end-to-end replay for every behavior.

## Composition and host discovery

`abacus-cli` resolves the environment once for each invocation and supplies a module-specific projection to each dependency. This prevents machine details from becoming another source of cross-module coupling. No inner module calls `dirs`, inspects `$HOME`, walks `PATH`, discovers Git roots, or reads global agent configuration on its own.

### Configuration precedence

From lowest to highest precedence:

1. compiled safe defaults;
2. repository `.abacus/config.toml`;
3. `ABACUS_*` environment overrides;
4. explicit command flags.

The resolved environment contains at least:

- repository root and Git common directory;
- stable local repository identity;
- Scribe database and socket paths;
- provider executable paths, versions, and capability results;
- provider timeouts;
- current actor identity and role;
- clock and process/socket implementations.

Each module receives only the fields it needs. There is no application-wide mutable configuration singleton.

### Repository identity

`<repo-id>` is a random local repository-instance identifier created by Scribe and stored under `<git-common-dir>/abacus/`. It moves with the Git common directory, survives a directory rename, and differs between independent clones. It is not derived solely from an absolute path and is not the user-visible project name.

The socket path is:

```text
$XDG_RUNTIME_DIR/abacus/<repo-id>.sock
```

If `XDG_RUNTIME_DIR` is unavailable, startup fails with a diagnostic and an explicit override suggestion; modules do not invent different fallbacks independently.

## State ownership

### Work provider state

`br` is authoritative for:

- bead identity, title, description, type, and priority;
- dependency edges and ready/blocked derivation;
- work status such as open, in progress, and closed;
- the work graph's own event/sync representation.

ABACUS may record immutable work snapshots for audit, but a snapshot is labeled with its provider revision or content hash and never becomes a second mutable graph.

### ABACUS workflow state

Scribe (`abacus-scribe`) is the only service allowed to write these Ledger facts:

- actors and role identity;
- profile/capability snapshots used for authorization decisions;
- assignments and immutable execution attempts;
- leases and fencing tokens;
- sanitized immutable Envelope snapshots;
- immutable typed Signals: attempt-scoped Directives/Reports and actor-to-actor Requests;
- runtime-handle associations and audit events for observations explicitly reported by an actor;
- evidence bundles and commit handoffs;
- accept/reject/revoke decisions and work-status application receipts;
- idempotency keys and audit events.

Durable state is SQLite in WAL mode at:

```text
<git-common-dir>/abacus/state.sqlite3
```

Only Scribe opens this database for mutation. Clients use a local versioned protocol over the repository socket. Scribe is a transactional state service, not a manager, message transport, scheduler, watcher, or policy engine.

### Runtime provider state

Herdr is authoritative for:

- processes, panes, sessions, and terminal buffers;
- provider session identifiers exposed by Herdr;
- observed runtime status and lifecycle events;
- live agent message and prompt delivery.

ABACUS stores the association between an execution attempt and an opaque Herdr handle. The adapter's private handle includes the named-session namespace, pane ID, and terminal/process generation; the pane ID alone is not identity because Herdr may restore it with a new terminal after restart. The association is durable; Herdr's current observation is not. A generation mismatch reports stale/unknown and requires explicit re-association. Scribe may append an audit event when an actor explicitly reports an observation, but it never polls, refreshes, or maintains live runtime status. Agent/pane status can trigger investigation but cannot accept or reject a handoff.

ABACUS does not implement a generic parallel mailbox. Critical instructions and decision-shaped coordination are first represented as durable workflow facts: the initial Envelope or a typed Signal with a mandatory bead/Assignment/Attempt/scope subject. Herdr then carries a bounded live prompt or content-free doorbell. Transient strategy/chatter may exist only in Herdr and is never treated as durable evidence or authority.

The routing rule is deliberate: progress is recorded as state and queried, not copied into live messages; a work-shaped blocker is a dependency edge in `br`; a worker progress/blocker record is a Report; an orchestrator-to-orchestrator arbitration, authority-transfer, reconciliation, or other decision-shaped ask is a Request; and binding direction to a worker is a Directive. Signal bodies never ride Herdr prompts. Per-actor and global unresolved sets derive from immutable Signals lacking their typed responding actions; there is no mutable inbox, `read_at`, per-Directive acknowledgement, delivery retry, or escalation-on-silence state.

### Ownership matrix

| Fact | Authority | ABACUS may cache? |
| --- | --- | --- |
| Bead dependencies and readiness | `br` | Yes, with revision/hash |
| Suggested priority order | None; `bv` advice | Yes, with graph hash and expiry |
| Assignment holder | Scribe/Ledger | Not elsewhere |
| Lease validity and fencing token | Scribe/Ledger | Client hint only |
| Signal records, derived unresolved sets, and current binding Directives | Scribe/Ledger | Not elsewhere |
| Agent process exists | Herdr | Yes, observation with timestamp |
| Worker claims to be finished | Evidence submission | Yes, pending validation |
| Handoff accepted | Scribe/Ledger decision | Not elsewhere |
| Bead closed | `br` | Yes, observed snapshot |
| Source commit object | Git | Yes, commit identity and verification result |

## Core domain model

Exact Rust names may evolve, but these concepts and distinctions must remain.

- **Bead ID** — validated identifier in the `ABACUS-` namespace.
- **Actor** — stable ABACUS identity with one role: orchestrator or worker.
- **Profile** — repository-configured name, instructions, capabilities, and responsibility scope applied to one or more actors. It is not a new authority class.
- **Capability** — a namespaced permission declared by the module owning one supported use case, such as select work, assign, observe runtime, send alerts, or decide a handoff. Core evaluates grants/scopes without enumerating every module capability.
- **Decision actor** — one concrete actor ID plus the name and content hash of the role-card profile active for that action.
- **Assignment** — an orchestrator decision connecting a bead, worker, intended edit scope, acceptance policy, and exact decision actor.
- **Execution attempt** — an immutable attempt to perform an assignment. Retries create new attempts rather than rewriting history.
- **Lease** — time-bounded ownership carrying a monotonically increasing fencing token.
- **Runtime handle** — an opaque reference returned by `abacus-runtime`; never an actor identity.
- **Envelope** — the canonical sanitized context snapshot persisted with an Assignment/Attempt before the exact same content is delivered through Herdr.
- **Signal** — an immutable, idempotently appended coordination record with a closed type, the fenced sender's full actor/profile/capability/scope identity, and exactly one validated Bead, Assignment, Attempt, or responsibility-scope subject. No Signal accepts a subject-free body.
- **Directive** — an orchestrator-to-Attempt Signal containing amended instructions, pause, abort, or an answer to a Report. It requires the Assignment's exact decision authority and binds the current Attempt from commit, unread included.
- **Report** — a worker-to-decision-actor Signal recording structured progress or blocked-with-reason state from the current Attempt under its current lease/fencing token. A linked responding Directive or fenced decision resolves it.
- **Request** — an actor-to-actor Signal carrying a bounded decision-shaped ask, initially arbitration, authority transfer, or reconciliation. It is the orchestrator channel and only a linked responding fenced decision, including refusal, resolves it.
- **Evidence bundle** — ordinary structured verification commands, actual outcomes, exact commit bindings, before/after workspace digests, relevant artifacts, and environment facts. “Red” and “green” are policy-derived roles for these same records, never separate evidence classes.
- **Commit handoff** — commit identity, base identity, worktree cleanliness proof, evidence identity, and worker attestation.
- **Submission refusal** — an audited failed precondition before a Handoff is recorded; the Attempt remains active.
- **Rejection** — an immutable orchestrator decision on a recorded Handoff; terminal for that Attempt but not its Assignment.
- **Acceptance** — an immutable authorizing decision that terminally accepts the Assignment/Attempt, followed by a separately tracked work-status application and receipt.
- **Decision** — orchestrator acceptance, rejection, revocation, or cancellation with reason and audit identity.

Provider payloads are converted into these concepts at their seam. Raw JSON values are not domain objects.

## Authority classes, profiles, and topology

`orchestrator` and `worker` are stable authority classes. They express the fundamental trust distinction: orchestrators coordinate and decide; workers execute and submit. They do not imply one orchestrator process or one fixed manager topology.

A named agent is a profile layered on an authority class:

```text
profile: delivery-manager
class: orchestrator
capabilities: [work:select, state:assign, state:decide_handoff]
scope: label=delivery

profile: watchdog
class: orchestrator
capabilities: [runtime:observe, state:read_audit, runtime:prompt]
scope: repository
```

Role-card Markdown frontmatter is the single profile definition. Repository configuration references cards and routes responsibility; it does not duplicate their authority schema. The semantics are fixed:

- capabilities correspond to supported use cases, not arbitrary command strings;
- each owning module declares its capability descriptors; the composition root builds the known-capability registry and core applies generic grant/scope rules;
- adding a module-local capability changes that module and its direct composition check, not `abacus-core`;
- no wildcard capability silently grants future use cases;
- scopes constrain the beads, assignments, or actors on which a capability operates;
- active grants are snapshotted with actor ID and profile content hash so later card edits do not rewrite history;
- loading/activating a changed profile records an audit event before it authorizes new actions;
- every mutating action records the concrete actor, profile hash, capability, scope decision, and fencing/idempotency data;
- assignments name the exact actor authorized to accept or reject their handoff;
- transfer of decision authority is explicit, fenced, and audited.

Scopes declare whether a capability is exclusive or shared. Configuration validation rejects overlapping exclusive mutation/decision scopes before sessions launch. Shared read, observation, and alert scopes may overlap. Scribe still serializes decisions and enforces fencing as a backstop; configuration validity never replaces write-time authorization.

This makes topology evolution cheap:

- splitting existing manager responsibilities changes profiles, routing, and authored cards;
- adding a watchdog without workflow-mutation authority uses existing observation, audit-query, and Herdr prompt interfaces;
- orchestrator-to-orchestrator decision asks use subject-bound Requests, so adding a manager does not add a mailbox, routing daemon, or pairwise protocol;
- adding new watchdog behavior changes only the owning use case and its direct consumers;
- creating a genuinely different authority class changes shared domain invariants and therefore requires an ADR and wider tests.

ABACUS is not a general RBAC system. The capability vocabulary remains small and is defined by real ABACUS use cases. Named historical SABLE roles are not baked into Rust enums, database columns, provider metadata, or command names.

A watchdog is a spawned orchestrator-class agent profile, not another daemon or resident loop. A watchdog without workflow-mutation authority uses observation and audit-query capabilities and can send live alerts through Herdr. Any automated recovery action requires its own explicit capability and follows the same fencing, exact-actor, and audit rules as a human-directed manager action.

Authored orchestrator policy follows two routing rules: progress is queried, never copied into messages; and cross-scope blockers become `br` dependencies when work-shaped or Requests when decision-shaped. These rules keep the Signal family small enough to remain typed workflow state.

When the Phase 6 authored assets land, the orchestrator role card defaults unsupervised autonomous Assignments to the red-green evidence-pair policy form. The card explicitly selects that policy when creating each Assignment. This is an authored orchestration default, not a CLI/core default: supervised and downstream workflows choose their own per-Assignment verification policy.

## Interfaces at seams

Interfaces belong to the use cases that consume them. Do not create a generic provider framework.

### Work graph interface

Required behavior:

- list and inspect normalized work;
- derive ready work deterministically;
- create and update beads;
- add or remove dependency edges;
- mark work in progress, close it, or reopen it;
- expose a revision/hash suitable for binding advice and audit snapshots.

Mutations validate the `ABACUS-` namespace and actor authorization before the `br` adapter runs.

### Work advisor interface

Required behavior:

- accept a graph revision and optional scope;
- return ranked suggestions with reasons and the analyzed revision/hash;
- distinguish complete, partial, timed-out, unsupported, and malformed analysis.

The use case always has a deterministic fallback based on ready status, explicit priority, and stable ID ordering.

### Scribe state interface

Required behavior:

- create/read assignments and attempts;
- acquire, renew, release, and fence leases;
- persist and read canonical sanitized Envelope snapshots;
- append/read authorized typed Signals with mandatory workflow subjects;
- append/read ordered attempt-scoped Directives and mechanically return the current binding set in every fenced worker response;
- append/read current-Attempt Reports and validate responding Directive/decision links;
- append actor-to-actor Requests and validate their responding fenced-decision links;
- query per-actor and global unresolved sets derived from immutable Signals lacking their typed responding actions;
- preserve immutable per-Attempt fenced call/response ordering from which Directive exposure and discharge are derived;
- append and inspect ordinary Evidence records and query them by Assignment/Attempt, normalized verification set, commit binding, closed normalized outcome, and optional overlay path/digest metadata;
- submit and decide handoffs;
- record immutable work-status application attempts/receipts and derive decisions lacking a successful receipt;
- append/query audit events;
- process idempotent requests.

### Runtime interface

Required behavior:

- start a worker from an explicit launch specification;
- deliver an ABACUS-generated context envelope;
- deliver bounded live prompts and content-free workflow doorbells;
- inspect a runtime handle;
- wait for or subscribe to observations;
- read bounded output for diagnosis/evidence;
- signal or stop a runtime;
- recover/re-associate known handles after Scribe restart where supported.

The interface exposes normalized observations such as running, blocked, idle, exited, unavailable, and unknown. These are runtime observations, not assignment states.

## Lifecycle

### Install once

The development installation path is a normal Rust source install:

```bash
git clone <repo-url> abacus
cd abacus
cargo install --path abacus-cli
```

A later prebuilt installer may replace these mechanics without changing project initialization or domain behavior. Installation provides binaries and packaged seed assets; it does not mutate an arbitrary project, global agent hooks, or shell configuration.

### 1. Initialize

1. `abacus init` discovers an existing Git root, remote when present, and likely base branch without network or mutation.
2. It shows the complete plan and asks for confirmation; ambiguous or absent base-branch detection requires `--base` or an interactive choice.
3. It writes repository-local `.abacus/config.toml`, `.abacus/providers.lock.toml`, and one-time seeded, project-owned role cards. Re-running validates or previews a focused diff; it never silently overwrites edited cards.
4. Through `abacus-work`, it initializes a fresh work graph with the `ABACUS-` prefix after provider compatibility succeeds.
5. Scribe creates its local repository-instance identity, Ledger directory, schema, and socket with user-only permissions.
6. `abacus doctor` verifies the resulting configuration and provider capabilities without mutating unrelated provider or user state.

Repositories without a remote are valid. The remote is metadata for later explicit publication, not a prerequisite for local execution. Initialization does not install global hooks, modify Claude/Codex homes, commit, push, initialize over legacy SABLE beads, or import SABLE state.

The operation is idempotent and supports a dry-run/plan mode suitable for automation. If existing `.sable` or incompatible `.beads` state would be touched, initialization refuses with a migration/pilot diagnostic rather than reusing it.

### 2. Select and assign

1. The orchestrator asks `abacus-work` for a normalized ready set and graph revision.
2. If configured, `bv` advice is requested with a deadline.
3. Advice is accepted only if it refers to the current graph revision and passes schema validation; otherwise deterministic ordering is used.
4. Core policy validates that the bead can be assigned; one Scribe transaction records the Assignment, its explicit acceptance-policy form and named verification set, the exact bead-content hash/revision and declared base commit that authorized it, first Attempt, authorizing decision, and work-status operation identity. A worker cannot later select or weaken that policy.
5. The work facade marks the bead in progress through `br` using that operation identity.
6. Scribe records the normalized application attempt and, on confirmed success, its receipt. Failure or ambiguity after step 4 is derived as a decision lacking a successful receipt; reconciliation is explicit and idempotent.

### 3. Start a worker

1. The orchestrator acquires a lease/fencing token for the execution attempt.
2. The composed use case renders a canonical context Envelope containing the bead snapshot, assignment identity, Attempt identity, lease/fencing token, edit scope, acceptance policy, and supported `abacus` commands.
3. Before any live delivery, Scribe records that sanitized Envelope snapshot with the Assignment/Attempt. Provider credentials and unrelated environment secrets are excluded from the Envelope entirely.
4. `abacus-runtime` starts a Herdr-managed agent with an explicit working directory/environment allowlist and delivers that exact Envelope.
5. The runtime handle is associated with the Attempt in the Ledger through Scribe.
6. The Phase 6 worker role card's first session move is to query its Assignment and per-actor unresolved Signal set. This improves orientation and latency; Directive correctness does not depend on it because every fenced response surfaces current binding Directives mechanically.

The worker does not receive direct database credentials or raw provider mutation instructions.

### 4. Execute and communicate

1. The worker edits only its assigned worktree/scope.
2. Every fenced worker response mechanically surfaces the Attempt's current binding Directives. This is a protocol property of Scribe responses, never worker discipline; a separate `assignment sync` command is useful only for latency and orientation.
3. The worker records structured progress or blocked-with-reason state as a Report from its current Attempt and lease. Managers query this durable state rather than receiving progress copies over Herdr.
4. The Assignment's exact decision authority may append a typed, immutable Directive to the active Attempt: amended instructions within the existing bead/scope/policy, pause, abort, or an answer to a Report. Widening edit scope, changing acceptance policy, or rebinding changed bead content requires an explicit Assignment/Attempt decision instead.
5. An in-scope actor may send another durable actor a Request for arbitration, authority transfer, or reconciliation, always with a mandatory workflow subject. Work-shaped blockers become dependency edges in `br`; unstructured strategy/chatter remains transient in Herdr.
6. A Report is resolved only by a linked responding Directive or fenced decision. A Request is resolved only by its linked responding fenced decision. Directive discharge follows its closed kind and a later substantive responding workflow action; opening, prompting, or an acknowledgement-only record resolves nothing. `abacus signal unresolved` derives the remaining per-actor/global set.
7. Only after Scribe commits a Signal does the facade ask Herdr to carry a best-effort, content-free doorbell such as “workflow signal available; query unresolved.” A Signal body never rides the prompt. Doorbell failure never rolls back the Signal and creates no retry queue.
8. A Directive binds from commit. Exposure and discharge are derived from immutable call ordering and responding actions: a Directive committed before the worker's latest fenced call was, by construction of step 2, surfaced in that call's response. No `read_at` columns, per-Directive acknowledgement state, or client-asserted seen-head exists.
9. Scribe/core refuses consequential actions that conflict with the current binding set. Handoff under an undischarged pause or amend Directive receives a distinct ordinary Submission refusal, records no Handoff, and leaves the Attempt active. After abort, Scribe refuses further mutating calls except abort-consistent ones.
10. The state client is idempotent and causally ordered so lost responses or concurrent calls cannot leapfrog the response that first surfaced a Directive. Role-card synchronization guidance affects only latency; no hook, prompt, or prose instruction is load-bearing.
11. Herdr remains available for transient agent-to-agent conversation, but transient chat is not evidence or authority. Actual Directive compliance and Request/Report resolution are evaluated from linked durable state.
12. Lease renewal includes the current fencing token. A stale token is rejected even if the old runtime is still alive.
13. An authorized orchestrator may explicitly inspect unresolved Signals or blocked, exited, and unknown runtime observations and choose whether to ring a Herdr doorbell or take a fenced workflow action. No background notifier or escalation-on-silence mechanism is implied.

### 5. Submit a handoff

The worker submits:

- assignment and attempt identities;
- current fencing token;
- commit object ID and expected base;
- proof that the worker worktree is clean;
- structured commands, raw exit details, and normalized outcomes required by the assignment policy, each with before/after workspace digests;
- when the Assignment requires the red-green form, a reference to the ordinary `assert-fail` Evidence for the same verification set against the declared-base implementation, including its exact overlay path set and per-file digests;
- the normalized changed-path set for edit-scope validation;
- evidence/artifact digests where applicable;
- an attestation binding the evidence to the handed-off commit.

Both red and green runs use the standard wrapper and the existing Evidence record shape. At the wrapper boundary, framework results normalize into the closed set `pass`, `assert-fail`, and `execution-error`; the raw command/exit details remain honest. For red, the wrapper constructs an isolated checkout of the declared-base implementation and overlays only policy-named verification files from the worker's current work. The Evidence binds the base commit, exact overlay path set, each overlaid file's content digest, and the composed tree's before/after workspace digests. Green runs the same verification set natively at the Handoff commit. An `expect-fail` capture mode affects only later policy interpretation and never suppresses, inverts, or manufactures an outcome.

If a precondition fails before recording—including a policy-required red half that is missing, bound to the wrong commit, actually passing, errored before completing verification, or stale against the Handoff commit—Scribe audits a Submission refusal with the appropriate distinct reason and the Attempt remains `active`; no Handoff or decision is created. An overlay path outside the policy's verification file set is refused as malformed evidence. A valid submission records an immutable Handoff and moves the Attempt—not the Assignment—to `submitted`. It does not close the bead.

### 6. Validate and accept

1. Scribe/core verifies the Attempt is current, lease/fencing history is valid, and the effective binding Directive sequence permits Handoff. A standing pause/abort or unincorporated amendment produces a distinct Submission refusal and surfaces the Directives; the worker does not assert their read status.
2. Core policy verifies that the deciding actor has the assignment's explicit decision authority.
3. The work facade reads the authoritative bead and confirms its current content hash matches the hash bound into the Assignment. A changed task must be explicitly re-authorized; it is never silently accepted against stale requirements.
4. Git verification confirms that the commit exists, is based on an allowed base, corresponds to the submitted worktree/evidence identity, and changes no path outside the Assignment's normalized edit scope.
5. Acceptance policy evaluates the ordinary Evidence records, including before/after workspace digests that expose verification commands which mutated the tree. Any mutation must be explicitly allowed, incorporated, and followed by a clean final proof; pane text never substitutes for this check.
6. If the Assignment selected the red-green form, core derives a pair for the policy-named verification set. Red must record `assert-fail` against the declared-base implementation, its overlay paths must be a subset of the policy verification files, and every per-file overlay digest must equal that file's digest in the Handoff commit. Green must record `pass` for the same verification set run natively at the Handoff commit. Missing red, wrong-commit red, passing red, `execution-error` red (`red-errored`), and a digest mismatch or missing overlaid file (`red-stale`) are distinct refusal/rejection reasons; stale red must be recaptured. The existing missing/failing-green reasons apply to green. Pairing and overlay validation create no record or mutable status.
7. On rejection, Scribe records the immutable Rejection, ends that Attempt, and leaves the Assignment active and the bead open/in progress. Only the Assignment's authorized decision actor may explicitly retry by appending a new fenced Attempt, possibly for the same worker.
8. On acceptance, one Scribe transaction records the immutable Acceptance decision, a bounded curated close reason, and operation identity and moves the Assignment/Attempt to terminal `accepted`.
9. `abacus-work` attempts to close the bead in `br` using the decision's operation identity and curated `accepted_handoff` close-reason code, and returns a normalized outcome.
10. Scribe records the immutable application attempt and, on confirmed success, a receipt with the work revision. These records describe the step-8 decision's projection; they are not another decision or lifecycle transition.

Push, PR creation, merge, deployment, and cleanup are explicit later actions.

The decide/close/confirm sequence is a small explicit saga because SQLite and `br` cannot share a transaction. The immutable Acceptance dominates: a definite close failure or crash never un-accepts the Assignment. Accepted decisions lacking a successful receipt form a derived reconciliation set. `abacus reconcile <assignment>` inspects the `br` status/revision and operation identity, retries safely or records the receipt, and surfaces any conflict/application anomaly. There is no `accepting` state, mutable pending queue, or resident reconciliation loop.

### Assignment states

```text
created -> active -> accepted
   |        |
   |        +----> cancelled
   +-------------> cancelled
```

### Attempt and Handoff states

```text
Attempt: active -> submitted -> accepted
            |          |
            |          +----------> rejected
            +---------------------> expired
            +---------------------> revoked

Handoff candidate --precondition failure--> submission refused (no Handoff recorded)
recorded Handoff --------------------------> accepted | rejected
```

`accepted` and `cancelled` Assignments are terminal. Rejection, expiry, and revocation are terminal for an Attempt, not its Assignment. ABACUS never retries automatically: the authorized decision actor may explicitly append a new fenced Attempt under the active Assignment, or cancel it and create a new Assignment. v1 has no universal hard cap; every Attempt and Submission refusal is audited/operator-visible, and an Assignment policy may impose a maximum. History is appended rather than rewritten.

## Consistency, concurrency, and recovery

### Idempotency

Every mutating Scribe request carries a client-generated idempotency key scoped to repository and operation. Scribe stores the key and prior result in the same transaction as the mutation.

Provider mutations use an ABACUS operation identity in the audit record. Where an upstream tool cannot accept an idempotency key, the adapter performs read-before-write reconciliation and returns an explicit ambiguous-outcome error instead of guessing.

### Leases and fencing

Lease expiry alone is insufficient because an old worker may resume. Every renewed or reassigned attempt receives a monotonically increasing fencing token. Writes that carry an older token fail.

Clock-dependent decisions use the Scribe clock. Worker clocks are evidence only.

### Scribe restart

- SQLite WAL transactions are the durable commit point.
- The socket is recreated without changing repository identity.
- Lease expiry is evaluated lazily from durable expiry/fencing data on the next authorized request; restart starts no timer.
- Scribe does not inspect Herdr or `br`. Runtime associations and decisions lacking successful application receipts remain queryable as durable facts.
- Per-actor/global unresolved Signals and current binding Directives are re-derived from immutable Signal, call-order, and responding-action records; restart creates no inbox, acknowledgement repair, notifier, or delivery queue.
- A later explicit `abacus status` or `abacus reconcile` invocation composes Scribe with `abacus-runtime`/`abacus-work`; missing handles become reported `unknown` observations rather than automatic failures.

### Provider failure

| Failure | Behavior |
| --- | --- |
| Scribe unavailable | Mutations fail closed; read-only cached output is clearly marked stale |
| `br` unavailable/incompatible | Work operations fail with doctor guidance; assignments are not invented from cached work |
| `bv` unavailable/timeout/malformed | Report the advisory failure and use deterministic ready ordering |
| Herdr unavailable | Do not create a runtime association; assignment remains recoverable |
| Signal doorbell lost | Signal remains authoritative in the Ledger; derived unresolved queries expose it, and every fenced worker response mechanically surfaces current binding Directives. No Signal body was on the doorbell, and no background retry or unread-state repair runs. |
| Signal recipient inactive | The per-actor/global unresolved queries retain the Signal until its typed responding action exists. The recipient's next facade activation queries its unresolved set; operator status can query the global set. No escalation-on-silence process starts. |
| Worker exits | Herdr returns an exit observation. Nothing expires automatically; on the next explicit inspection or fenced action, policy evaluates the observation and lease time and may make the Attempt reclaimable. |
| Stale lease writer | Reject by fencing token and record audit event |
| Binding Directive conflicts with requested action | Reject before mutation and surface the current binding Directive set; exposure/discharge remains derived from immutable call/action ordering |
| Dirty worker tree | Refuse submission with a structured reason before recording a Handoff; if discovered against a recorded Handoff at decision time, record an explicit Rejection of that Attempt |
| Missing/failed evidence | Refuse submission before recording a Handoff, or explicitly reject the recorded Handoff according to assignment policy; never conflate refusal with Rejection |
| Required red-green pair invalid | Use distinct policy reasons for missing red, red bound anywhere but the declared base commit, passing red, `execution-error` red (`red-errored`), or overlay digests that do not match the Handoff commit (`red-stale`, requiring recapture). Refuse out-of-policy overlay paths as malformed evidence. Preserve the actual ordinary Evidence records; never invert an outcome, create a pair row, or substitute coverage/threshold checks. |
| Ambiguous provider mutation | Stop, inspect provider state, and reconcile idempotently |
| Provider schema/version mismatch | Refuse affected operation; do not parse best-effort text |

## Provider adapters

### `br`

The initial work adapter invokes `br` using argv execution, an explicit working directory, a sanitized environment, a deadline, and machine-readable output. It never builds a shell command string.

The adapter:

- accepts only the pinned executable identity and compatible schema/capabilities;
- requests JSON/robot output and validates all fields it consumes;
- maps upstream identifiers and errors into ABACUS values;
- records graph revision/hash before advice or mutation;
- returns revisioned status observations and normalized anomaly signals so the core use case can correlate them with Ledger decisions without giving the work module Ledger access;
- renders only bounded curated close reasons from an Acceptance operation (`accepted_handoff` initially), never arbitrary message text;
- serializes work mutations at the ABACUS use-case level;
- flushes the provider's JSONL representation when required by the pinned version;
- never commits, pulls, pushes, or installs hooks.

The initial role policy allows the orchestrator to mutate the graph. Workers report progress and handoffs through Scribe; they do not close their own beads.

### `bv`

The advisor invokes only non-interactive robot modes and validates that returned advice corresponds to the graph input it analyzed. It has a short deadline and a circuit-breaker-style cool-down after repeated incompatibility.

No ABACUS transition depends on a ranking score. Advice is an explanation-bearing ordering hint.

Compatibility between the pinned `br` JSONL layout and the pinned `bv` reader is a required spike. ABACUS does not assume that two tools from the same ecosystem remain schema-compatible.

### Herdr

The v1 runtime adapter uses Herdr's high-level CLI/JSON facade after capability negotiation: named sessions, `agent start/prompt/wait/read`, and workspace/pane operations. Direct socket implementation is deferred unless it provides a measured benefit without changing the normalized interface. The adapter treats provider identifiers as an opaque generation-fenced handle and keeps ABACUS role/Assignment identity in the Ledger through Scribe. Herdr owns every live message/prompt operation; the adapter does not shadow it with ABACUS delivery state. For any durable Signal, the facade commits through Scribe first and then asks this adapter for only a best-effort doorbell.

Each repository uses a collision-resistant Herdr named-session namespace derived from the local ABACUS repo ID. Herdr v0.7.5 keeps named-session state under its provider-owned user root even when config/socket overrides are supplied; ABACUS does not pretend to own or relocate that state. It installs no Herdr integration/plugin. Disposal and T3 tests stop/delete only their exact named namespace and verify a pre/post namespace manifest.

Herdr screen-manifest status is useful observation but not authoritative completion. ABACUS does not rebuild tmux scraping in the core. If provider-specific detection or lifecycle reporting is needed, it remains inside the Herdr adapter and its compatibility tests.

Herdr's GitHub repository has changed owner namespace during this design (`ogulcancelik/herdr` now resolves to [`herdrdev/herdr`](https://github.com/herdrdev/herdr)). The provider lock records immutable release/commit and checksum identity rather than relying on repository naming.

## Provider pinning and upgrades

`.abacus/providers.lock.toml` will record for each executable:

- canonical provider name;
- exact version and, where available, commit/release identity;
- expected binary checksum;
- protocol/schema fingerprint or declared capability set;
- fixture set version last validated against it.

An upgrade is an explicit workflow:

1. obtain the candidate binary without replacing the active pin;
2. verify checksum/source metadata;
3. capture or update sanitized machine-output fixtures;
4. run adapter contract tests against the fixtures;
5. run the provider's live compatibility smoke suite in a temporary repository/config root;
6. review normalized-interface changes;
7. update the lock and compatibility record together.

The rest of the workspace does not change unless the normalized interface must change. If it must, that is an architectural change and receives an ADR.

## Test architecture

The test architecture exists to preserve implementation locality as ABACUS changes, not to maximize the number of gates.

### Tiers

| Tier | Contents | Runs when | Live dependencies |
| --- | --- | --- | --- |
| T0: domain/unit | Pure rules, parsing of ABACUS-owned formats, transition tables | During edits and every handoff | None |
| T1: adapter contract | Fake process/socket tests against checked-in provider fixtures | Adapter edits and every handoff | None |
| T2: hermetic acceptance | Multi-module use cases with fake work, state, runtime, Git, and clock implementations | Every handoff | None |
| T3: provider compatibility | Minimal real `br`, `bv`, Herdr, Git, and socket smoke tests in temporary roots | Provider pin changes; scheduled/manual | Pinned local binaries |
| T4: live agent journey | One disposable repository and real Claude/Codex workers | Release candidate/manual diagnosis | Real providers and credentials |

`cargo test --workspace` includes T0–T2 only. T3 and T4 require explicit commands/features and can never be pulled into the default suite by a helper script.

### Locality matrix

| Changed area | Required fast feedback while editing |
| --- | --- |
| `abacus-core` internal implementation | Core tests while editing; the same bounded hermetic workspace gate required for every Handoff remains the backstop |
| `abacus-core` interface/invariant | Core plus all hermetic consumers; requires design review because this is the broadest seam |
| `abacus-state` internal implementation | State tests |
| `abacus-state` public interface | State plus direct CLI/core-use-case composition tests |
| `abacus-work` internal implementation or fixtures | Work tests |
| `abacus-work` public interface | Work plus direct CLI/core-use-case composition tests |
| `abacus-runtime` internal implementation or fixtures | Runtime tests |
| `abacus-runtime` public interface | Runtime plus direct CLI/core-use-case composition tests |
| `abacus-cli` | CLI tests |
| Authored Markdown | Markdown/link/schema checks only |
| Provider lock | Affected adapter tests plus explicit T3 compatibility suite |

There is no custom impact-analysis daemon. Cargo package targets provide edit-time locality; the bounded hermetic suite provides handoff confidence. A module does not acquire another module's tests merely because both happen to use the same provider or helper.

### Initial budgets

On the development machine used to establish the baseline:

- `abacus-core` tests: target under 5 seconds warm;
- any adapter module's hermetic tests: target under 15 seconds warm;
- complete T0–T2 workspace: target under 90 seconds warm;
- no default test may sleep on wall-clock time or wait on a real process timeout.

Record actual baselines once code exists. On slower/faster machines, compare both absolute time and regression from that machine's recorded baseline. A budget increase is investigated before being accepted; it is not hidden by automatically skipping more tests.

### Test seams

- Core receives deterministic clocks, ID generation, and policy inputs, including red-green pairing and overlay validation over ordinary fake Evidence values.
- State uses temporary Git common directories and SQLite files; tests cover Signal subject/sender fencing, per-actor/global linked-resolution derivation, mechanical Directive surfacing and causal-call enforcement, red-green evidence queries with overlay metadata but no new record class, and schema/interface proof that no read/ack state exists.
- Work uses a fake argv process runner and versioned stdout/stderr/exit fixtures.
- Runtime uses a fake Herdr socket/protocol peer and recorded event fixtures; it does not need state tests to exercise a doorbell.
- CLI composes in-memory/fake adapters through the same interfaces used in production.
- Live compatibility tests redirect provider config/state into temporary directories when supported. For Herdr named sessions, whose v0.7.5 state root is not redirectable, they use a collision-resistant disposable namespace, record an exact pre/post manifest, install no integrations/plugins, and stop/delete only that namespace.

Avoid giant golden snapshots. Fixtures represent upstream contracts and should be minimal examples for fields ABACUS consumes, plus malformed and forward-compatible cases.

## Operational safety

- The Scribe socket and Ledger directory are user-only.
- Local peer identity is checked where the platform supports it.
- Provider commands are invoked as argv without a shell.
- Environment forwarding uses an allowlist, not the caller's entire environment.
- Secrets are not persisted in assignment context, evidence, or audit rows.
- Persisted Envelope snapshots are size-bounded and exclude provider credentials and unrelated environment secrets.
- Database migrations are transactional and backup-aware.
- Destructive recovery requires explicit targets and confirmation.
- No initialization step edits `~/.claude`, `~/.codex`, tmux configuration, shell startup files, or global Git configuration.

## Observability

Structured audit events answer who requested what, against which bead/assignment/attempt, with which idempotency key and fencing token, and what durable result occurred.

Logs are diagnostic and may be rotated. Logs are never the only record of a state transition. Provider stdout/stderr is bounded, redacted where necessary, and linked to—not substituted for—normalized results.

## Compatibility spikes required before code hardens

1. **`br` contract:** verify ID prefix configuration, JSON schemas used by required operations, JSONL flush behavior, locking, and canonical working-directory behavior across Git worktrees.
2. **`bv` contract:** verify that the pinned version reads the exact pinned `br` representation and returns a graph/data hash that can be bound to advice.
3. **Herdr contract:** verify generation-fenced runtime handles, CLI/API schema discovery, atomic prompt delivery, events/wait behavior, restart recovery, Claude/Codex launch environment, named-session isolation, sandbox access, and status uncertainty.
4. **Sandbox contract:** verify a worker denied direct `.git` writes can still reach Scribe, produce a Git commit through the approved execution environment, and submit evidence without global config changes.
5. **Path-change contract:** rename/move a repository and verify repository identity, state discovery, and socket recovery without recreating an obsolete path.

Each spike uses a disposable repository and redirected configuration roots. Passing evidence is checked in as a compatibility record; exploratory scripts do not become permanent orchestration unless they earn a deep interface.

## Architecture acceptance criteria

The first vertical slice is architecturally acceptable when:

- a normalized `ABACUS-` bead can be selected without `bv`;
- an orchestrator can create one durable assignment and start one Herdr-managed worker;
- workers can durably Report progress/blockers, orchestrators can issue Directives and actor-to-actor Requests, per-actor/global unresolved coordination derives from linked responding actions, and Herdr can ring content-free transient doorbells without a generic ABACUS inbox;
- every fenced worker response mechanically surfaces the Attempt's current binding Directives as a Scribe protocol property, and a worker can submit a fenced, Directive-compliant, evidence-bound clean commit without hooks/read receipts;
- Acceptance rejects changed bead content, out-of-scope paths, and unaccounted verification-induced workspace changes;
- an Assignment may require assertion-level red against its declared-base implementation using only policy-named, digest-bound verification overlays and green for the same verification set at its Handoff commit, while green-only, wrong-commit-red, passing-red, red-errored, red-stale, and out-of-policy-overlay cases are refused correctly without coverage machinery or new evidence records;
- the orchestrator can reject or accept the handoff and only acceptance closes the bead;
- an existing orchestration capability can move between two named profiles without rebuilding adapter modules;
- Scribe and runtime restarts preserve or explicitly reconcile durable state;
- provider incompatibility is diagnosed at its adapter rather than leaking parse errors across modules;
- T0–T2 run hermetically within the agreed budget;
- T3 validates each pinned provider without touching live user configuration;
- no ABACUS path, environment variable, or new state root is SABLE-branded;
- no operation requires a global hook or implicit push.

## Deliberately open decisions

These should be settled by spikes or focused ADRs, not guessed into the first implementation:

- exact command grammar beneath the top-level `abacus` groups;
- precise schema for evidence policies and command results;
- canonical control-checkout strategy for `br` when workers use Git worktrees;
- whether the Scribe transport begins as JSON, MessagePack, or another versioned local encoding;
- Herdr CLI versus socket operations for each runtime function;
- packaging strategy for installing the `abx` alias;
- thresholds that justify an ABACUS fork of an upstream provider.
