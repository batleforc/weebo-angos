use std::collections::BTreeMap;

use kube::CustomResource;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

pub use weebo_kit_api::InstanceRef;

use crate::status::Status;

/// The short names the rest of the workspace uses.
pub use self::AngosRepository as Repository;
pub use self::AngosRepositorySpec as RepositorySpec;

/// A repository of an Angos instance: the namespaces under its `prefix`,
/// who may pull, browse, push and delete there, where it caches from and
/// replicates to. Rendered into the instance's configuration by the
/// instance; `Ready` once every pod of the instance runs it.
#[derive(CustomResource, Serialize, Deserialize, Clone, Debug, Default, PartialEq, JsonSchema)]
#[kube(
    group = "angos.weebo.io",
    version = "v1alpha1",
    kind = "AngosRepository",
    plural = "angosrepositories",
    shortname = "arepo",
    namespaced,
    status = "RepositoryStatus",
    printcolumn = r#"{"name":"Instance","type":"string","jsonPath":".status.instance.name"}"#,
    printcolumn = r#"{"name":"Prefix","type":"string","jsonPath":".spec.prefix"}"#,
    printcolumn = r#"{"name":"Upstream","type":"string","jsonPath":".spec.upstreams[0].url"}"#,
    printcolumn = r#"{"name":"Ready","type":"string","jsonPath":".status.conditions[?(@.type=='Ready')].status"}"#,
    printcolumn = r#"{"name":"Reason","type":"string","jsonPath":".status.conditions[?(@.type=='Ready')].reason"}"#,
    printcolumn = r#"{"name":"Age","type":"date","jsonPath":".metadata.creationTimestamp"}"#
)]
#[serde(rename_all = "camelCase")]
#[schemars(extend("x-kubernetes-validations" = [
    {"rule": "size(self.upstreams) == 0 || (size(self.access.push) == 0 && size(self.access.delete) == 0)", "message": "a pull-through repository (upstreams) refuses every push: access.push and access.delete must be empty"}
]))]
pub struct AngosRepositorySpec {
    /// The instance serving it: unset, the only one of this namespace. An
    /// instance of another namespace must accept this one
    /// (`repositorySelector.namespaces`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instance_ref: Option<InstanceRef>,
    /// The namespaces it covers: `<prefix>` and `<prefix>/…` (an OCI
    /// namespace: lowercase segments, no quote). **Immutable**: changing it
    /// would move the data out of the repository (delete and recreate).
    #[schemars(regex(
        pattern = r"^[a-z0-9]+(?:(?:__|[._]|-+)[a-z0-9]+)*(?:/[a-z0-9]+(?:(?:__|[._]|-+)[a-z0-9]+)*)*$"
    ))]
    #[schemars(length(max = 255))]
    #[schemars(extend("x-kubernetes-validations" = [{"rule": "self == oldSelf", "message": "prefix is immutable: delete the repository and create another"}]))]
    pub prefix: String,
    /// `namespace`: the `?ns=` value it answers to as a mirror (Docker's
    /// mirror requests name the upstream registry).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mirror_of: Option<String>,
    /// Who may do what (`access_policy`).
    #[serde(default)]
    pub access: RepositoryAccess,
    /// Registries it caches from, tried in order (`[[upstream]]`): a
    /// pull-through cache, which refuses every push.
    #[serde(default)]
    pub upstreams: Vec<Upstream>,
    /// Registries it replicates to (`[[downstream]]`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub downstreams: Vec<Downstream>,
    /// `retention_policy`: CEL rules on `image`; a matching one keeps the
    /// image (OR with the instance's), `angos prune` deletes the others.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retention: Option<Rules>,
    /// `immutable_tags` (OR with the instance's).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub immutable_tags: Option<bool>,
    /// `immutable_tags_exclusions`: regexes. Not empty, it **replaces** the
    /// instance's list.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub immutable_tags_exclusions: Vec<String>,
    /// `index` policy: `default` (`index` or `skip`) and CEL rules on
    /// `image`; unset, the instance's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub index: Option<ImagePolicy>,
}

