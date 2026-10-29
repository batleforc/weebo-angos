//! Use-cases and the ports they need. No concrete adapter and no
//! Kubernetes client here: adapters implement [`ports`] in
//! `adapters-outbound`, `operator::main` injects them.

pub mod admission;
pub mod cron;
pub mod identities;
pub mod maintenance;
pub mod ports;
pub mod reload;
pub mod render;
pub mod repositories;
pub mod rules;
pub mod status_patch;
pub mod upgrade;
pub mod use_cases;
pub mod workload;

#[cfg(any(test, feature = "test-support"))]
pub mod test_support;
