# `abacus-state` transitional contract

Status: superseded implementation; no new target behavior may be added

## Why this crate still exists

`abacus-state` implements the separate SQLite Ledger selected before
ADR-0006. It contains an in-memory engine, SQLite persistence/migrations,
portable contracts, and stored DTOs for Assignments, Attempts, leases,
Signals, Evidence, Handoffs, decisions, application attempts/receipts,
idempotency, audit, profile activation, and runtime associations.

ADR-0006 removed the product premise for that store. Pinned stock `br`,
selected through one shared absolute `BEADS_DIR`, is now the only durable
workflow store. There is no Scribe process, state protocol, second database,
or state-service lifecycle.

The crate remains temporarily because production composition and four journeys
still consume it. Deleting it before the shared-`br` replacement would create
a behavior hole. Existing source describes the superseded implementation, not
the target architecture.

## Binding transition rules

- Do not add state verbs, tables, migrations, transports, clients, lifecycle,
  credentials, authorization, reconciliation, or recovery machinery here.
- Do not adapt `SqliteState` for multiprocess direct writing. Its constructor
  cache assumes one long-lived owner; the design that needed it is gone.
- Do not delete live behavior ahead of replacement. The stock-`br` facade and
  displaced state consumers land in one cross-reviewed C3 stack.
- The two-store application-attempt, receipt, supersession, and pending-
  projection subsystem is obsolete in the target, but remains until that
  replacement stack.
- Runtime association remains a real Herdr concern, but its minimum durable
  shape is decided in ADR-0006's necessity round rather than extended here.
- Existing portable contracts remain regression evidence for behavior that
  survives; they are not proof every current type must survive.

The only source subtraction authorized before the necessity round is the
five-item inert authorization cluster in `abacus-core` (plus mechanical compile
fallout if any): `ValidatedProfileSet::authorize`, `AuthorizationTarget`,
`ActionContext`, `AuthorizationRefusal`, and producerless
`StateError::ScopeUnauthorized`.

## Transition validation

While the crate remains:

- changes that touch it run its portable memory/SQLite contract and restart
  tests;
- a seam or core change runs direct consumers and the full hermetic workspace;
- no live provider, socket, daemon, or user-global state enters default tests;
- no migration is added solely to delete data from a product that has never
  shipped; and
- replacement must keep the four journeys meaningful, removing only steps
  whose failure mode disappeared with the second store.

## Exit criteria

After the necessity round and thin shared-`br` facade land, this crate is
either removed or narrowed to a genuinely single-consumer codec/reducer that
the round explicitly justifies. Its current name, tables, and traits create no
presumption of survival.

See `CONTEXT.md`, ADR-0006, and `docs/migration.md` for the binding target and
sequence.
