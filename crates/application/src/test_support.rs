//! In-memory adapters for use-case and controller tests (exposed to other
//! crates by the `test-support` feature): the kit's owned objects and pod
//! logs.

pub use weebo_kit_application::test_support::{FakeOwnedObjects, FakePodLogs};
