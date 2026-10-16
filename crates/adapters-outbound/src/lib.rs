//! Outbound adapters. Angos has no administration API: the operator talks
//! to it through the files it renders, the objects it owns, the pods' logs
//! and their status — the kit's `KubeOwnedObjects` and `KubePodLogs`.
//! What is the operator's own here: which instance a CR lives on
//! ([`LocalInstanceDirectory`]) and the settings tying the kit's adapters
//! to `AngosLocalInstance` ([`workload_settings`]).

pub mod cluster;
pub mod instances;

pub use cluster::{KubeClusterIssuer, KubeCredentials, RandomKeys};
pub use instances::LocalInstanceDirectory;
use weebo_kit_adapters::WorkloadSettings;

/// The field manager of every apply and status patch.
pub const FIELD_MANAGER: &str = "weebo-angos-operator";

/// What ties the kit's `KubeOwnedObjects` to the operator: its field
/// manager, its `managed-by`, `AngosLocalInstance` as the owner, the
/// credential label and the images a pod may run.
pub fn workload_settings(allowed_images: Vec<String>) -> WorkloadSettings {
    WorkloadSettings {
        field_manager: FIELD_MANAGER.into(),
        managed_by: api::MANAGED_BY.into(),
        owner_api_version: format!("{}/{}", api::GROUP, api::VERSION),
        owner_kind: "AngosLocalInstance".into(),
        credential_label: api::CREDENTIAL_LABEL.into(),
        allowed_images,
    }
}
