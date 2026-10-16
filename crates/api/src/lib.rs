//! The CRDs of the operator. Pure types: `serde`/`schemars` derives and
//! the kit's shared field types, no client code. Doc comments become the
//! CRD schema descriptions (`crdgen` → `deploy/crd/`).
//!
//! The kinds' Rust types carry their kind's name (`AngosLocalInstance`,
//! the CRD's title); the workspace uses them through short re-exports
//! ([`LocalInstance`]).

pub mod config;
pub mod local_instance;
pub mod status;

/// API group shared by every CRD of this operator.
pub const GROUP: &str = "angos.weebo.io";

/// The version of every CRD of this operator.
pub const VERSION: &str = "v1alpha1";

/// The kit's common annotations under [`GROUP`]:
/// `angos.weebo.io/force-orphan` (release a CR without
/// cleanup), `allow-disruptive-update`, `import-pending`.
pub const ANNOTATIONS: weebo_kit_api::Annotations = weebo_kit_api::Annotations::new(GROUP);

/// The `app.kubernetes.io/managed-by` of every object the operator writes:
/// an existing object labelled with it (and the instance's name) is handed
/// over to the instance.
pub const MANAGED_BY: &str = "weebo-angos";

/// The label (`"true"`) a `Secret` must carry for the operator to read it
/// or the instance's pods to mount it (spec 2.8).
pub const CREDENTIAL_LABEL: &str = "angos.weebo.io/credential";

/// The operator's own annotations (spec 3.3).
pub mod annotations {
    /// On the pod template: the hash of the startup layer (what rolls the
    /// pods).
    pub const BOOT_HASH: &str = "angos.weebo.io/boot-hash";
    /// On an instance: the startup hash whose storage change is confirmed.
    pub const CONFIRM_STORAGE: &str = "angos.weebo.io/confirm-storage";
    /// On a repository: `"true"` accepts that its data is erased at the
    /// next prune once it is deleted.
    pub const ALLOW_DATA_DELETION: &str = "angos.weebo.io/allow-data-deletion";
    /// On an identity or an instance: a changed value rotates its secret.
    pub const ROTATE: &str = "angos.weebo.io/rotate";
    /// On a maintenance: a changed value runs it now.
    pub const RUN_NOW: &str = "angos.weebo.io/run-now";
    /// On an instance: the Angos tag allowed off the N → N+1 path.
    pub const APPROVED_UPGRADE: &str = "angos.weebo.io/approved-upgrade";
    /// On the pod template: when the operator rolled the pods (a reload
    /// that wasn't confirmed, `onUnconfirmed: Rollout`).
    pub const RESTARTED_AT: &str = "angos.weebo.io/restarted-at";
}

/// The Angos image the operator targets, and the versions it is tested
/// with (spec 6.8): its configuration model, validation, startup keys and
/// log messages are those of these versions.
pub mod angos {
    /// The image repository of Angos.
    pub const IMAGE_REPOSITORY: &str = "ghcr.io/project-angos/angos";
    /// The version targeted: the default tag.
    pub const VERSION: &str = "1.12.3";
    /// The tags an instance may run.
    pub const SUPPORTED_VERSIONS: &[&str] = &["1.12.3"];
}

pub use local_instance::LocalInstance;
pub use status::{InstanceStatus, Status};
