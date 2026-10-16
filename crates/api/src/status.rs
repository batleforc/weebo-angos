//! The status every CRD of this operator shares.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
pub use weebo_kit_api::{BoundInstance, Condition};

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    /// The `metadata.generation` the last successful reconcile applied.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub observed_generation: Option<i64>,
    /// The instance this object was resolved to and stays bound to: a
    /// `spec.instanceRef` by name only never silently moves to another
    /// instance created later under the same name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instance: Option<BoundInstance>,
    /// `Ready` and the advisories, carrying one of the operator's reason
    /// codes.
    #[serde(default)]
    pub conditions: Vec<Condition>,
    /// The Angos tag the instance's ready pods run.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    /// Where the instance answers in the cluster: `<service>.<namespace>.svc:<port>`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub endpoint: Option<String>,
}
