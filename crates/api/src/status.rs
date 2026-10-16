//! The status of the operator's CRDs.

use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
pub use weebo_kit_api::{BoundInstance, Condition};

/// The status every CR bound to an instance shares.
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
}

/// The status of an `AngosLocalInstance`: what runs, and where the last
/// configuration stands.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct InstanceStatus {
    /// The `metadata.generation` the last successful reconcile applied.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub observed_generation: Option<i64>,
    /// `Ready` and the advisories, carrying one of the operator's reason
    /// codes.
    #[serde(default)]
    pub conditions: Vec<Condition>,
    /// Where the instance answers in the cluster:
    /// `<service>.<namespace>.svc:<port>`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub endpoint: Option<String>,
    /// The Angos tag the ready pods run.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub available_replicas: Option<i32>,
    /// The hash of the startup layer the pods run.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub boot_hash: Option<String>,
    /// The startup layers the instance ran, newest first: their `Secret`s
    /// (`<name>-boot-<hash8>`) are kept for a rollback, the older ones
    /// removed.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub boot_history: Vec<String>,
    /// A startup layer that changes the storage, waiting for the annotation
    /// `angos.weebo.io/confirm-storage` set to this value.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pending_boot_hash: Option<String>,
    /// The hash of the hot layer desired.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub config_hash: Option<String>,
    /// The hash of the last hot layer every ready pod confirmed loading.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub applied_config_hash: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_applied_time: Option<String>,
    /// When the hot layer being confirmed was written (RFC 3339): the logs
    /// are read from then.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub config_written_time: Option<String>,
    /// The hash the hot layer had when written (what `configWrittenTime`
    /// is about).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub written_config_hash: Option<String>,
    /// The last hot layer a pod refused, and why (keys and paths only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rejected_config_hash: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rejection_message: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_rejection_time: Option<String>,
    /// Each pod's answer while a hot layer is being confirmed.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pods: Vec<PodReload>,
    /// The repositories bound to the instance and what became of each
    /// (`namespace/name`): rendered, or why not.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub repositories: BTreeMap<String, RepositoryRender>,
    /// The repositories that left the configuration, and when
    /// (`namespace/name`): a prune waits `pruneLock.windowHours` after.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub released_repositories: BTreeMap<String, String>,
    /// The identities bound to the instance and what became of each
    /// (`namespace/name`).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub identities: BTreeMap<String, IdentityRender>,
    /// When the operator last rolled the pods for an unconfirmed reload.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rolled_out_for: Option<String>,
}

/// A pod's answer to a hot layer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct PodReload {
    pub name: String,
    pub state: PodReloadState,
    /// Since when (RFC 3339).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub since: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub enum PodReloadState {
    /// No answer yet.
    Pending,
    /// The pod loaded the layer.
    Applied,
    /// The pod refused it.
    Rejected,
    /// The pod started after the layer was written: it read it at start.
    Restarted,
}

/// What an instance made of a repository bound to it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct RepositoryRender {
    pub prefix: String,
    pub state: RenderState,
    /// Why it isn't rendered.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    /// The repository holding its prefix (`namespace/name`), in conflict.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conflict_with: Option<String>,
    /// The `metadata.generation` rendered.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub generation: Option<i64>,
    /// The `metadata.generation` every pod of the instance confirmed
    /// loading.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub applied_generation: Option<i64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub enum RenderState {
    /// In the configuration.
    Rendered,
    /// Its prefix overlaps an older repository's.
    Conflict,
    /// A Secret it names is missing.
    SecretMissing,
    /// A Secret it names doesn't opt in.
    SecretNotAllowed,
    /// Its spec can't be rendered.
    Invalid,
}

/// What an instance made of an identity bound to it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct IdentityRender {
    pub username: String,
    pub state: RenderState,
    /// The password version rendered.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<u64>,
    /// The password version every pod of the instance confirmed loading:
    /// what the identity's targets may get.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub applied_version: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conflict_with: Option<String>,
}
