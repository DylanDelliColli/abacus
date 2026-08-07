# `abacus-runtime` module contract

> **ADR-0006 transition (2026-08-07).** Herdr remains the runtime provider.
> References below to Scribe/Ledger transport are superseded: durable facts use
> the one shared stock-`br` store, and the necessity round decides whether an
> Attempt locator or durable runtime association survives. Runtime carries no
> state secret and never writes a second store.

Status: design contract; no Rust implementation yet

## Purpose

`abacus-runtime` is the only ABACUS module allowed to control or inspect the agent execution substrate. It implements a normalized runtime interface over pinned Herdr while keeping pane/session mechanics and detection uncertainty outside the domain.

## Owns

- Provider-neutral launch specifications and opaque runtime handles
- Herdr CLI/socket adapter and protocol validation
- Agent start, live message/prompt delivery, inspect, wait/subscribe, bounded read, signal, and stop behavior
- Explicit working directory and environment allowlist construction
- Mapping of Herdr observations into normalized runtime observations
- Runtime recovery/re-association behavior after provider/composition restart
- Herdr capability/version detection and compatibility fixtures
- Runtime-provider-specific diagnostics surfaced through `abacus doctor`
- Namespaced capability descriptors for runtime-owned use cases

## Does not own

- Actor identity, profile capabilities, assignment state, or decision authority
- Beads or work-graph mutation
- Lease/fencing authority
- Durable workflow records, evidence, handoff, or acceptance state
- Signal content, workflow subjects/resolution, unread state, or acknowledgement state
- Git worktree creation/verification unless a later ADR explicitly expands the module
- Completion inferred from terminal text
- tmux compatibility or SABLE pane metadata
- Global Claude/Codex integration installation

## Initial provider

