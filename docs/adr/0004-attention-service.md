# ADR-0004: The attention service — a derivation, not a daemon

- **Status:** Proposed, revision 8 — Codex cross-review PASS at revision 7; **external SABLE review then returned four blockers and three corrections, all accepted and repaired here.** That review upheld all three cuts (fixed tick over persisted backoff, run-local outcomes over a delivery store, stateless age over a ladder) and found instead that the *delivery* path could run correctly while waking nobody. Awaiting SABLE re-review, then operator sign-off. One residual question is explicitly left to the operator (§5, unresolvable-audience sink).
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
leases, the timestamps belonging to each, and the **already-resolved audience
facts** each one implies. The derivation holds no port at all, so it cannot
read, cannot write, and cannot reach a mutation verb even by accident.

Facts are pre-joined deliberately. If `AttentionFacts` carried raw Reports and
Handoffs, the derivation would have to look up owning Assignments to learn who
should act — which means holding a port again, and the purity would be back to
being a promise. Whatever the derivation needs to name an audience is resolved
before it is called.

**Gathering happens outside `abacus-attention` entirely.** Revision 4 put it in
the module's ring pass, which left the module holding
`DecisionWorkflowStatePort` — an interface carrying every decision mutation.
The function was pure while the module was still handed the verbs §2 says it
cannot use, so the integration assertion proved current behavior rather than
unrepresentability. The composition root gathers the facts and passes them in;
`abacus-attention` never names a state port at all.

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

**Urgency is arithmetic on durable timestamps, and it is ADDITIVE.** Every
obligation carries the age of the record that produced it. When that age
crosses a policy threshold the obligation is marked **aged** in the report and
**keeps ringing its named actor exactly as before**. This is a pure function of
`(record_timestamp, now, policy)`. No ladder, no stored stage, nothing to get
out of sync.

Revisions 1–7 said an aged obligation was "classed for the operator instead of"
its named actor, and external review established that this inverted the
feature. §5 makes operator-class obligations report-only and §6 rings only
named-actor obligations, so crossing the threshold moved an obligation from
ringable to unringable: **more urgency produced less attention.** It also
falsified the Consequences row promising that unresolved state keeps re-ringing,
and weakened outage catch-up, since a long outage could surface as a printed
report and no bell at all.

The defect was emergent rather than authored. Age promotion arrived in revision
1 and report-only operator obligations in revision 5, each defensible alone;
nothing in either section showed what they did together. Additive severity
repairs it without a ladder, a stage, a counter, or any new authority —
escalation adds a signal, it never removes the only one that reaches anybody.

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

**But v1 acceptance owns activation and verification of that timer, and
revisions 1–7 did not.** Shipping a command plus a documented recipe means the
delivered product is the on-demand-only alternative this ADR explicitly
rejects, right up until some human separately creates and enables a unit.
Between those two moments there is no floor — only a document claiming one.

This is not a hypothetical. Legacy SABLE (`SABLE-5xz68`) staged timer units and
printed a human activation recipe. Nobody ran it. The units sat on disk for
months while the supposedly pane-independent liveness floor did not exist, and
nothing in the system was capable of noticing. The eventual repair was one
explicit self-verifying install command plus a check that asked the *running*
scheduler two questions: are you active, and do you target this repository.

ABACUS adopts that repair without owning a daemon. `systemd` or `cron` still
owns recurrence; I12 is untouched. What changes is that **one bounded
install/check path — or an equivalent `init`/`doctor` gate — is part of v1
acceptance**, and it interrogates the live scheduler rather than the filesystem.
A passing unit test proves a correct command; it does not prove a timer exists,
survived a reboot, or points at this repository.

The tick must be **dumb**. It is a timer invoking a deterministic command,
never an LLM watchdog deciding when to look, because an agent watchdog can
itself idle and would reintroduce the failure it exists to detect. An agent
may sit *above* this floor and add judgment; it may not *be* the floor.

### 5. Four obligation classes, each declaring its own audience

Operator decision, 2026-08-06. Each class names who should act, so routing
is data on the obligation rather than branching in the ringer.

