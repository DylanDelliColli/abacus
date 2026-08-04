//! Domain language and pure rules for ABACUS.
//!
//! The binding contract is `README.md` in this folder. This crate owns
//! identifiers, the two authority classes, assignment/attempt lifecycles,
//! leases and fencing, typed Signals, evidence semantics, and the
//! provider-neutral ports. It is deterministic: no I/O, no clock or ID
//! generation of its own, and no knowledge of SQLite, subprocesses,
//! `br`, `bv`, or Herdr.

#![forbid(unsafe_code)]

#[cfg(test)]
mod tests {
    // Scaffold placeholder proving the per-module hermetic test target
    // (`cargo test -p abacus-core`) exists and runs. Replaced by real
    // domain tests as ABACUS-9NH.2 through .6 land.
    #[test]
    fn hermetic_test_target_runs() {}
}
