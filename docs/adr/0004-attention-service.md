# ADR-0004: The attention service — a derivation, not a daemon

- **Status:** Proposed, revision 4 — Codex cross-review returned six material findings on revision 3; all six accepted and resolved here. Awaiting re-review, then operator sign-off as named decider.
- **Date:** 2026-08-06
- **Decider:** operator (Dylan Delli Colli), on cross-reviewed proposal
- **Companions:** CONTEXT I6/I10/I12/I16/I17/I19, ADR-0001 §8 (typed Signals), bead `ABACUS-IKQ`, `abacus-runtime/README.md` (doorbell verb)

## Context

A Signal commits durably before anything is delivered. The recipient learns
it exists on its next facade activation, or when a content-free Herdr
doorbell nudges it to look. That ordering is already built: `doorbell()`
exists on the runtime seam, carries no content, is generation-fenced, and
returns `HandleStale` rather than misdelivering.

What is not built is what happens when the nudge does not land. The prompt
may be lost (I6 says it may). The recipient's session may have exited. The
worker may be alive and simply stopped. In each case a durable obligation
sits in the Ledger and no one finds out. Correctness is unharmed — that is
I6 working as designed — but the fleet stalls silently, and a stall nobody
observes is indistinguishable from progress.

Closing that gap is a **liveness** requirement, and liveness needs something
that runs when nothing else is running. That collides with I12, which
forbids ABACUS-owned timers, watchers, sweepers, and polling daemons. The
operator-ratified boundary reading (CONTEXT, 2026-08-05) permits an
explicitly authorized operations module to provide liveness, arriving by its
own ADR and consuming existing seams. This is that ADR.

The boundary reading is an exception, and exceptions are a budget. The
design below spends as little of it as the requirement allows: the
reliability floor is a pure function, and the only thing that recurs lives
outside ABACUS entirely.

## Binding constraints

- **I12** — no ABACUS-owned resident process or loop beyond Scribe. The
  boundary reading permits the module; it does not permit a daemon inside it.
- **I12 (boundary reading)** — the module's absence or failure may delay
  progress but must never corrupt or invent workflow state.
- **I19** — no read/ack state anywhere. Resolution is the linked responding
  workflow action; "unresolved" is a derived query. The mail boundary names
  *escalation-on-silence machinery* as one of the three things that make a
  system mail.
- **I17** — every decision is fenced to an actor and scope, and audited.
- **I6** — nothing critical rides the transient channel; the doorbell stays
  content-free.
- **I10** — pending work is a query over records, not another mutable queue.
- **I16** — a watchdog is a spawned Herdr-managed profile, never a daemon.

## Decision

### 1. Attention is a derivation over workflow state

The reliability floor is one pure function:

```
attention(facts: AttentionFacts, now, policy) -> AttentionReport
```

No I/O, no timers, no clock of its own, no state of its own. It is ordinary
deterministic Rust with ordinary unit tests, and it is the entire
correctness surface of this module. Everything else in this ADR is plumbing
around it.

Revisions 1–3 wrote this signature as `attention(state, work, now, policy)`
and claimed the result was a pure function that could not write. Cross-review
established that the signature proved no such thing. `DecisionWorkflowStatePort`
carries the decision mutations alongside its reads, and `WorkGraphPort`
likewise carries mutations; a function handed either one is not provably
read-only, and it performs read I/O besides. The claim was true of the
intended behavior and unprovable from the types, which is the same defect
class as a document asserting provenance the code cannot supply.

`AttentionFacts` fixes it by construction. It is an immutable plain value —
the unresolved Reports and Requests, the pending Handoffs, the reclaimable
leases, and the timestamps belonging to each — gathered by the caller and
handed in. The derivation holds no port at all, so it cannot read, cannot
write, and cannot reach a mutation verb even by accident. Gathering is
ordinary I/O and lives in the ring pass (§6), where it is honest about being
I/O.

The `work` parameter is deleted outright. It was left over from a draft that
included ready-work-without-a-worker, which §5 excludes; nothing in the
derivation could legitimately have used it.

