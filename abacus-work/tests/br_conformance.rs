//! The portable work-graph contract, run against the REAL br adapter.
//!
//! `run_work_graph_suite` drives `WorkFacade` over `BrWorkProvider`
//! backed by a stateful in-memory simulator whose argv surface and
//! response shapes replicate the recorded evidence in
//! `fixtures/br-v0.1.45`. Every portable expectation the fake provider
//! satisfies is therefore inherited by the real adapter code paths:
//! pin probe, revision bracketing, parse layer, error classification,
//! and the mutation `Err` contract.

use std::cell::RefCell;
use std::collections::BTreeMap;

use abacus_core::ContentHash;
use abacus_core::ports::{Eligibility, ObservedCloseReason, WorkError, WorkStatus};
use abacus_work::br_process::{BrObservation, BrRequest, BrRunError, BrRunner};
use abacus_work::br_provider::BrWorkProvider;
use abacus_work::contract::{Behavior, Scenario, run_work_graph_suite};
use abacus_work::to_provider;

const PINNED: &str = "br 0.1.45";

/// Deterministic test digest — NOT cryptographic; `abacus-omw.8` binds
/// the production primitive.
fn digest(preimage: &str) -> ContentHash {
    let mut acc: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in preimage.bytes() {
        acc = acc.wrapping_mul(0x0000_0100_0000_01b3) ^ u64::from(byte);
    }
    ContentHash::new(&format!("{acc:016x}").repeat(4)).expect("64 hex")
}

struct SimBead {
    status: String,
    close_reason: Option<String>,
    priority: u8,
}

/// A miniature br: stateful, argv-addressed, fixture-shaped output.
struct BrSimulator {
    beads: RefCell<BTreeMap<String, SimBead>>,
    tick: RefCell<u32>,
    behavior: Behavior,
}

fn ok0(stdout: String) -> Result<BrObservation, BrRunError> {
    Ok(BrObservation {
        exit_code: 0,
        stdout,
        stderr: String::new(),
    })
}

fn structured_error(
    exit_code: i32,
    code: &str,
    message: &str,
) -> Result<BrObservation, BrRunError> {
    ok0(String::new()).map(|_| BrObservation {
        exit_code,
        stdout: format!(
            "{{\"error\":{{\"code\":\"{code}\",\"message\":\"{message}\",\"hint\":null,\"retryable\":false,\"context\":null}}}}"
        ),
        stderr: String::new(),
    })
}

impl BrSimulator {
    fn from_scenario(scenario: &Scenario) -> Self {
        let (status, close_reason) = match scenario.status {
            // A parked scenario materializes the way `br` actually
            // expresses it: the issue is DEFERRED. The suite then
            // asserts identical facade behavior for the fake's flag and
            // the provider's real status, which is what makes the
            // parked expectations portable rather than fake-shaped.
            WorkStatus::Open if matches!(scenario.eligibility, Eligibility::Parked) => {
                ("deferred".to_owned(), None)
            }
            WorkStatus::Open => ("open".to_owned(), None),
            WorkStatus::InProgress => ("in_progress".to_owned(), None),
            WorkStatus::Closed { observed_reason } => (
                "closed".to_owned(),
                Some(match observed_reason {
                    ObservedCloseReason::AcceptedHandoff => "abacus:accepted-handoff".to_owned(),
                    ObservedCloseReason::CancelledObsolete => {
                        "abacus:cancelled-obsolete".to_owned()
                    }
                    ObservedCloseReason::UnrecognizedProviderReason => {
                        "closed out of band".to_owned()
                    }
                }),
            ),
        };
        let provider_id = to_provider(&scenario.bead).as_str().to_owned();
        let mut beads = BTreeMap::new();
        beads.insert(
            provider_id,
            SimBead {
                status,
                close_reason,
                priority: 1,
            },
        );
        Self {
            beads: RefCell::new(beads),
            tick: RefCell::new(scenario.tick),
            behavior: scenario.behavior.clone(),
        }
    }

    fn graph_hash(&self) -> String {
        format!("{:064x}", *self.tick.borrow())
    }

