//! The runtime seam: the only ABACUS module that controls or inspects
//! the agent execution substrate.
//!
//! The binding contract is `README.md` in this folder. This crate
//! implements core's `RuntimePort` over an internal provider seam, so
//! provider session mechanics, generation identity, and detection
//! uncertainty stay inside this module. Callers reason about opaque
//! handles and normalized observations, never panes or terminals.
//!
//! Layering (ADR-0001 §3): depends on `abacus-core` only. It cannot
//! import state, work, or CLI, and it never persists anything — the
//! caller records durable facts through the state seam.

#![forbid(unsafe_code)]

pub mod adapter;
pub mod contract;
pub mod facade;
pub mod fake;
pub mod herdr;
pub mod target;

pub use adapter::{
    RawRunError, RawSessionIdentity, RawStartupDelivery, RawStatus, RuntimeProvider,
};
pub use facade::RuntimeFacade;
pub use fake::{FakeFailure, FakeRuntimePeer};
pub use herdr::{CommandError, CommandOutput, CommandRunner, HerdrPin, HerdrProvider};
pub use target::{AgentSnapshot, TargetRefusal, resolve_target};