### 2. The module holds no authority and never mutates

The attention module is **read-only with respect to workflow state**. It
cannot assign, launch, reclaim, accept, reject, or close anything. It rings
and it reports; those are its only two effects.

This is the load-bearing safety property, and it is a deliberate refusal of
a tempting feature. An unattended timer that could launch a session would be
an unfenced decision actor, which I17 forbids outright — and it would be
precisely the class of machinery that made the legacy system's failures
unattributable. When the correct response to an obligation is a decision, the
module surfaces it to a human or an authorized actor and stops. It does not
act on their behalf.

Consequence, stated plainly so nobody is surprised by it: a stalled worker
with no live session is **escalated, never auto-recovered**. Recovery is a
fenced decision and stays one.

### 3. Nothing is remembered between runs

The module persists nothing. It writes no table, keeps no per-obligation
counter, and records no last-delivery timestamp. Every run recomputes from
current workflow state.

Two things fall out, and they replace the three heaviest items in the
original brief rather than deferring them:

**Rate limiting is the tick interval.** A condition that is still true at the
next run is rung again. No backoff counter exists because the interval
already bounds the rate.

**Urgency is arithmetic on durable timestamps.** Every obligation carries the
age of the record that produced it. When that age crosses a policy threshold
the obligation is classed for the operator instead of the worker. This is a
pure function of `(record_timestamp, now, policy)`. No ladder, no stored
stage, nothing to get out of sync.

Where that timestamp comes from is not uniform, and revision 1 of this ADR
was wrong to imply it was. Only `Lease` carries wall-clock time
(`expires_at`). `Signal` carries a `Seq` — a commit *order*, not a time — and
`HandoffRecord` carries no temporal field at all. A threshold expressed in
minutes cannot be evaluated against a sequence number.

The resolution adds no field to any immutable record, because I10 already
guarantees the fact exists: every state-changing operation appends an audit
event, and `AuditEvent` carries both `seq` and `at`. Commit time is therefore
recoverable for every obligation without touching the records themselves,
which also keeps I10's split between current state and immutable record
intact. Concretely:

- **Reclaimable lease** — `expires_at` directly. No join.
- **Pending Handoff** — the new query returns the record together with its
  submission time, joined from the creating audit event inside `abacus-state`.
  `HandoffRecord` carries no `seq`, so the caller has no way to bound an audit
  query itself; doing the join in SQL is what keeps it bounded.
- **Unresolved Report and Request** — joined in memory against audit reads
  bounded by the minimum `seq` among the Signals just read. `Signal` carries
  its own `seq`, so the bound is exact and `unresolved_signals()` keeps its
  current signature.

Revision 3 said "one audit read" and specified `AuditClass::Signal` for it.
That was wrong: `AuditKind::class()` maps `ReportRecorded` to
`AuditClass::Report`, and only `DirectiveAppended` and `RequestAppended` to
`AuditClass::Signal`. A single Signal-class read would have returned no
Report events at all, so **every unresolved Report would have carried no age
and could never have been promoted** — the escalation path silently dead for
the class most likely to need it. Two class-scoped reads are required:
`AuditClass::Report` for Reports and `AuditClass::Signal` for Requests.

No pre-existing seam changes shape. The cost is two bounded audit reads per
run rather than the one claimed.

`AttentionPolicy` carries the thresholds as ordinary values passed to the
derivation. It is not a new config schema and introduces no file format.

### 4. The recurring tick is external to ABACUS

ABACUS ships the derivation and one command that runs it. It does not ship
the thing that calls the command on a schedule. A systemd timer or a cron
entry invokes `abacus attend` every N minutes.

Revisions 1–3 also listed "an I16-shaped Herdr-managed profile" here and then,
two paragraphs later, forbade an agent watchdog from being the floor. Those
cannot both stand: an I16 profile is an agent profile, so the option
contradicted the constraint. The timer is the specified mechanism. If a
non-agent supervised-process form ever exists, it qualifies on the same terms
— deterministic, unable to idle — but no such form is assumed here.