| Condition | Defined by | Audience | Query |
|---|---|---|---|
| Unresolved Report | no linked responding action | the owning Assignment's decision actor; operator once aged | `unresolved_signals(None)`, exists |
| Unresolved Request | no linked responding action | the named recipient actor; operator once aged | `unresolved_signals(None)`, exists |
| Pending Handoff | the Attempt is currently `Submitted` | the deciding actor; operator once aged | `pending_handoffs()`, **new** |
| Reclaimable lease | the Attempt is currently `Active` **and** `now > expires_at` | the owning Assignment's `decision_actor` | `reclaimable_leases(now)`, **new** |

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

**Reclaimable leases ring the Assignment's decision actor**, and revisions 1–7
were wrong to make them operator-class. Reclaim is a fenced I17 decision — that
part stands, and it is why §2 still refuses auto-recovery — but the design
mistook "the module may not decide" for "there is nobody to tell." There is.
Every `AssignmentRecord` stores its exact current `decision_actor`,
`TransferAuthority` keeps it current, and `validate_decision_authority`
(`abacus-state/src/memory.rs:477`) accepts a Reclaim **only** from that exact
actor. So a known, named, authorized, ringable audience was already recorded
and the ADR routed it to a report nobody is required to read.

Ringing that actor performs no Reclaim and grants the module no authority. It
wakes the one actor already entitled to decide. Expiry still does not establish
death — a live worker may hold an expired lease — which is exactly why the
decision belongs to a human-directed actor rather than to this module.

**Resolving a decision actor to a session needs one more query than revision 3
admitted.** `runtime_handle` takes a `LaunchSubject`, and the actor form
requires actor, profile, *and* activation generation. A Report or Handoff
yields a `DecisionActor` (actor and profile) but no generation; a Request
yields only an `ActorId`. Revision 3's claim that "a handle resolves from an
Attempt as readily as from an actor" was false.

A third read-only query resolves an actor to its **current activations**,
returning profile *and* generation together — a generation alone is still
insufficient for a Request, which supplies no profile.

**The across-profile ambiguity rule applies only where the profile is genuinely
unknown.** Revision 5 applied zero/one/many to every audience, which discards a
disambiguator the obligation already carries: a Report, a Handoff, and a
reclaimable lease all yield a full `DecisionActor` including its profile. Under
the blanket rule, an actor active in the decision profile *and* one unrelated
profile would resolve as ambiguous and receive no ring at all — silence caused
by information we already had. So:

- **Report, Handoff, reclaimable lease** — resolve by actor **and the recorded
  profile**. A second unrelated activation is irrelevant and must not suppress
  the ring.
- **Request** — carries only an `ActorId`, so the zero/one/many rule across
  profiles genuinely applies here and nowhere else.

None means no live activation; many (Requests only) means the audience is
ambiguous. Neither is an error and neither is silently collapsed to a guess; an
obligation resolving to none or many is reported without a ring.

**What remains without a named audience, and why that is now a small set.**
Once leases ring their decision actor and age is additive, every obligation
class has a named, ringable audience. Only one residual case has none: an
obligation whose audience resolves to no live activation, or to many (§5's
zero/one/many). Those are reported and not rung.

That residual is honest only because it is small and transient. External review
established the general principle the hard way: **a report with no named
consumer is diagnosis, not escalation.** A liveness feature whose escalation
path is "text appears in output a human may later read" recreates the exact
failure this ADR exists to prevent — a stall nobody observes — relocated to a
different file descriptor. Revisions 5–7 made *every* operator-class obligation
report-only and so committed that error at scale; this revision confines it to
the unresolvable-audience case.

Two honest options remain for that residual, and **the operator chooses**: an
explicitly configured attention sink (I16 forbids *assuming* a singleton
operator, it does not forbid an operator naming a target), or accepting that
this case is diagnosis rather than a floor and saying so in the text. This ADR
does not decide it.

Three new **read-only** state queries are therefore required, not two. They add
no mutable surface, but they are **C3, not C1** — CONTEXT §7 classifies *any*
`abacus-core` change as C3 with full workspace fan-out, and these extend
`abacus-core`'s state port. Revisions 5–7 called them C1, which understated the
blast radius of the ADR's own proposal. The §6 generalization of the runtime
doorbell contract is likewise C3. The growth and its true class are named here
rather than discovered during implementation.

**Explicitly excluded** (operator decision, same date): ready work with no
worker assigned. That is scheduling, not attention. Admitting it would make
this module responsible for throughput, and throughput pressure is what turns
an observer into a decider.

### 6. Delivery outcomes are returned, never persisted

