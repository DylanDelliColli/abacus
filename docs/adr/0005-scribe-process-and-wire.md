# ADR-0005: Scribe process lifecycle and private wire

- **Status:** **Withdrawn** (2026-08-07); never accepted or implemented.
- **Date:** drafted 2026-08-06; withdrawn 2026-08-07
- **Decider:** operator (Dylan Delli Colli)
- **Superseded by:** ADR-0006
- **Companions:** ADR-0003 (superseded transport history),
  `docs/compatibility/2026-08-07-ledger-write-boundary.md`, and beads
  `ABACUS-9NH.8` / `ABACUS-9NH.9`

## Context

The draft specified explicit Scribe start/status/stop, a process-ownership
lock, socket lifecycle, readiness signaling, repository identity, a guarded
decision surface, a bounded framed protocol, Unix clients, and crash recovery.
It reached a coherent revision after cross-review but was never signed and no
source implemented it.

Before acceptance, implementation-contact review found no requirement for a
resident process: no push, timer, connection pool, shared in-memory state, or
process-owned truth. The socket and its Codex relay were costs of the selected
form factor rather than product capabilities.

## Withdrawal

The proposal is withdrawn in full. ADR-0006 subsequently removed the premise
as well as the process: stock `br`, selected through one shared absolute
`BEADS_DIR`, is the only durable store. There is no one-shot replacement state
RPC; agents access the trusted-local store directly.

The following machinery is deleted rather than deferred:

- resident lifecycle, ownership lock, readiness/PID/status/stop protocol, and
  stale-socket recovery;
- socket, framing, private request schema, Unix clients, and protocol
  negotiation;
- repository-ID and decision-capability publication files; and
- transport-only state error variants and migration obligations.

Two findings remain useful history:

1. current `SqliteState` loads an in-memory snapshot in its constructor and
   never refreshes reads, so it was safe only under a singleton owner; and
2. runtime-handle compare-and-swap concerns are real Herdr lifecycle concerns,
   but their eventual storage shape must be decided against ADR-0006 rather
   than smuggled through this withdrawn wire.

Reintroducing any resident ABACUS service requires a measured capability gap
and a new ADR. This document grants no implementation authority.