I12 therefore remains literally true: no ABACUS-owned loop exists. If the
timer is misconfigured, disabled, or dead, ABACUS loses nudges and loses
nothing else — the kernel never depended on it. The loop is one line of
configuration that an operator can read, not a subsystem with a lifecycle.

The tick must be **dumb**. It is a timer invoking a deterministic command,
never an LLM watchdog deciding when to look, because an agent watchdog can
itself idle and would reintroduce the failure it exists to detect. An agent
may sit *above* this floor and add judgment; it may not *be* the floor.

### 5. Three obligation classes, each declaring its own audience

Operator decision, 2026-08-06. Each class names who should act, so routing
is data on the obligation rather than branching in the ringer.

| Condition | Defined by | Audience | Query |
|---|---|---|---|
| Unresolved Report | no linked responding action | the owning Assignment's decision actor; operator once aged | `unresolved_signals(None)`, exists |
| Unresolved Request | no linked responding action | the named recipient actor; operator once aged | `unresolved_signals(None)`, exists |
| Pending Handoff | the Attempt is currently `Submitted` | the deciding actor; operator once aged | `pending_handoffs()`, **new** |
| Reclaimable lease | the Attempt is currently `Active` **and** `now > expires_at` | operator always | `reclaimable_leases(now)`, **new** |

**Directives produce no standing obligation, and this is deliberate.**
Revisions 2 and 3 both got this wrong, in opposite directions, and the second
correction was as mistaken as the first. Revision 3 claimed a global
`unresolved_signals(None)` would return Directives and route them to their
subject Attempt. It will not: `abacus_core::signal::unresolved` is documented
as "unresolved Reports and Requests" and returns `false` for
`SignalBody::Directive` in the core derivation itself, so the exclusion is
upstream of every recipient filter. No call to the existing query, with any
argument, yields a Directive.

The right response is not to extend that query. A Directive is **binding from
the moment it commits**, and every fenced worker response mechanically
surfaces the Attempt's current binding Directives — so a worker that is
working always sees it, with no attention machinery involved. Two paths
already cover the rest: the architecture rings a content-free doorbell when
the Directive commits, and a worker that never interacts again stops renewing
its lease, which surfaces as a reclaimable-lease obligation. Adding a
Directive-unresolved derivation would extend Signal semantics to duplicate
coverage that exists, which is precisely the growth I19 tells us to stop and
write an ADR about rather than absorb.

**Both new queries are defined by current state, not by an absent link.**
Cross-review found the earlier link-absence definitions unsound in a way that
breaks the central by-construction claim. A Handoff defined as "submitted with
neither Accept nor Reject" keeps ringing after a Cancel terminalizes the
Attempt without either decision. A lease defined by timestamp alone keeps
ringing forever once the Attempt is reclaimed, since the expiry stays true
after reclamation. Both would have violated "resolved state stops ringing"
while the ADR claimed that row held by construction. Keying on
`AttemptState::Submitted` and on `Active` + expired makes the row true. Expiry
is strict (`now > expires_at`), matching `Lease::is_expired`, and Reclaim is
permitted only from `Active`, matching the lifecycle.

Reclaimable leases are operator-class unconditionally — not because the worker
is presumed dead, which expiry does not establish, but because **Reclaim is a
fenced I17 decision**, the same reason §2 refuses auto-recovery. A live worker
may hold an expired lease; the module surfaces it and stops.

**Resolving a decision actor to a session needs one more query than revision 3
admitted.** `runtime_handle` takes a `LaunchSubject`, and the actor form
requires actor, profile, *and* activation generation. A Report or Handoff
yields a `DecisionActor` (actor and profile) but no generation; a Request
yields only an `ActorId`. Revision 3's claim that "a handle resolves from an
Attempt as readily as from an actor" was false. A third read-only query is
therefore required: **current activation generation for an actor**, from which
the `LaunchSubject` and then the handle follow. It must state its
missing-and-ambiguous behavior explicitly and must assume no singleton
orchestrator (I16). An obligation whose audience has no resolvable live
activation is surfaced to the operator rather than dropped.