/// The vocabulary of weebo-si's charts: subjects `anonymous`,
/// `authenticated`, `provider:<name>`, `identity:<username>`,
/// `group:<group>`, `namespace:<namespace>`,
/// `serviceaccount:<namespace>:<name>` (the last two need the instance's
/// `auth.kubernetes`).
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct RepositoryAccess {
    /// Who may pull (manifests, blobs, referrers, tags).
    #[serde(default)]
    pub pull: Vec<String>,
    /// Who may list revisions and uploads.
    #[serde(default)]
    pub browse: Vec<String>,
    /// Who may push.
    #[serde(default)]
    pub push: Vec<String>,
    /// Who may delete manifests and blobs.
    #[serde(default)]
    pub delete: Vec<String>,
    /// More CEL rules (`request`, `identity`): a matching one allows.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub rules: Vec<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Upstream {
    /// The registry, e.g. `https://registry-1.docker.io`; a path is the
    /// upstream namespace prefix.
    #[schemars(regex(pattern = r"^https?://"))]
    pub url: String,
    /// Basic credentials (`username`, `password` keys), in a Secret of
    /// this namespace labelled `angos.weebo.io/credential: "true"`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub credentials_secret_ref: Option<BasicCredentials>,
    /// Angos: 5.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_redirect: Option<u8>,
    /// Angos: 30.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub connect_timeout_secs: Option<u64>,
    /// Angos: 300.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub read_timeout_secs: Option<u64>,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct BasicCredentials {
    pub name: String,
    #[serde(default = "username_key")]
    pub username_key: String,
    #[serde(default = "password_key")]
    pub password_key: String,
}

fn username_key() -> String {
    "username".into()
}

fn password_key() -> String {
    "password".into()
}

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Downstream {
    /// Unique in the repository: the replication's job key and metrics
    /// label.
    #[schemars(length(min = 1))]
    pub name: String,
    #[schemars(regex(pattern = r"^https?://"))]
    pub url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub credentials_secret_ref: Option<BasicCredentials>,
    /// `EventAndReconcile` (default), `EventOnly` or `ReconcileOnly`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<ReplicationMode>,
    /// Regexes of the namespaces replicated; empty: all.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub namespace_filter: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(range(min = 1))]
    pub max_concurrent_pushes: Option<u64>,
    /// Propagate deletions to the downstream: it needs the annotation
    /// `angos.weebo.io/allow-data-deletion: "true"` on the repository.
    #[serde(default)]
    pub prune: bool,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, JsonSchema)]
pub enum ReplicationMode {
    EventAndReconcile,
    EventOnly,
    ReconcileOnly,
}

/// CEL rules on `image` (`namespace`, `tag`, `pushed_at`,
/// `last_pulled_at`, `scanned_at`) with `now()`, `days()`, `hours()`,
/// `minutes()`, `top_pushed()`, `top_pulled()`.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Rules {
    #[serde(default)]
    pub rules: Vec<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ImagePolicy {
    /// What applies when no rule matches; a matching rule does the
    /// opposite.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<IndexDefault>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub rules: Vec<String>,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, JsonSchema)]
pub enum IndexDefault {
    Index,
    Skip,
}

/// The status of a repository.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct RepositoryStatus {
    #[serde(flatten)]
    pub common: Status,
    /// The repository is in the instance's configuration.
    #[serde(default)]
    pub rendered: bool,
    /// The repository that holds its prefix instead (`namespace/name`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conflict_with: Option<String>,
}

/// The repositories' outcomes an instance records, by `namespace/name`:
/// what each repository's controller reflects.
pub type RenderedRepositories = BTreeMap<String, crate::status::RepositoryRender>;

#[cfg(test)]
mod tests {
    use kube::CustomResourceExt;

    use super::*;

    #[test]
    fn the_prefix_is_immutable_and_a_pull_through_refuses_pushes() {
        let crd = serde_json::to_value(Repository::crd()).unwrap();
        let schema = &crd["spec"]["versions"][0]["schema"]["openAPIV3Schema"]["properties"]["spec"];
        assert!(
            schema["properties"]["prefix"]["x-kubernetes-validations"]
                .to_string()
                .contains("self == oldSelf")
        );
        assert!(
            schema["x-kubernetes-validations"]
                .to_string()
                .contains("pull-through")
        );
        assert_eq!(crd["spec"]["names"]["shortNames"][0], "arepo");
    }
}
