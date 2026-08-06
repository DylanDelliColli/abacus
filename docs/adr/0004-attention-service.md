# ADR-0004: The attention service — a derivation, not a daemon

- **Status:** Proposed, revision 1 — pending Codex cross-review and operator sign-off as named decider
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
attention(state, work, now, policy) -> AttentionReport
```

No I/O, no timers, no clock of its own, no state of its own. It reads
workflow state, computes the set of currently-owed actions, and returns
them. It is ordinary deterministic Rust with ordinary unit tests, and it is
the entire correctness surface of this module. Everything else in this ADR
is plumbing around it.

This is what satisfies the boundary reading's hard requirement. A pure
function cannot corrupt or invent workflow state because it cannot write.

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
age of the record that produced it — a Signal's commit time, a Handoff's
submission time, a lease's expiry. When that age crosses a policy threshold
the obligation is classed for the operator instead of the worker. This is a
pure function of `(record_timestamp, now, policy)`. No ladder, no stored
stage, nothing to get out of sync.

`AttentionPolicy` carries the thresholds as ordinary values passed to the
derivation. It is not a new config schema and introduces no file format.

### 4. The recurring tick is external to ABACUS

ABACUS ships the derivation and one command that runs it. It does not ship
the thing that calls the command on a schedule. A systemd timer, a cron
entry, or an I16-shaped Herdr-managed profile invokes `abacus attend` every
N minutes.

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

| Condition | Audience | Query |
|---|---|---|
| Unresolved Signal — a Directive, Report, or Request with no linked responding action | the Signal's recipient; operator once aged past threshold | `unresolved_signals()`, exists |
| Pending Handoff — submitted for acceptance, neither accepted nor rejected | the deciding actor; operator once aged | `pending_handoffs()`, **new** |
| Reclaimable lease — an Attempt whose lease expired | operator always | `reclaimable_leases(now)`, **new** |

Reclaimable leases are operator-class unconditionally: the presumed-dead
worker is by definition not going to answer a doorbell, and reclamation is a
fenced decision per §2.

Two new **read-only** state queries are required. They are a C1 seam
extension in `abacus-core`'s state port plus `abacus-state` implementations,
and they add no mutable surface. Both are named here as the bead requires.

**Explicitly excluded** (operator decision, same date): ready work with no
worker assigned. That is scheduling, not attention. Admitting it would make
this module responsible for throughput, and throughput pressure is what turns
an observer into a decider.

### 6. Delivery outcomes are audit events, not a new store

The runtime seam already returns a typed result from `doorbell()`. Each
attempt is recorded on the existing audit trail with its normalized outcome —
submitted, not delivered, or ambiguous — which preserves the diagnostic value
the original brief wanted without a second store to own.

That metadata is strictly diagnostic. It is never read back by the
derivation, never affects whether an obligation is owed, and never claims
that a Signal was read, understood, discharged, or resolved. A `Submitted`
doorbell means one prompt left the building. `HandleStale` is expected during
reclaim, is not an error, and does not stop the run.

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
| Duplicate and ambiguous deliveries are harmless and retried | The derivation is idempotent and content-free; a duplicate nudge is a no-op |
| Stale generations never target the wrong Attempt | The runtime seam is generation-fenced and already returns `HandleStale` |
| Herdr or service outage catches up | The next run recomputes; nothing was queued to be lost |
| Unresolved state keeps re-ringing despite `Submitted` | `Submitted` is never read by the derivation (§6) |
| Resolved state stops ringing with no mutable ack | The obligation ceases to derive; there is nothing to clear |

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
  outcomes audited, and **zero durable workflow mutations** (§2). The
  no-mutation assertion is the one this module cannot ship without.
- A run against a stale handle records `HandleStale` and completes the
  remaining obligations rather than aborting.
- Running the derivation twice over unchanged state produces an identical
  report, and the second run's doorbells change no workflow state.

## Alternatives considered

**Stateful attention service as originally briefed** — per-obligation
delivery records, bounded backoff, throttling, an escalation ladder.
Rejected: it buys diagnosis and pacing we can obtain from durable timestamps
and the existing audit trail, at the cost of a new store, its migrations, and
its drift. I19 names escalation-on-silence machinery as a marker of mail, and
this is the one exception in the design worth refusing outright.

**On-demand only, no recurring check** — ship the command, never schedule it.
Rejected by the operator on 2026-08-05 and again on 2026-08-06: it is a
status tool, not a reliability floor, and nothing finds a stalled fleet
unless a human already suspects one.

**An agent watchdog as the floor** — rejected in §4; it can idle, which is
the failure it exists to detect.