Three new **read-only** state queries are therefore required, not two. They
are a C1 seam extension in `abacus-core`'s state port plus `abacus-state`
implementations, and they add no mutable surface. The growth is named here
rather than discovered during implementation.

**Explicitly excluded** (operator decision, same date): ready work with no
worker assigned. That is scheduling, not attention. Admitting it would make
this module responsible for throughput, and throughput pressure is what turns
an observer into a decider.

### 6. Delivery outcomes are returned, never persisted

The ring pass gathers `AttentionFacts`, resolves handles, calls the existing
content-free `doorbell()`, and returns each normalized outcome — submitted,
not delivered, or ambiguous — **in the run's report**. Nothing about delivery
is written anywhere.

Revisions 1–3 said these outcomes ride "the existing audit trail," and that
was not implementable as described. ADR-0001 defines audit as a transaction
index over a canonical durable mutation, not a standalone event store, and the
state seam exposes no append-audit verb at all. The nearest writer,
`record_runtime_observation`, demands a canonical observation record and a
reporter `AuthoritySnapshot` — which a no-authority timer cannot honestly
produce, and which does not describe a doorbell outcome in any case. Worse,
writing them would have made §2's no-mutation guarantee false and turned the
audit index into exactly the delivery store §3 claims to have cut.

So the diagnostics stay in the report. An operator reading a run sees every
delivery outcome; nothing accumulates a history the module then owns. If
durable delivery history is ever genuinely needed, it arrives as its own
designed record with a canonical form, an idempotency identity, and honest
initiator provenance — a deliberate amendment, not a field that grew.

A `Submitted` doorbell means one prompt left the building and nothing more.
`HandleStale` is expected during reclaim, is not an error, and does not stop
the run. A later tick is **fresh reconciliation from a still-durable
obligation**, never a retry authorized by an earlier ambiguous result: the
runtime contract is explicit that ambiguity never licenses a blind retry, and
the module never consults a prior outcome to decide anything.

### 7. Accepted v1 non-goals

- No external notification channel. Escalation reaches a terminal and a live
  operator session; it does not send email, SMS, or push. If the operator is
  away from every session, nothing reaches them, and that is a known limit of
  v1 rather than an oversight.
- No auto-launch, auto-reclaim, or any other automated recovery (§2).
- No worker-side acknowledgement of any kind (I19).
- No scheduling or work distribution (§5).

## Consequences

Most of the acceptance proofs demanded by `ABACUS-IKQ` are satisfied **by
construction** rather than by machinery, which is the strongest argument for
this shape:

| Required proof | How it holds |
|---|---|
| Crash after Signal commit, before ring, recovers on restart | No ring state exists to lose; the next run recomputes from the Ledger |
| Duplicate and ambiguous deliveries are harmless and retried | The derivation is idempotent and the nudge is content-free. A duplicate nudge is **not** literally a no-op — it can prompt an agent to act again, and that action can produce a distinct Report. The honest claim is narrower: the nudge carries no authority, and any action it provokes still passes the ordinary fencing and idempotency gates that guard every worker write |
| Stale generations never target the wrong Attempt | The runtime seam is generation-fenced and already returns `HandleStale` |
| Herdr or service outage catches up | The next run recomputes; nothing was queued to be lost |
| Unresolved state keeps re-ringing despite `Submitted` | `Submitted` is never read by the derivation (§6) |
| Resolved state stops ringing with no mutable ack | The obligation ceases to derive; there is nothing to clear. For Reports and Requests this is verified rather than assumed: `unresolved_signals` computes over signals joined to their response actions with no stored flag, pinned by `abacus-state`'s existing `unresolved_signals_are_derived_from_responses`. For Handoffs and leases it holds **only** under §5's current-state definitions — the link-absence definitions of revisions 1–3 left a cancelled Handoff and a reclaimed Attempt ringing forever |

Costs, stated honestly:

- Nudge rate is coarse. A stuck worker is nudged on every tick until the
  condition clears. The nudge is content-free and the worker is stuck, so
  the noise is judged acceptable; if it proves otherwise, §3 is the thing to
  revisit, and doing so means accepting durable state we do not have today.