The ring pass takes the gathered `AttentionFacts`, wakes each **named-actor**
obligation — the audience may be an orchestrator as readily as a worker, since
Reports and Handoffs name deciding actors — and returns every normalized
outcome — submitted, not delivered, or ambiguous — **in the run's report**.
Nothing about delivery is written anywhere.

**The existing Signal doorbell cannot carry these obligations, and revisions
1–7 wrongly assumed it could.** `abacus-runtime`'s contract pins the delivered
text exactly: `"workflow signal available; query unresolved"`
(`abacus-runtime/src/contract.rs:318`). Pending Handoffs and reclaimable leases
are not Signals and are not returned by `unresolved_signals`, so a deciding
actor woken by that bell is told to run a query that **cannot reveal why it was
woken**. Directives are excluded from that query by design, so the same gap
applies to the commit-time bell §5 counted as existing Directive coverage: the
bell lands and points at a set that cannot contain the Directive. Safety still
holds — the next fenced response surfaces binding Directives — but the
*latency* path is not implemented by current bell semantics.

Two repairs are required, and both are **C3**:

1. **A generic attention wake.** The bell's pinned text generalizes from a
   Signal-specific instruction to a content-free attention wake naming no
   subject, id, or class — it says only that this actor has outstanding
   workflow attention and where to look. It stays content-free, one method, and
   generation-fenced; only its *meaning* widens from Signals to workflow
   obligations. The runtime contract test pinning the exact string moves with
   it.
2. **One deterministic recipient-side discovery path.** The wake must point at a
   single facade command returning *every* obligation class owed to the caller —
   unresolved Reports and Requests, pending Handoffs awaiting the caller's
   decision, and reclaimable leases on Assignments the caller decides for. A
   recipient must never be told to look somewhere that structurally cannot hold
   the thing that woke it.

This ADR does not assume an agent will helpfully run some other command after
being told to query unresolved. If the wake and the discovery path disagree,
the bell is noise with extra steps.

The ring pass holds exactly one narrow port: a **doorbell seam** with a single
method, ringing a resolved handle and returning the typed outcome. It does not
receive `RuntimePort`, which also carries launch and stop — a module that
cannot mutate workflow state but can start and kill sessions has not honoured
§2 in any meaningful sense. One method in, one outcome out; the module cannot
express any other effect.

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

- No external notification channel, and no operator ringing at all (§5).
  Operator-class obligations appear in the run's report and nowhere else. If
  the operator is not reading runs, nothing reaches them; that is a known
  limit of v1 rather than an oversight.
- No auto-launch, auto-reclaim, or any other automated recovery (§2).
- No worker-side acknowledgement of any kind (I19).
- No scheduling or work distribution (§5).

## Consequences

Several acceptance proofs demanded by `ABACUS-IKQ` are satisfied **by
construction**, and that is the argument for this shape — but the claim is
narrower than revisions 1–7 made it, and external review was right to narrow
it.

**What "by construction" actually covers is state-loss immunity and eventual
reconciliation, not within-run consistency.** `AttentionFacts` is assembled
outside the module through several independent state and audit reads, and each
SQLite read takes and releases its lock separately. A response, an authority
transfer, an activation rotation, or a handle rebind can commit between them. A
run may therefore ring once after an obligation resolved, or ring a former
actor. Generation fencing and content-free delivery make both harmless, and the
next run reconciles — but that is *eventual*, not a within-run guarantee, and
the table below should be read that way.

**The pure function is also not the whole correctness surface.** Fact
selection, audit joins, audience resolution, scheduler activation, and report
delivery can each drop an obligation while the derivation stays perfectly
deterministic. Purity buys reviewability and testability of the *decision*; it
does not buy correctness of everything feeding it. That is why §7's coverage
obligations are not optional extras.

No snapshot or transaction machinery is added for v1. A harmless duplicate ring
is a better trade than a consistency subsystem, and if evidence later shows the
extra rings are costly, that is the moment to revisit — not now.