    fn bead_element(&self, id: &str, bead: &SimBead) -> String {
        let close = bead
            .close_reason
            .as_ref()
            .map(|reason| format!(",\"close_reason\":\"{reason}\""))
            .unwrap_or_default();
        format!(
            "{{\"id\":\"{id}\",\"title\":\"sim\",\"status\":\"{}\",\"priority\":{}{close},\"labels\":[]}}",
            bead.status, bead.priority
        )
    }

    fn apply(&self, verb: &str, id: &str, args: &[&str]) {
        let mut beads = self.beads.borrow_mut();
        let bead = beads.get_mut(id).expect("mutation targets a known bead");
        match verb {
            "update" => {
                bead.status = "in_progress".to_owned();
            }
            "close" => {
                bead.status = "closed".to_owned();
                bead.close_reason = args
                    .iter()
                    .find_map(|arg| arg.strip_prefix("--reason="))
                    .map(str::to_owned);
            }
            other => panic!("unexpected mutation verb {other}"),
        }
        *self.tick.borrow_mut() += 1;
    }

    fn mutate(&self, verb: &str, id: &str, args: &[&str]) -> Result<BrObservation, BrRunError> {
        match &self.behavior {
            Behavior::Fails(WorkError::Busy) => {
                structured_error(2, "DATABASE_ERROR", "Database error: database is busy")
            }
            Behavior::Fails(WorkError::NotFound) => {
                structured_error(3, "ISSUE_NOT_FOUND", "Issue not found")
            }
            Behavior::Fails(WorkError::ProviderUnavailable) => Err(BrRunError::Spawn),
            Behavior::Fails(other) => {
                panic!("suite scenario uses an unmapped provider error {other:?}")
            }
            Behavior::AmbiguousApplied => {
                // The write lands; the acknowledgement is lost.
                self.apply(verb, id, args);
                Err(BrRunError::DeadlineExceeded)
            }
            Behavior::AmbiguousLost => Err(BrRunError::DeadlineExceeded),
            Behavior::Normal => {
                self.apply(verb, id, args);
                let beads = self.beads.borrow();
                let bead = beads.get(id).expect("just mutated");
                ok0(format!("[{}]", self.bead_element(id, bead)))
            }
        }
    }
}

impl BrRunner for BrSimulator {
    fn run(&self, request: &BrRequest) -> Result<BrObservation, BrRunError> {
        let args: Vec<&str> = request.args.iter().map(String::as_str).collect();
        match args.as_slice() {
            ["--version"] => ok0(format!("{PINNED}\n")),
            ["sync", "--status", "--json"] => ok0(format!(
                "{{\"dirty_count\":0,\"jsonl_content_hash\":\"{}\",\"jsonl_newer\":false,\"db_newer\":false}}",
                self.graph_hash()
            )),
            ["show", id, "--json"] => match self.beads.borrow().get(*id) {
                Some(bead) => ok0(format!("[{}]", self.bead_element(id, bead))),
                None => structured_error(3, "ISSUE_NOT_FOUND", "Issue not found"),
            },
            ["ready", "--limit", "0", "--json"] => {
                let beads = self.beads.borrow();
                let open: Vec<String> = beads
                    .iter()
                    .filter(|(_, bead)| bead.status == "open")
                    .map(|(id, bead)| {
                        format!(
                            "{{\"id\":\"{id}\",\"title\":\"sim\",\"status\":\"open\",\"priority\":{}}}",
                            bead.priority
                        )
                    })
                    .collect();
                ok0(format!("[{}]", open.join(",")))
            }
            [verb @ ("update" | "close"), id, rest @ ..] => self.mutate(verb, id, rest),
            other => panic!("unexpected br invocation {other:?}"),
        }
    }
}

#[test]
fn the_portable_contract_suite_passes_against_the_real_br_adapter() {
    run_work_graph_suite(|scenario: &Scenario| {
        BrWorkProvider::new(
            BrSimulator::from_scenario(scenario),
            PINNED,
            digest as fn(&str) -> ContentHash,
        )
    });
}
