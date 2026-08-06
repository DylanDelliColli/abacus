# ADR-0003: Scribe agent transport and caller authority on Linux

- **Status:** **Accepted** (2026-08-06), revision 6 — Claude C2 cross-review PASS; operator sign-off as named decider.
- **Date:** 2026-08-04; revised 2026-08-06
- **Decider:** operator (Dylan Delli Colli), on cross-reviewed proposal
- **Companions:** `docs/compatibility/2026-08-04-scribe-socket.md` (provider evidence), ADR-0001 §3 (Scribe seam), ADR-0002 §5 (reachability is not authority), `abacus-state/README.md`, beads `ABACUS-HPG.7` and `ABACUS-719`

## Context

Scribe listens on a user-only Unix socket at
`$XDG_RUNTIME_DIR/abacus/<repo-id>.sock`, and agents reach durable state only
through it. Compatibility evidence establishes an asymmetric Linux transport
fact:

- the operative Claude carriage can open the socket directly;
- Codex 0.146.x under the ordinary Linux sandbox cannot create the required
  `AF_UNIX` connection, even when the socket path itself is readable.

The inherited-descriptor alternative crosses the raw Codex sandbox but does
not survive into model-issued commands. The command-scoped host relay does:
fresh direct and Herdr-launched Codex sessions successfully invoked an exact
operator-approved executable/subcommand, supplied one dynamic request on
stdin, received one typed response, and failed closed when that rule was
absent. The checked compatibility record contains the controls, discarded
rows, versions, artifact hashes, and cleanup evidence.

Transport is only half of the seam. Revision 5 made every worker an
authenticated bearer-credential holder. Adversarial review found that this
solved the wrong problem at high cost: launch secrets had to survive model
context compaction, stay unavailable to same-uid project children, rotate and
revoke, and cross both launch carriages without entering argv, environment,
the worktree, or transcripts. The resulting credential vocabulary had spread
through core, state, runtime, storage, protocol, and tests before any worker
call actually authenticated in the production journey.

The observed legacy failure class was different. Distributed machinery chose
or inherited caller identity from environment and role state; one path ignored
the worker marker and silently acted as a manager. Caller-asserted identity
therefore preserves the contamination failure even when cryptographically
authenticated: a bad launcher can faithfully authenticate the wrong asserted
principal. Scribe already owns the durable relation
`Attempt -> Assignment -> bound worker`. A worker call needs to identify the
Attempt it concerns; it must never choose the authority under which Scribe
records or authorizes that call.

## Binding constraints

1. **No compensation machinery.** No watcher, polling bridge, reconnecting
   wrapper, retry daemon, request-file queue, or second ABACUS-owned resident
   process is added (CONTEXT I12).
2. **One protocol, two injected carriages.** Direct UDS and host relay carry
   the same versioned request/response model. Carriage selection is injected
   configuration, never try-direct-then-fallback probing (I13).
3. **No request data in argv.** The relay is an exact two-token invocation of
   an operator-owned executable and fixed subcommand; it rejects every extra
   argument or flag.
4. **One request per process and connection.** Batching and shared reply
   channels are forbidden in v1. A future batching design is C2.
5. **No sandbox weakening.** No danger mode, broad filesystem/network grant,
   loopback transport, or repository-controlled executable surface is an
   alternative.
6. **Reachability is not authority.** Access to the socket or relay grants no
   decision authority. The worker and decision interfaces are separate at the
   core type seam and at the versioned protocol seam.
7. **No silent retry or fallback.** Denial, unavailability, malformed frames,
   unresolved callers, stale fencing, and ambiguous outcomes remain distinct
   typed results. A client disconnect never proves whether a transaction
   committed; explicit replay uses the same operation identity.

## Decision

The operator approved this revision with the explicit implementation-contact
expectation: "I'll approve the current version - I think it's going to be hard
to critique this any further without actually testing an implementation in
practice." Acceptance therefore authorizes implementation; it does not make
the text immune from evidence. An implementation finding that cannot satisfy
this decision honestly amends the ADR instead of acquiring a workaround.

### 1. One versioned protocol over direct and relay carriages

Claude uses the direct UDS carriage where the configured sandbox permits it.
Linux Codex uses the per-call host relay
`/operator/path/abacus scribe-rpc`. The relay is fixed-dispatched before
ordinary argument parsing, repository discovery, or project configuration. It
reads one bounded newline-terminated typed request from stdin, privately
applies Scribe's framing, writes one bounded typed response, and exits under a
fixed deadline. It is a typed Scribe client, not a byte relay and not a
general facade for host execution.