| Required proof | How it holds |
|---|---|
| Crash after **Report or Request** commit, before ring, recovers on restart | No ring state exists to lose; the next run recomputes from the Ledger |
| Crash after **Directive** commit, before ring — **narrowed by operator ratification, 2026-08-06, confirmed against the corrected cost** | **Not by this module, by design.** With no standing Directive obligation the next run derives nothing. A worker that keeps renewing successfully recovers at its next renewal via `FencedResponse.binding_directives`, bounded by its renewal cadence. A worker that is alive but **not** renewing — hung, starved, stalled — recovers not at all: it crosses expiry into a reclaimable-lease obligation that, being operator-class, is report-only and rings nobody, and the Directive is never self-delivered. Explicitly not a next-tick guarantee and, in the second case, not a bounded one |
| Duplicate and ambiguous deliveries are harmless, and a later tick reconciles afresh | The derivation is idempotent and the nudge is content-free. A duplicate nudge is **not** literally a no-op — it can prompt an agent to act again, and that action can produce a distinct Report. The honest claim is narrower: the nudge carries no authority, and any action it provokes still passes the ordinary fencing and idempotency gates that guard every worker write |
| Stale generations never target the wrong Attempt | The runtime seam is generation-fenced and already returns `HandleStale` |
| Herdr or service outage catches up | The next run recomputes; nothing was queued to be lost. Catch-up **rings**, rather than merely printing, because age no longer converts an obligation into a report (§3) |
| Unresolved state keeps re-ringing despite `Submitted` | `Submitted` is never read by the derivation (§6), and **age never removes the ring** — severity is additive (§3). Revisions 1–7 falsified this row by reclassifying aged obligations into a report-only class |
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
- A run against a stale handle reports `HandleStale` and completes the
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
  resolvable live activation is reported, never dropped. **Regression case:** an
  actor active in the obligation's recorded profile *and* one unrelated profile
  still rings, because Report/Handoff/lease audiences resolve by actor **and**
  profile. Only an actor-only Request may resolve as ambiguous.
- **Age is additive, tested on both sides of the threshold.** One obligation
  below the threshold and the same obligation above it both ring their named
  actor; the aged one additionally carries the aged marker. A test asserting
  that an aged obligation stops ringing would be pinning the defect external
  review found.
- **The wake and the discovery path agree.** Whatever text the generalized bell
  delivers, the command it names returns every obligation class owed to the
  caller. A test drives one obligation of each class and asserts the woken
  actor's discovery call surfaces it — a bell pointing at a query that cannot
  contain the obligation is the defect, not the baseline.
- **Scheduler activation is verified against the live scheduler, not the
  filesystem.** The install/check path reports whether a recurring invocation is
  active *and* targets this repository. A test proving the command is
  well-formed does not satisfy this; the point is that a staged-but-never-
  enabled timer must be detectable as an absent floor.
- **A Directive produces no obligation**, asserted directly rather than left
  implicit in the absence of a test. A committed, undischarged binding
  Directive on an Active Attempt yields an empty report from this module. That
  case pins the operator's ratified narrowing so a later reader cannot mistake
  the gap for a bug and quietly "fix" it back into scope.

## Resolved scope decision: Directives (operator, 2026-08-06)

**The operator ratified the narrower scope (option 1 below).** Directives get
the commit-time doorbell and the renewal-carried binding set; the attention
service adds nothing for them; and the crash-window proof `ABACUS-IKQ` calls
non-negotiable is **formally narrowed to Reports and Requests**. The record of
the choice and its cost follows, because a narrowed promise that is not
written down is a promise quietly broken.

§5 removes the Directive obligation class on the grounds that existing
mechanisms cover it. Cross-review accepted the lease path as an honest
liveness bound but rejected it as a silent substitute for the crash-window
proof, and that objection stood — which is what made this the operator's
decision rather than the author's.

One correction to how both this ADR and that review characterized the fallback,
because it changes the size of the gap rather than the principle. Both said
recovery waits on "the worker's next fenced call — which never comes if the
worker is idle." That is incomplete. `renew_lease` returns a `FencedResponse`,
and every `FencedResponse` carries `binding_directives`, so **a responsive
worker that continues to renew successfully is handed the current binding
Directives on its next renewal** — directly, and without an operator in the
loop.

**That covers responsive workers only, and revision 6 wrongly generalized it to
every live worker.** Renewal delivers binding Directives only if the worker
actually invokes renewal; being alive does not imply renewing. A
worker that is alive but hung, starved, or stalled invokes nothing, sees
nothing, and crosses expiry without ever learning of the Directive. Revision 6
also asserted that "a worker that stops renewing has genuinely stopped," which
contradicts both this ADR's own §5 — a live worker may hold an expired lease —
and CONTEXT's statement that a worker may be alive and simply stopped. Expiry
establishes reclaimability, never death; that error has now been made three
times in this document and is called out here so the fourth reader does not
repeat it.

