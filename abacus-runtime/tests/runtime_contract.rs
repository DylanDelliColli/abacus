//! The portable runtime contract, run over the hermetic fake peer.
//!
//! The facade under test is the production `RuntimeFacade`; only the
//! provider is fake. The `gyh.2` live Herdr lane implements the same
//! harness's compatible subset against real sessions.

use std::rc::Rc;

use abacus_core::Timestamp;
use abacus_core::ports::{ClockPort, RuntimePort};
use abacus_runtime::RuntimeFacade;
use abacus_runtime::contract::{RuntimeContractHarness, run_runtime_suite};
use abacus_runtime::fake::{FakeFailure, FakeRuntimePeer, StartRecord};

#[derive(Clone)]
struct FixedClock(Timestamp);

impl ClockPort for FixedClock {
    fn now(&self) -> Timestamp {
        self.0
    }
}

struct FakeHarness {
    peer: Rc<FakeRuntimePeer>,
    facade: RuntimeFacade<Rc<FakeRuntimePeer>, FixedClock>,
}

impl RuntimeContractHarness for FakeHarness {
    fn port(&self) -> &dyn RuntimePort {
        &self.facade
    }

    fn rotate_generation(&self, correlation: &str) {
        self.peer.rotate_generation(correlation);
    }

    fn set_raw_status(&self, correlation: &str, raw: &str) {
        self.peer.set_status(correlation, raw);
    }

    fn arm(&self, failure: FakeFailure) {
        self.peer.arm(failure);
    }

    fn accepted_prompts(&self, correlation: &str) -> Vec<String> {
        self.peer.prompts(correlation)
    }

    fn startup_deliveries(&self, correlation: &str) -> Vec<(String, String)> {
        self.peer.startup_deliveries(correlation)
    }

    fn stops(&self, correlation: &str) -> Vec<bool> {
        self.peer.stops(correlation)
    }

    fn starts(&self) -> Vec<StartRecord> {
        self.peer.starts()
    }
}

fn build() -> FakeHarness {
    let peer = Rc::new(FakeRuntimePeer::new());
    FakeHarness {
        peer: peer.clone(),
        facade: RuntimeFacade::new(peer, FixedClock(Timestamp(77)), "abacus-rt-test"),
    }
}

#[test]
fn the_runtime_facade_passes_the_portable_contract_over_the_fake_peer() {
    run_runtime_suite(build);
}
