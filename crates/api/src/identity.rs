use kube::CustomResource;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

pub use weebo_kit_api::{InstanceRef, LocalSecretKeyRef, SecretTarget};

use crate::status::Status;

/// The short names the rest of the workspace uses.
pub use self::AngosIdentity as Identity;
pub use self::AngosIdentitySpec as IdentitySpec;

/// A Basic identity of an Angos instance (`[auth.identity]`): a robot, a
/// CI, a scanner. The operator generates its password (or adopts one),
/// hashes it with Argon2id for the instance's configuration, and writes the
/// clear password to its targets once every pod of the instance accepts
/// it. Angos verifies a Basic password on every request (≈ 19 MiB and tens
/// of milliseconds each): workloads should rather use their
/// ServiceAccount's token (`auth.kubernetes`).
#[derive(CustomResource, Serialize, Deserialize, Clone, Debug, Default, PartialEq, JsonSchema)]
#[kube(
    group = "angos.weebo.io",
    version = "v1alpha1",
    kind = "AngosIdentity",
    plural = "angosidentities",
    shortname = "aid",
    namespaced,
    status = "IdentityStatus",
    printcolumn = r#"{"name":"Instance","type":"string","jsonPath":".status.instance.name"}"#,
    printcolumn = r#"{"name":"Username","type":"string","jsonPath":".status.username"}"#,
    printcolumn = r#"{"name":"Ready","type":"string","jsonPath":".status.conditions[?(@.type=='Ready')].status"}"#,
    printcolumn = r#"{"name":"Reason","type":"string","jsonPath":".status.conditions[?(@.type=='Ready')].reason"}"#,
    printcolumn = r#"{"name":"Age","type":"date","jsonPath":".metadata.creationTimestamp"}"#
)]
#[serde(rename_all = "camelCase")]
pub struct AngosIdentitySpec {
    /// The instance: unset, the only one of this namespace. An instance of
    /// another namespace must accept this one
    /// (`repositorySelector.namespaces`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instance_ref: Option<InstanceRef>,
    /// The username (`identity:<username>` in access rules). Default: the
    /// CR's name. Unique on the instance, never an OIDC provider's name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(regex(pattern = r"^[A-Za-z0-9._-]+$"))]
    pub username: Option<String>,
    /// Where the credentials are written (keys `username`, `password`, and
    /// `.dockerconfigjson` with `registryHost`). Unset: the Secret
    /// `<name>-credentials` of this namespace.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    // The keys each target holds, for the docs' CRD IDE (as `reconcile_identity::target_data` writes them).
    #[schemars(extend("x-weebo-keys" = ["username", "password", ".dockerconfigjson"]))]
    pub secret_targets: Vec<SecretTarget>,
    /// The registry's public host (e.g. `registry.example.com`): adds a
    /// `.dockerconfigjson` key to the targets.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub registry_host: Option<String>,
    /// A new password every this many days; the annotation
    /// `angos.weebo.io/rotate` (a new value) rotates it at once.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(range(min = 1))]
    pub rotation_days: Option<u32>,
    /// An existing password to keep (an identity moved from another tool),
    /// in a Secret of this namespace labelled
    /// `angos.weebo.io/credential: "true"`: read once, at creation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub adopt_secret_ref: Option<LocalSecretKeyRef>,
}

/// The status of an identity.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct IdentityStatus {
    #[serde(flatten)]
    pub common: Status,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub username: Option<String>,
    /// The password version written to the targets (1, then one more per
    /// rotation).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub password_version: Option<u64>,
    /// A password version the instance hasn't confirmed yet (written to
    /// the targets once it has).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pending_password_version: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_rotation_time: Option<String>,
    /// The identity holding the username instead (`namespace/name`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conflict_with: Option<String>,
}

/// The name of the identity's own Secret: its password, hash and version
/// (the operator's, never to edit).
pub fn password_secret(name: &str) -> String {
    format!("{name}-angos-password")
}

/// The keys of that Secret.
pub mod keys {
    pub const PASSWORD: &str = "password";
    pub const HASH: &str = "hash";
    pub const VERSION: &str = "version";
    pub const ROTATION: &str = "rotation";
    pub const ROTATED_AT: &str = "rotated-at";
}