The honest exposure has two cases, not one:

- **A continuously renewing worker** recovers at its next *successful* renewal,
  bounded by its renewal cadence, and recovers by itself.
- **A live non-renewing worker** does not recover at all. It crosses strict
  expiry, becomes a reclaimable-lease obligation, and — because §5 makes
  operator-class obligations **report-only** — that obligation rings nobody. It
  waits in a run report until the operator reads it and takes a fenced
  decision. Nothing bounds that interval, and the Directive is never
  self-delivered.

The second case is strictly worse than revision 6 described, and it is worse
than it would have been before §5 removed operator ringing — an interaction
between two separately reasonable choices that no single section made visible.

The options were:

1. **Ratify the narrower scope — CHOSEN, and re-confirmed against the corrected
   cost.** Directives get the commit-time doorbell plus the renewal-carried
   binding set, and the crash-window proof is explicitly narrowed to Reports
   and Requests. Ships nothing new. The cost was first presented as "at most
   one lease-renewal interval of an alive worker's time," which was wrong — it
   holds only for a worker that keeps renewing. The operator was told the
   corrected worst case, including that a live non-renewing worker never
   self-recovers and ends in an unbounded report-only obligation, and confirmed
   the choice against it.
2. **Add a binding-Directive attention read.** A fourth read-only query
   surfacing Active Attempts with undischarged binding Directives, holding the
   proof for all three Signal types. Not free: "binding" includes Pause, and a
   legitimately paused worker must not ring every tick forever, so the query
   needs a defensible rule for which binding Directives constitute an unmet
   obligation versus a steady state. That rule, not the query, was the cost.

**What the narrowing actually costs, stated so no later reader mistakes it for
an oversight:** there is no next-tick guarantee for Directives, and for one
class of worker there is no guarantee at all. A worker that keeps renewing
recovers by itself at its next renewal. A worker that is alive but not renewing
never sees the Directive, and its expiry produces a report-only operator
obligation that rings nobody — recovery then waits on a human reading a run
report and issuing a fenced decision.

Option 2 was declined because its cost was a policy rule about which standing
instructions count as unmet versus a legitimate steady state, and rules of
exactly that shape are what accreted into the predecessor system's ceremony.

**The operator's stated reason for accepting it is the governing one, and it
outranks the specific arithmetic above** (operator, 2026-08-06): *start simple
and build a contingency if needed; avoid architecting solutions to problems we
have not actually faced.* The hung-worker case is real but hypothetical — it
has not been observed, and the worker it describes is already failing at its
actual job rather than merely at reading messages. Building the rule now would
mean paying a permanent complexity cost against a projection.

This is deliberately a **contingency, not a gap left open by accident.** Option
2 remains fully available: §5 forecloses nothing about it, the query would be
additive and read-only like the other three, and this section is its
specification. The evidence that would trigger it is concrete rather than
atmospheric — a live, non-renewing worker observed missing a Directive in
practice, even once. Absent that observation, the rule is not written.

If operating experience shows the renewal interval is too coarse in practice,
option 2 remains available and this section is its starting point. Reopening it
is an amendment with evidence, not a redesign.

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

A third gap is **pre-existing** and this ADR merely surfaces it, but it must be
closed in the same commit because §5 now depends on it. CONTEXT line 37 and
ADR-0001 §8 both define an unresolved Signal unconditionally as "a Signal
lacking its linked responding action." The implemented derivation covers only
Reports and Requests and excludes Directives by design, so both sentences are
already stronger than the query — before this ADR existed. `docs/architecture.md`
§4.6 and the `abacus-cli` unresolved-Signal contract inherit the same
imprecision. All four must distinguish **binding Directives**, which are in
force from commit and discharged by worker action, from **unresolved Reports
and Requests**, which are the derived set.

The standard here is the one this lineage applied to ADR-0003's provenance
weakening: a document that claims something stronger than the code delivers is
a defect, whichever direction the gap runs, and whether or not the current
change introduced it. An ADR that ships escalation while CONTEXT denies
escalation exists is that defect; so is a CONTEXT that promises a derived set
broader than the code computes.

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