- Escalation is bounded by the operator's presence (§7).
- The `abacus-cli` crate is contract-only today, so the command in §4 lands
  when the CLI does. The derivation does not wait on it.

## Validation and acceptance obligations

- The derivation is unit-tested against fabricated state with no I/O: each
  obligation class derives when owed, does not derive when resolved, and is
  promoted to operator-class exactly at the policy threshold.
- An integration test drives real composition — real state, fake runtime
  peer — and asserts the full run: obligations derived, doorbells attempted,
  outcomes returned, and **zero durable writes of any kind** (§2, §6). That
  assertion is the one this module cannot ship without, and it is now
  checkable rather than aspirational: the derivation holds no port, and the
  ring pass writes nothing.
- Each obligation class has an explicit **absence** case, because the
  by-construction table depends on them: a cancelled Handoff produces no
  obligation, a reclaimed or revoked Attempt produces no lease obligation, and
  a lease exactly at `expires_at` produces none either (expiry is strict).
- A run against a stale handle records `HandleStale` and completes the
  remaining obligations rather than aborting.
- Running the derivation twice over unchanged state produces an identical
  report, and the second run's doorbells change no workflow state.
- Age derivation is tested per class against the audit trail (§3), including
  that a Report's age is read from `AuditClass::Report` and a Request's from
  `AuditClass::Signal`. Promotion happens at the threshold and not before.
  This mechanism replaced the escalation ladder, so it carries the ladder's
  test burden — and a class-mismatch bug here silently disables promotion
  rather than failing loudly, which is why it is tested per class.
- Actor-to-handle resolution (§5) is tested for its missing and ambiguous
  cases, not only the happy one: an obligation whose audience has no
  resolvable live activation is surfaced to the operator, never dropped.

## Normative amendments this ADR requires on acceptance

This ADR is the authorized I12 crossing, but authorization does not let the
binding documents keep asserting the opposite of what ships. Two statements in
CONTEXT become false the moment fixed-cadence re-ringing with age promotion
exists, and both must be amended **in the accepting commit**, not afterwards:

- **I6** currently states that v1 "deliberately has no durable mail, inbox,
  acknowledgement, or delivery-retry machinery." Re-ringing on a cadence is
  delivery follow-up. The amendment must keep the prohibitions that still
  hold — no inbox, no acknowledgement, no read state, no durable delivery
  queue — while naming this module's content-free, stateless re-ring as the
  authorized exception, with the I12 boundary reading as its warrant.
- **The failure table** states that an unresolved Signal whose recipient is
  inactive gets "no escalation machinery." Age promotion to operator-class is
  escalation, deliberately bounded and stateless. The row must say so.

The standard here is the one this lineage applied to ADR-0003's provenance
weakening: a document that claims something stronger than the code delivers is
a defect, whichever direction the gap runs. An ADR that ships escalation while
CONTEXT denies escalation exists is that defect.

## Alternatives considered

**Stateful attention service as originally briefed** — per-obligation
delivery records, bounded backoff, throttling, an escalation ladder.
Rejected: it buys diagnosis and pacing obtainable from durable timestamps and
run-local reporting, at the cost of a new store, its migrations, and its
drift. I19 names escalation-on-silence machinery as a marker of mail, and this
is the one exception in the design worth refusing outright.

**Extending the unresolved-Signal derivation to cover Directives** — rejected
in §5. A Directive is binding from commit and is surfaced mechanically on
every fenced worker response; the commit-time doorbell and lease expiry cover
the rest. Extending Signal semantics to duplicate existing coverage is what
I19 tells us to stop and write an ADR about, not absorb quietly.

**On-demand only, no recurring check** — ship the command, never schedule it.
Rejected by the operator on 2026-08-05 and again on 2026-08-06: it is a
status tool, not a reliability floor, and nothing finds a stalled fleet
unless a human already suspects one.

**An agent watchdog as the floor** — rejected in §4; it can idle, which is
the failure it exists to detect.
