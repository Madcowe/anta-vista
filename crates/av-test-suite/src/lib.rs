//! Independent adversarial test suite for anta-vista
//!
//! Shared test fixtures, proptest generators, and attack scenario builders.
//! Complements existing per-crate happy-path tests with adversarial mindset.

pub mod fixtures;
pub mod generators;
pub mod attacks;
pub mod x0x_harness;

pub mod prelude {
    pub use crate::fixtures::*;
    pub use crate::generators::*;
    pub use crate::attacks::*;
    pub use crate::x0x_harness::{X0xDaemonConfig, skip_if_no_daemon, inject_gossip_payload, spawn_named_instance};
}