[Herdr](https://github.com/herdrdev/herdr) is a persistent terminal-based agent multiplexer with CLI and local socket control surfaces. It supports agent spawning, output reading, waiting/events, and status observation.

Herdr's GitHub repository has changed owner namespace during this design (`ogulcancelik/herdr` now resolves to `herdrdev/herdr`). The provider lock therefore records immutable release/commit and checksum identity rather than treating an owner/repository string as artifact identity.

ABACUS starts with an external pinned executable, not a fork or linked internal crate.

## Deep runtime interface

### Launch

Input is an explicit launch specification containing:

- the closed **launch subject** — a worker Assignment Attempt or a spawned orchestrator/watchdog actor activation (including its activation generation). Every launch, recovery, and durable handle association is keyed by this same non-secret subject;
- provider kind such as Claude or Codex;
- executable/argument selection resolved by configuration;
- absolute working directory;
- allowlisted environment;
- the exact canonical sanitized ABACUS Envelope rendered from durable shared-store facts;
- startup and delivery deadlines.

Output is an opaque, generation-fenced ABACUS runtime handle plus normalized
launch facts. The adapter delivers the sanitized Envelope as one startup
submission and reports one closed startup-delivery outcome (`Submitted` = one
provider API submission accepted, never proof of application;
`NotDelivered(reason)` keeps the handle for explicit stop/inspection;
`Ambiguous`). The adapter privately binds the Herdr workspace namespace, pane
ID, and terminal/process generation; callers receive none of those provider
structures separately. Any non-secret workflow correlation placed in a launch
is chosen by the ADR-0006 necessity round and written by launch composition,
never treated by runtime as authority.

### Observe

- inspect current process/runtime observation;
- wait for a requested observation with a deadline;
- subscribe/read events with a resumable provider cursor where supported;
- read a bounded text/ANSI/detection view for diagnosis;
- explain uncertainty without claiming a domain transition.

Normalized observations include starting, running, idle, blocked, exited, unavailable, and unknown. They are timestamped and provider-attributed.

An observer/watchdog profile consumes this interface through authorized core use cases. Adding a named watchdog that observes existing events is configuration/authored-policy work; it does not add a runtime type.

A watchdog is a normal spawned Herdr-managed agent session. It is not a runtime daemon, timer, or hidden sweep loop.

### Control

- deliver a prompt atomically where the provider supports it;
- send a bounded signal/input operation;
- request graceful stop, then explicit forced termination according to policy;
- reconnect/re-associate a known handle after restart, and recover a possibly-created session after an ambiguous launch from the pre-known `(launch subject, correlation)` pair validated **together** — a correlation alone never rebinds a session to another workflow identity, and the recovered startup-delivery fact is `Ambiguous` unless the adapter holds explicit durable or provider-supplied proof of the startup submission (the provider issues no receipt for it after a lost response);
- distinguish not found, stale handle, provider unavailable, rejected, timeout, and ambiguous outcomes.

Herdr is the complete live transport. ABACUS adds no inbox, acknowledgement,
delivery queue, or retry layer. A critical instruction is first represented as
an append-only fact in the shared `br` store; Herdr may then carry transient
content or a bounded doorbell. Delivery never proves the fact was read or that
a workflow transition occurred.

Runtime control does not update workflow state directly. Composition records
durable results through the shared-`br` work/workflow interface.

## Identity rule

An ABACUS actor/attempt identity and a Herdr runtime handle are different facts.

- If the necessity round retains a durable association, the shared `br` record
  attached to the relevant work/Attempt owns it.
- Herdr may move panes between workspaces/tabs without changing domain identity.
- A restored pane ID with a new terminal/process generation is stale/unknown until explicit re-association; pane ID reuse cannot silently attach to another Attempt.
- Named manager/watchdog profiles are never encoded in pane-option conventions.

## Status and completion

Herdr may observe agent status through integration signals or screen manifests. That is operationally useful but fallible.

- `blocked` can trigger an alert.
- `exited` can trigger reconciliation.
- `idle` is advisory and cannot by itself authorize new work.
- `done`-like output cannot accept a handoff.

Only a core handoff use case, with durable evidence and decision authority, completes work.

## Provider contract

The provider lock records:

- Herdr exact version/release/commit and checksum;
- socket/protocol or CLI schema/capability fingerprint;
- supported agent kinds and required operations;
- fixture-set version;
- status-source assumptions used by ABACUS.

The initial adapter targets Herdr's high-level CLI/JSON facade for workspace,
start, prompt, wait, read, and bounded pane operations. The bundled API schema
is fingerprinted for diagnosis. Source verification shows the current
high-level start/prompt shapes do not yet provide an argv-free startup Envelope
path. The old ADR-0003 answer—an authenticated `runtime-rpc` composer loading
facts from Scribe—is superseded with the state design. The exact startup path
is therefore held for the necessity round/live pilot rather than smuggled in
as a surviving exception. Any later host composer must be a narrow runtime
composition justified by Herdr evidence; it cannot restore state credentials,
caller authority, or a second store. A client-supplied arbitrary `LaunchSpec`
remains outside an agent-facing command.

**Namespace scoping (operator decision, 2026-08-05; identifier derivation
reopened by ADR-0006).** Each repository confines launched agents to a
dedicated Herdr workspace inside the operator's existing session. Composition
supplies a collision-resistant repository label from project configuration;
there is no Scribe-minted repo ID. Orchestrator/operator panes live outside the
worker workspace. A separate named session remains rejected because nested
session attach makes the fleet unwatchable from the operator's actual session.

Three consequences remain binding. Two repositories cannot collide in one
session. Teardown scopes to the workspace; ABACUS never stops/deletes a named
session or installs provider integrations/plugins. Session-level events rotate
terminal generations; stale handles remain stale until explicit observation or
re-association selected by the necessity round. Workspace containment is an
operational boundary, never workflow identity.

Herdr v0.7.5 did not relocate named-session state with config/socket overrides, so disposable spike lanes use an exact disposable namespace with pre/post manifests rather than claiming full path redirection.

The compatibility spike must verify:

- stable handle semantics;
- Claude and Codex launch/resume behavior;
- atomic prompt delivery to idle and busy agents;
- wait/event subscription and cursor behavior;
- bounded output reads;
- process exit and forced stop;
- Herdr/server and composition restart recovery;
- pane-ID restoration with changed terminal/process generation;
- named-session isolation, exact cleanup, and sandboxed CLI/socket access;
- behavior when status detection is uncertain;
- no destructive collision with existing global provider configuration.

ABACUS does not install Herdr's global Claude/Codex hooks during initial repository setup. Any integration experiment redirects agent configuration roots into a disposable environment.

## Dependency rule

`abacus-runtime` depends only on `abacus-core` within ABACUS. It cannot import state, work, or CLI modules. It accepts correlation values and returns opaque handles/observations; the caller persists them.

## Evolution and blast radius

| Change | Expected validation |
| --- | --- |
| Internal Herdr protocol/command refactor with same normalized outcomes | Runtime tests |
| Pinned Herdr upgrade with same normalized interface | Runtime contract tests plus live Herdr compatibility lane |
| Add/split a named manager or watchdog using existing runtime capabilities | Profile/card/config tests; no runtime code |
| Add a new normalized runtime operation | Runtime plus direct core/use-case and CLI composition tests |
| Change runtime observation semantics | Direct consumers; ADR if breaking |

A Herdr change does not run work/state live tests. Runtime fixtures remain private to this module.

## Test contract

Default tests use a fake socket/protocol peer and captured minimal fixtures. They cover:

- exact launch specification mapping and environment allowlisting;
- successful and failed prompt delivery;
- content-free Signal-doorbell delivery after a fake durable commit;
- status/event mapping and unknown fields;
- deadline, disconnect, partial response, and ambiguous outcome behavior;
- stable/reused/missing handle protection;
- same-pane/new-terminal generation fencing after restart;
- reconnect/re-association after restart;
- bounded output and redaction;
- graceful/forced stop policy;
- multiple named profiles without hard-coded role names.

No default test launches Herdr, tmux, Claude, Codex, a PTY, or user-global integration. The live compatibility lane is explicit and uses disposable sessions/configuration.

Warm hermetic target: under fifteen seconds on the baseline development machine.

## Acceptance criteria

- Core and authored roles cannot observe a Herdr-specific type.
- A runtime event never directly completes an assignment.
- A new named manager/watchdog can use existing runtime capabilities without Rust changes.
- A provider restart produces recoverable known/unknown outcomes rather than false completion.
- A pinned Herdr change affects only runtime tests and its compatibility lane when the normalized interface is unchanged.
- No tmux command or `@sable_*` metadata exists in the implementation contract.
