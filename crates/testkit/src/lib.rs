//! Test harness of the operator: the kit's envtest control plane with this
//! operator's CRDs, status polling, and the in-memory owned objects and
//! pod logs.

pub mod envtest;
pub mod polling;

pub use application::test_support::{FakeOwnedObjects, FakePodLogs};
