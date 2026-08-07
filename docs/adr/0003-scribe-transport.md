# ADR-0003: Scribe transport and caller authority

- **Status:** **Superseded in full** (2026-08-07) by ADR-0006.
- **Date:** 2026-08-04; accepted revision 6 on 2026-08-06; superseded
  2026-08-07
- **Decider:** operator (Dylan Delli Colli)
- **Companions:** ADR-0006,
  `docs/compatibility/2026-08-04-scribe-socket.md`, and rationale bead
  `ABACUS-9WJ`

## Historical decision

Accepted revision 6 specified one versioned Scribe protocol over two injected
Linux carriages: direct Unix socket where available and an exact two-token
host relay for sandboxed Codex. It also replaced worker bearer credentials
with a non-secret Attempt locator, derived worker authority from durable
Assignment state, retained Attempt fencing, and separated worker and decision
verbs at the type and protocol seams.

The design was internally coherent and received cross-review PASS. Its
compatibility premise remains true: installed Codex on Linux cannot create the
required Unix-socket connection under its ordinary sandbox, while Claude can.
That evidence is historical provider evidence, not a current transport
requirement.

## Supersession

Implementation contact showed that the protocol existed only because ABACUS
had chosen a second database behind a resident process. The process supplied
no needed resident capability. The operator subsequently selected pinned
stock `br`, addressed through one shared absolute `BEADS_DIR`, as the single
durable store (ADR-0006).

Consequently ABACUS builds none of this ADR's transport or caller-authority
machinery:

- no Scribe process or Unix socket;
- no direct/relay carriage selection, relay command, framing, or protocol
  version;
- no public-versus-private state request envelope;
- no worker credential, decision guard, repository ID, or state-service
  authentication;
- no state dispatcher whose reachability must be divided into worker and
  decision surfaces; and
- no claim that local provider access is a security boundary.

The worker/decision type split, Attempt identity, leases, numeric fencing,
operation idempotency, and authority snapshots survive only if ADR-0006's
operator-required necessity round independently justifies them. Existing code
is not evidence enough.

## Reintroduction rule

This ADR is not dormant permission to restore a relay or daemon. A future
state transport requires a measured need that stock `br` plus the chosen data
shape cannot satisfy, a new ADR, and fresh compatibility evidence. There is no
fallback path from the ADR-0006 design to this one.
