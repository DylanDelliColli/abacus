# ABACUS build notes

Pre-backlog scratchpad per `AGENTS.md`. Once the `ABACUS-` backlog exists, durable work items belong in `br`, not here. Newest entries first; an entry that gets absorbed into a bead should be annotated with the bead ID rather than deleted.

## 2026-08-04 — teammate-mode kickoff and backlog bootstrap (Claude)

Operator direction: the adversarial documentation phase is complete; the Claude and Codex lineages now operate as teammates and decide work allocation between themselves. The agreed next step is post-spike decomposition into beads. Commit/push policy follows `AGENTS.md` (push after each coherent commit).

Claude is drafting the initial `br` backlog now — this entry is the claim; the graph lands in the next commit. Sources: `docs/migration.md` staged phases, `docs/adr/0001-modular-architecture.md` open questions and spike gates, and the remaining pin-gate checks in `docs/compatibility/`.

Proposed allocation, for Codex to accept or amend (here or by updating the beads):

- **Codex**: the remaining provider gates — destructive `br` sync fixtures; independently fetched/checksummed `bv` release asset; Herdr live-agent prompt gates; the approved Herdr sandbox execution path; and the Codex-sandbox Scribe-socket probe (ADR 0001 open question 1). Rationale: continuity with the original spike work, and a sandbox probe has to run inside the sandbox being certified.
- **Claude**: the scope-expression syntax design (ADR 0001 open question 3, feeding the `abacus-core` profile schema) and the start of Phase 2 `abacus-core` implementation, which is pure domain code and blocked by no provider gate.
- Cross-review remains mandatory at C1+ seam changes and phase gates per `AGENTS.md`. First act of teammate mode: Codex reviews the minted bead graph itself.

Tooling note: `br` v0.1.45 is installed at `~/.local/bin/br`, extracted from the spike tarball after re-verifying the release-asset SHA-256 recorded in `docs/compatibility/2026-08-04-br-bv.md`. Direct `br` CLI use is permitted during the build (`AGENTS.md`).
