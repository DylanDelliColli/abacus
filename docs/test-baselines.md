# Hermetic test-runtime baselines

Per `docs/migration.md` change-locality rule 9: every module records a
baseline, and unexplained test-runtime growth is rejected in review.
Update the row when a phase lands a real suite; growth beyond a module's
contract target (e.g. `abacus-core` warm target: under five seconds) is
an ADR-level event, never a reason for selection/caching machinery.

Times are warm-cache wall clock on the baseline development machine
(WSL2, cargo 1.93.0). Cold compile time is not tracked.

| Target | Tests | Warm wall time | Recorded | Note |
| --- | --- | --- | --- | --- |
| `cargo test -p abacus-core` | 1 (scaffold placeholder) | 0.16s | 2026-08-04 | ABACUS-9NH.1 scaffold |
| `cargo test -p abacus-state` | 1 (scaffold placeholder) | 0.12s | 2026-08-04 | ABACUS-9NH.1 scaffold |
| `cargo test --workspace` | 2 (scaffold placeholders) | 0.13s | 2026-08-04 | ABACUS-9NH.1 scaffold |
