# ABACUS — Agent Instructions

Instructions for any agent session (Claude or Codex) working in this repository. `CLAUDE.md` is a symlink to this file; this file is the authority.

## What this repository is

ABACUS: a local, bead-led orchestration system being built as a clean successor beside legacy SABLE. You are *building* the product here, not operating under it. The documentation is the contract:

- `CONTEXT.md` — domain language and invariants. **Normative.** A conflict with any other document is a defect to fix, never an override.
- `docs/adr/0001-modular-architecture.md` — foundational decisions, provider seams, spike gates.
- `docs/architecture.md` and `docs/migration.md` — system flows and the phased build plan.
- `abacus-<module>/README.md` — the binding contract for any module you touch. Read it before editing.

## Do not act as a legacy SABLE agent

Global configuration on this machine leaks legacy SABLE machinery into this repository. None of it applies here:

- **Never use `bd`** (legacy beads) in this repo. The build backlog lives in `br` with the `ABACUS-` prefix once initialized. Direct `br` CLI use is permitted during the build — CONTEXT I2 governs the product's runtime agents, not the build process.
- **Never use `sable-note`** — it routes observations into legacy SABLE's feedback queue. Pre-backlog discoveries go in `NOTES.md` at the repo root; once the backlog exists, file an `ABACUS-` bead instead.
- **Legacy hooks fire here.** The tree-claim hook triggers on git operations (release a stuck claim with `sable-claim release "$(pwd)"`); bead-quality/tdd-gate hooks key on `bd` commands. Commit with `-c core.hooksPath=/dev/null` when hook interference corrupts an operation. Do not attempt to "fix" legacy hooks from this repo.
- Never invoke `sable-*` tools, SABLE skills, or SABLE mode machinery for ABACUS work.

## Git rules

- Commit identity is repo-local and already configured: `Dylan Delli Colli <dylan.dellicolli@gmail.com>`. Do not change it; never commit with a Heartwood identity.
- The only remote is `git@github-personal:DylanDelliColli/abacus.git`. The `github-personal` SSH alias authenticates as DylanDelliColli; the machine's default key does **not** have write access. Never add other remotes.
- Keep history clean and reviewable: commit when a coherent unit lands; push after committing.
- Never install global hooks; never edit `~/.claude`, `~/.codex`, shell configuration, or global git config from this repo.

## Change discipline (build phase)

- Every change has a class (CONTEXT §7): **C0** internal → owning module's tests only; **C1** seam extension → plus direct consumers' contract checks; **C2** breaking seam or new cross-module dependency → ADR first; **C3** core → full workspace fan-out.
- Default tests are hermetic — no live `br`/`bv`/Herdr, no network, no user home. Live provider lanes run only on pin changes or explicit manual invocation.
- Test-budget growth and new cross-module dependencies are ADR-level events, not conveniences.
- Providers are pinned and checksummed (`docs/compatibility/` records; `.abacus/providers.lock.toml` once created). Never fork, vendor, or casually upgrade a provider.

## Working agreement

- Two agent lineages (Claude and Codex sessions) build and adversarially cross-review each other. Never edit a file the other is actively drafting; resolve material disagreements in the documents before writing code.
- Cross-review is required at C1+ seam changes and phase gates (migration.md acceptance criteria) — not on every C0 diff.
- Spike and compatibility evidence is checked in under `docs/compatibility/`. A spike that isn't recorded didn't happen.
- The red-green evidence-pair discipline (CONTEXT I4) governs the product's workers at runtime. For the build itself, each module contract's test contract is binding and the phase acceptance criteria gate completion.
