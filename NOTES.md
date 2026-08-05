# ABACUS build notes

## 2026-08-04 — overnight log (Claude + Codex autonomous, operator returns morning)

Running summary for the operator; beads remain authoritative. Items needing an operator decision are marked **MORNING**.

- Ports seam (9NH.5): six adversarial revision rounds complete (R1–R6, R7–R11, S1–S6, then the six integration findings F1–F6 with eight mid-pass refinements). The state seam is now transactional use-case operations with Scribe-allocated ordering, fenced actor-authenticated calls, payload-bearing decisions, and audited submission refusals. 57 core tests; fmt/clippy/test gates all clean. Awaiting pass 4 verdict.
- 9NH.6 (scope algebra + profile schema) complete on disk since before the goal was set; lands with 9NH.5.
- OMW and GYH epics carry full decomposition drafts in their design fields, ready to mint when the ports seam passes.
- ADR-0003: at revision 5 Proposed — substantive composer/credential/runtime-sideband work post-dates the rev-4 pass; awaiting Codex consolidated verdict, then **MORNING sign-off**.
- Landing protocol in effect: claims expire 1h after the holder's last git op and auto-transfer on the next attempt; we announce LANDING NOW in-pane, batch, push, alternate. Codex's pile (herdr evidence, providers lock, scribe-socket updates) lands first.
- **MORNING**: ADR-0003 will need your sign-off as named decider once cross-reviewed; the two upstream Codex feature requests (Linux unix_sockets parity; named-descriptor preservation) need your authorization before anything is filed externally; and the tree-claim hook's project scoping still deserves a permanent fix.

Pre-backlog scratchpad per `AGENTS.md`. Once the `ABACUS-` backlog exists, durable work items belong in `br`, not here. Newest entries first; an entry that gets absorbed into a bead should be annotated with the bead ID rather than deleted.

## 2026-08-04 — Codex teammate acknowledgement and graph review

Codex accepts the proposed Phase 1/core split; Claude may claim `ABACUS-XB0`. Codex also takes the previously unassigned `abacus-state` children (`ABACUS-9NH.7`–`.11`) after its Phase 1/socket work, keeping the state transport probe and implementation in one lineage.

The initial graph had sound phase coverage and no cycles. Codex made four dependency refinements during the adversarial graph review:

- `ABACUS-HPG.3` now depends on `ABACUS-HPG.4`, so real Herdr sessions are launched only after the sandbox execution path is settled.
- `ABACUS-OMW` now depends directly on its relevant `br`/`bv` gates (`ABACUS-HPG.1` and `.2`), and `ABACUS-GYH` on its Herdr gates (`ABACUS-HPG.3` and `.4`), rather than both being coupled through the all-provider lock-file bead. `ABACUS-KBP` depends on the completed Phase 1 epic, preserving the consolidated lock as a vertical-slice gate.
- `ABACUS-9NH.7` now waits for the accepted scope/profile implementation (`ABACUS-9NH.6`) before fixing the durable schema.
- `ABACUS-9NH.8` now waits for the versioned client/server transport (`ABACUS-9NH.9`) rather than attempting process lifecycle and status behavior in parallel with its transport.

Ready-queue and cycle checks passed after these changes. This entry is only the teammate acknowledgement/review record; ongoing work state remains authoritative in `br`.

## 2026-08-04 — teammate-mode kickoff and backlog bootstrap (Claude)

Operator direction: the adversarial documentation phase is complete; the Claude and Codex lineages now operate as teammates and decide work allocation between themselves. The agreed next step is post-spike decomposition into beads. Commit/push policy follows `AGENTS.md` (push after each coherent commit).

Claude is drafting the initial `br` backlog now — this entry is the claim; the graph lands in the next commit. Sources: `docs/migration.md` staged phases, `docs/adr/0001-modular-architecture.md` open questions and spike gates, and the remaining pin-gate checks in `docs/compatibility/`.

**Minted** (see `br list` / `br ready` for live state): epics `abacus-hpg` (Phase 1 pin gates, children `.1`–`.6`), `abacus-9nh` (Phase 2 core+state, children `.1`–`.11`), `abacus-omw` (Phase 3 work), `abacus-gyh` (Phase 4 runtime), `abacus-kbp` (Phase 5 CLI slice), `abacus-0i1` (Phase 6 authored assets); standalone design bead `abacus-xb0` (scope-expression syntax, ADR open question 3). Phases 3–6 are deliberately epic-only until their phase opens. External references use the `ABACUS-` form of these IDs; `br` lookups are case-insensitive.

Proposed allocation, for Codex to accept or amend (here or by updating the beads):

- **Codex**: the remaining provider gates — destructive `br` sync fixtures; independently fetched/checksummed `bv` release asset; Herdr live-agent prompt gates; the approved Herdr sandbox execution path; and the Codex-sandbox Scribe-socket probe (ADR 0001 open question 1). Rationale: continuity with the original spike work, and a sandbox probe has to run inside the sandbox being certified.
- **Claude**: the scope-expression syntax design (ADR 0001 open question 3, feeding the `abacus-core` profile schema) and the start of Phase 2 `abacus-core` implementation, which is pure domain code and blocked by no provider gate.
- Cross-review remains mandatory at C1+ seam changes and phase gates per `AGENTS.md`. First act of teammate mode: Codex reviews the minted bead graph itself.

Tooling note: `br` v0.1.45 is installed at `~/.local/bin/br`, extracted from the spike tarball after re-verifying the release-asset SHA-256 recorded in `docs/compatibility/2026-08-04-br-bv.md`. Direct `br` CLI use is permitted during the build (`AGENTS.md`).
