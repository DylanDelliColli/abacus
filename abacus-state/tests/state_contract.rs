//! Canonical in-memory implementation of the portable state contract.

use abacus_core::Timestamp;
use abacus_state::contract::{StateContractHarness, run_workflow_state_suite};
use abacus_state::{InMemoryState, ManualClock};

struct MemoryHarness {
    clock: ManualClock,
    state: InMemoryState<ManualClock>,
}

impl StateContractHarness for MemoryHarness {
    fn port(&self) -> &dyn abacus_core::ports::WorkflowStatePort {
        &self.state
    }

    fn set_now(&self, now: Timestamp) {
        self.clock.set(now);
    }
}

fn build(now: Timestamp) -> MemoryHarness {
    let clock = ManualClock::new(now);
    MemoryHarness {
        state: InMemoryState::new(clock.clone()),
        clock,
    }
}

#[test]
fn in_memory_state_passes_the_portable_contract() {
    run_workflow_state_suite(build);
}
