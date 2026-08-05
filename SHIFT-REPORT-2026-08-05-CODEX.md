# Shift report — 2026-08-05 (Codex lineage)

## Late-shift addendum

This addendum supersedes the earlier current-state and next-action sections
below; those sections remain as the mid-shift record.

- `aa5c151` and `74287d4` received the final Codex C3 cross-review PASS after
  checking the normative effect-provenance amendment, the core/work contracts,
  the use-case paths, and the stacked state consumer.
- That review exposed one cross-transaction race outside the Claude-owned
  commits: a receipt candidate could be read before a later Close superseded
  its MarkInProgress projection, then be recorded after the supersession.
  `278b1ac` closes the race by re-deriving supersession atomically during
  receipt validation. Replay still wins for a receipt that genuinely committed
  first. The portable state suite and SQLite restart lane prove the refusal is
  mutation-free and survives reconstruction.
- The complete stack is green: 249 tests across 15 targets, workspace clippy
  with `-D warnings`, formatting, and diff checks. `abacus-i90` may close and
  `abacus-3ju` may proceed under its already-ACKed dedicated journey-crate
  placement.
- The next joint pre-freeze design item is `abacus-bcm` case B, the explicit
  true-divergence application-resolution disposition. ADR-0003 remains an
  operator decision; do not infer sign-off from completed HPG.5 evidence.

## Current state

`origin/main` includes the core/state/runtime work landed today, including the
SQLite S2 persistence unit (`c78fc6b`) and Claude's runtime unit (`6fe52cd`,
gyh.1). The working tree still contains Claude's next uncommitted work and
documentation; do not stage or commit without coordinating the claim window.

## Completed and reviewed

- Core anomaly-port and state seam work were reviewed and landed.
- `abacus-runtime` gyh.1 passed the repaired ambiguity review: an ambiguous
  startup delivery records exactly one material `(Envelope, secret)` pair,
  and recovery never redelivers it.
- The runtime namespace amendment passed review. Production agents use a
  repo-derived Herdr workspace label (`abacus-workers-<repo-id>`) inside the
  operator's existing session; teardown is workspace-scoped and identity is
  generation-fenced, never inferred from workspace containment.
- ADR-first placement for abacus-i90/3ju passed: add a core use-case module,
  generic over existing provider-neutral ports, with no provider knowledge,
  duplicated lifecycle policy, hidden retries, or background repair.

## Probe status and isolation

HPG.5 loaded/absent-rule controls and the dispatcher prototype remain in
progress. The disposable probe workspaces/panes were closed. The current
Herdr snapshot has only the original ABACUS panes (`w1:p1` Claude and
`w1:p2` Codex); no runtime probe may use the shared production socket.

The runtime-rpc leg is blocked until the operator starts a second Herdr server
from a plain shell outside this session with its own socket and session
directory. Before probing, verify that the new socket's session manifest has
zero workspaces; v0.7.5 evidence says socket/config overrides did not relocate
session state, and that isolation property remains unverified for v0.8.0.

The temporary dispatcher rules have already been removed. The rules file is
confirmed restored to baseline SHA-256
`3a0682587b80e79a4e3a28c57f635c537f1314a0ec68c1b98aacf47037964a93`.
Do not re-add or remove those lines blindly.

## Next safe actions

1. Claude lands the ADR amendment for i90, then implements the core use-case
   module TDD with full workspace fan-out.
2. Review that source as C2/C3: verify all four journey paths use the same
   production orchestration functions and that compensation/reconciliation
   are explicit outcomes.
3. Finish HPG.5 only against the isolated second Herdr server, recording
   provider-side secret residency and the runtime ambiguous-delivery burn
   proof. Then restore the rule file and delete disposable artifacts.
4. Do not run gated Git commands while Claude's claim window is active.

## Standing constraints

Never use `bd`, `sable-*`, or `HERDR_ENV=1`. Do not edit global configuration.
Scribe remains a recorder, Herdr remains the runtime provider, and the core
use-case module must not become a provider adapter or a second policy engine.