The exact relay command has two tokens and accepts no trailing material.
Codex prefix rules match prefixes, so the executable itself enforces that
closed argv shape. Request fields, repository identity, Attempt locator,
fencing token, operation identity, payload, socket path, executable path, and
environment overrides never appear in argv. The relay resolves the repository
socket only from a bounded repo ID plus its operator-injected runtime base;
arbitrary socket paths are unrepresentable.

Host approval is environment policy, not repository policy. The loaded-rule
probe proves propagation, while the absent-rule probe proves fail-closed
behavior before any ABACUS process, socket connection, or retry exists. A
host-rule denial is therefore an agent-boundary failure, not a `StateError`.

### 2. Worker calls name an Attempt, never an authority

ABACUS launch composition writes one non-secret Attempt locator into the exact
worker launch it creates. That locator is transport data, not a credential and
not an authority claim. It may be visible in the launched process environment;
no confidentiality property depends on it. The worker-facing facade obtains
it from injected launch configuration and carries it opaquely on both direct
and relay requests. The model supplies no ActorId, authority class, profile,
profile hash, capability, scope, credential, runtime handle, pane, terminal,
or generation selector.

There is exactly one writer of the locator: ABACUS launch composition, which
already knows the Attempt it is launching. There is exactly one semantic
resolver: Scribe. Relay, framing, and CLI layers transport the value but never
interpret it or derive authority from it. Repository configuration, current
working directory, provider pane names, inherited manager variables, and
model context are not identity evidence.

For every worker mutation Scribe resolves, in order:

1. the locator to one durable Attempt;
2. that Attempt to its Assignment;
3. the Assignment's recorded worker binding;
4. the current Attempt/Assignment lifecycle, Lease, and fencing token;
5. the requested worker verb and its ordinary core invariants.

Only the resolved binding supplies audit and Signal provenance. A caller field
that purports to select actor, profile, capability, scope, or decision
authority is unknown/forbidden; the wire decoder rejects unknown fields rather
than ignoring them, so such a field is never accepted even as a consistency
hint and never used. Missing, malformed, detached, unmapped, ambiguous, stale,
or generation-incoherent launch associations refuse loudly before mutation.
No environment, asserted identity, pane inference, or other fallback is tried.

The fencing token stays in every mutating worker call. Runtime-handle
generation fences provider-session association; the Lease token fences
workflow Attempt ownership and supersession. Neither substitutes for the
other. A well-formed call from an ended Attempt or terminal Assignment returns
the stale-fencing refusal, not the bundle-incoherence refusal: its identities
may agree even though its authority to mutate has ended.

### 3. Worker and decision interfaces are structurally separate

The state seam is split over the same in-memory and SQLite implementations:

- the **worker interface** contains only fenced Report, Evidence, Handoff,
  Abort-compliance, Lease-renewal operations, plus the explicitly selected
  reads a worker requires;
- the **decision interface** contains Assignment/Attempt openings, decisions,
  Directives and Requests, profile activation/deactivation, runtime-association
  composition, application attempts/receipts, and decision-side reads.

The worker RPC dispatcher receives only the worker trait. It cannot name or
invoke a decision verb; this is a type fact rather than a routing convention.
Worker-facing core use cases are likewise generic only over the worker trait,
so the separation holds before protocol dispatch as well as at it.
The authenticated, operator-started decision composer receives the decision
trait and holds the operator-granted decision capability. Scribe still checks
and records the exact decision actor, profile hash, capability, and scope on
every decision. Worker-side Attempt resolution never manufactures decision
authority.

The portable state contract may exercise an internal aggregate bound so both
implementations receive one behavioral suite. That aggregate is test
convenience only; it is never handed to worker composition or the worker
dispatcher.

### 4. Worker credential machinery is removed

Worker bearers do not exist in v1. Consequently:

- `CredentialProvisioning` is absent from `AssignmentOpening`,
  `AttemptOpening`, and `ActivationOpening`;
- `verify_launch_subject` and the credential-invalid,
  credential-binding-mismatch, and credential-revoked state errors are absent;
- `LaunchSubject::WorkerAttempt` contains only the Attempt, while
  `LaunchSubject::ActorActivation` contains actor, profile, and activation
  generation;
- `RuntimePort::launch` accepts `LaunchSpec` without an
  `EphemeralLaunchSecret` sideband;
