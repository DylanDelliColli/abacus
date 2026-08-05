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
| `cargo test -p abacus-core` | 88 (full domain through transport seam hardening R5.6-R5.29: aggregate acceptance, activation-consuming authorize, launch-subject credential binding, typed projection saga, call-identity idempotency) | 0.20s | 2026-08-05 | ABACUS-9NH.5/.6, d9fcea7 |
| `cargo test -p abacus-state` | 1 (scaffold placeholder) | 0.12s | 2026-08-04 | ABACUS-9NH.1 scaffold; row refreshed by ABACUS-9NH.7 when the migration foundation lands |
| `cargo test -p abacus-work` | 60 (31 unit: facade, id seam, scope labels; 29 contract suites: work, id seam, scope labels) | 0.12s | 2026-08-05 | ABACUS-omw.1/.3/.4; contract target: under fifteen seconds |
| `cargo test --workspace` | 152 across 9 targets | 0.38s | 2026-08-05 | ABACUS-omw.1/.3/.4 |