- Assignment/Attempt termination and profile deactivation perform no
  credential rotation or revocation;
- no credential file, slot, digest, CSPRNG provisioning, constant-time
  comparison, launch-secret prompt, or transcript lifecycle remains.

Existing credential columns/tables may remain dormant until an otherwise
necessary schema migration touches them; their mere removal earns no migration
or deletion project. New state writes and semantic association keys never
depend on them. If an existing schema-version transition is required to keep
credential-bearing launch keys or versioned records readable, the state module
performs the smallest forward compatibility migration; that is compatibility
work, not credential-retention behavior.

Generic multi-orchestrator activation credential topology is deferred until a
second orchestrator exists. The v1 decision composer remains authenticated;
this ADR does not convert decision calls into caller-asserted authority.

### 5. Terminal/pane derivation is deferred, not prohibited

Receiver derivation from Unix peer process to controlling terminal to Herdr
pane was evaluated and rejected for the current two-carriage mechanism. The
faithful Codex relay uses nonempty `write_stdin`; installed Codex 0.146.1
therefore creates an inner PTY, starts a new session, and makes that inner PTY
the relay process's controlling terminal. It is not the outer Herdr pane.
Moreover the persisted `arh1|namespace|pane|generation` handle has no existing
OS-TTY-to-pane reverse index. The receiver would derive no pane or the wrong
terminal on the relay carriage.

This option remains deliberately reopenable as additive direct-carriage
hardening if provider behavior or the runtime adapter changes. It must never
replace or alter the public worker message shape, become a per-request Herdr
dependency, or introduce a fallback from failed terminal derivation to
caller-asserted identity.

### 6. Accepted v1 non-goal

ABACUS v1 is a trusted-repository, local single-user system. It does not defend
worker provenance against a hostile same-uid process that can read or copy
another worker's non-secret locator and fencing token. Such a process may forge
worker-side lifecycle calls and claims under the target Attempt. The Handoff
gate still rechecks its structural commit, scope, base, cleanliness, evidence
identity, and policy predicates, but it does not reproduce verification
commands. The decisive containment is structural: the worker interface cannot
reach Assignment, decision, Directive, profile, or application-authority
verbs. Defending same-uid worker provenance requires a stronger OS/provider
trust boundary and is outside v1; this ADR does not imply that mode bits,
environment opacity, or model obedience provide one.

## Consequences

- Model-context compaction loses no authentication material. A worker-facing
  facade re-reads non-secret launch configuration on each invocation.
- A bad launcher can omit or stale a locator, causing a loud refusal, but it
  cannot smuggle manager authority through worker request fields.
- Scribe remains the sole writer and authority resolver; transport layers stay
  stateless and per-call, so Scribe restart requires no reconnecting bridge.
- Direct and relay carriages remain asymmetric in reachability but identical in
  protocol and identity semantics.
- The type split makes adding a worker-reachable decision verb a deliberate
  core seam change with full C3 fan-out.
- Removing credentials deletes substantial core/state/runtime vocabulary and
  testing cost without weakening the authenticated decision surface.

## Validation and acceptance obligations

1. Preserve the passing direct, relay, Herdr-launched relay, loaded-rule,
   absent-rule, one-request/one-connection, bounded framing, and cleanup rows in
   the compatibility record. Credential-specific historical rows remain
   evidence of revision 5, not revision-6 requirements.
2. Contract-test identical worker request semantics on both carriages: locator,
   fencing token, operation identity, and payload only; actor/authority/profile/
   capability/scope and override fields refuse before mutation.
3. Contract-test the trait separation: a worker dispatcher cannot express a
   decision call, while the authenticated decision composer records exact
   decision authority.
4. Add the contamination regression: manager identity variables deliberately
   exist in a worker launch; the worker Report is attributed through the
   Assignment binding; manager decision and manager-wide query/dispatch verbs
   are unavailable on the worker interface.
5. Preserve portable in-memory/SQLite parity for locator resolution, replay,
   stale fencing, terminal Assignment/Attempt refusal, and audit provenance.
6. Prove `Passed` Evidence is emitted by the execution adapter after observing
   the subprocess and cannot be asserted directly through the worker command
   interface. State honestly that Handoff rechecks structural evidence binding
   and policy but does not rerun every command; rerun only when policy requires
   it or execution outcome is missing/ambiguous.
7. After C2 cross-review and operator signature, update architecture, migration,
   module contracts, compatibility conclusions, and the HPG.5/HPG.7 bead state
   before implementing the breaking seams.
