use std::collections::BTreeMap;

use k8s_openapi::api::core::v1::{
    Affinity, EnvFromSource, EnvVar, LocalObjectReference, PodSecurityContext,
    ResourceRequirements, SecurityContext, Toleration, Volume, VolumeMount,
};
use k8s_openapi::api::networking::v1::NetworkPolicyPeer;
use k8s_openapi::apimachinery::pkg::apis::meta::v1::LabelSelector;
use kube::CustomResource;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

pub use weebo_kit_api::{AttachNamespaces, NamespacesFrom, SharedWith};

use crate::config::{
    Auth, Cache, ExtraConfig, Global, Reload, Server, Storage, Tracing, Ui, open_object,
};
use crate::status::InstanceStatus;

/// The short names the rest of the workspace uses.
pub use self::AngosLocalInstance as LocalInstance;
pub use self::AngosLocalInstanceSpec as LocalInstanceSpec;

/// An Angos registry the operator runs in this namespace: its
/// `Deployment`, its configuration (rendered from this spec, its
/// repositories and identities) and the objects around them. `Ready` tells
/// whether its pods run the desired configuration.
#[derive(CustomResource, Serialize, Deserialize, Clone, Debug, PartialEq, JsonSchema)]
#[kube(
    group = "angos.weebo.io",
    version = "v1alpha1",
    kind = "AngosLocalInstance",
    plural = "angoslocalinstances",
    shortname = "ali",
    namespaced,
    status = "InstanceStatus",
    printcolumn = r#"{"name":"Endpoint","type":"string","jsonPath":".status.endpoint"}"#,
    printcolumn = r#"{"name":"Version","type":"string","jsonPath":".status.version"}"#,
    printcolumn = r#"{"name":"Ready","type":"string","jsonPath":".status.conditions[?(@.type=='Ready')].status"}"#,
    printcolumn = r#"{"name":"Reason","type":"string","jsonPath":".status.conditions[?(@.type=='Ready')].reason"}"#,
    printcolumn = r#"{"name":"Age","type":"date","jsonPath":".metadata.creationTimestamp"}"#,
    // The instance's name is its Service's (a DNS-1035 label) and the
    // `app.kubernetes.io/instance` label of every object it owns.
    validation = Rule::new(
        "size(self.metadata.name) <= 63 && self.metadata.name.matches('^[a-z]([-a-z0-9]*[a-z0-9])?$')"
    ).message("the instance's name names its Service: a DNS-1035 label (lowercase letters, digits and '-', starting with a letter), at most 63 characters")
)]
#[serde(rename_all = "camelCase")]
pub struct AngosLocalInstanceSpec {
    /// The Angos image. Its repository must be one the operator allows
    /// (`managedInstances.allowedImages`).
    #[serde(default, skip_serializing_if = "Image::is_default")]
    pub image: Image,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub image_pull_secrets: Vec<LocalObjectReference>,
    /// One replica until the worker and the shared-store checks land.
    #[serde(default = "one")]
    #[schemars(range(min = 0, max = 1))]
    pub replicas: i32,
    /// Unset: requests 100m CPU / 256Mi, limit 1Gi.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(schema_with = "open_object")]
    pub resources: Option<ResourceRequirements>,
    #[serde(default)]
    pub service: Service,
    #[serde(default)]
    pub service_account: ServiceAccount,
    /// A `NetworkPolicy` around the pods: unset, none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub network_policy: Option<NetworkPolicy>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub pod_annotations: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub pod_labels: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub node_selector: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    #[schemars(schema_with = "open_list")]
    pub tolerations: Vec<Toleration>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(schema_with = "open_object")]
    pub affinity: Option<Affinity>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub priority_class_name: Option<String>,
    /// Rarely needed (Angos reads no secret from its environment). Secrets
    /// read here must be labelled `angos.weebo.io/credential: "true"`;
    /// `RUST_LOG` is the operator's (`logLevel`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    #[schemars(schema_with = "open_list")]
    pub env: Vec<EnvVar>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    #[schemars(schema_with = "open_list")]
    pub env_from: Vec<EnvFromSource>,
    /// CA bundles and other files: no `hostPath`; Secrets labelled
    /// `angos.weebo.io/credential: "true"`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    #[schemars(schema_with = "open_list")]
    pub extra_volumes: Vec<Volume>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    #[schemars(schema_with = "open_list")]
    pub extra_volume_mounts: Vec<VolumeMount>,
    /// Unset: non-root 65534 (the image's user), seccomp `RuntimeDefault`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(schema_with = "open_object")]
    pub pod_security_context: Option<PodSecurityContext>,
    /// Unset: read-only root filesystem, no capability, no escalation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(schema_with = "open_object")]
    pub security_context: Option<SecurityContext>,
    /// Unset: three times `global.shutdownDrainSecs` (30) plus 10.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub termination_grace_period_seconds: Option<i64>,
    /// `RUST_LOG` of the pods (e.g. `info`, `warn,angos=debug`); the
    /// operator always adds `angos::configuration::watcher=info`, the
    /// reload messages it reads.
    #[serde(default = "info")]
    pub log_level: String,
    #[serde(default)]
    pub server: Server,
    pub storage: Storage,
    #[serde(default)]
    pub global: Global,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache: Option<Cache>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ui: Option<Ui>,
    #[serde(default)]
    pub auth: Auth,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tracing: Option<Tracing>,
    #[serde(default)]
    pub reload: Reload,
    /// Which `AngosRepository`s (and `AngosIdentity`s) the instance
    /// serves, and from which namespaces.
    #[serde(default, skip_serializing_if = "RepositorySelector::is_default")]
    pub repository_selector: RepositorySelector,
    /// The prune lock (spec 6.6): no `AngosMaintenance` prune runs while a
    /// repository was released less than `windowHours` ago (its data goes
    /// at the next prune: time to notice and recreate it).
    #[serde(default)]
    pub prune_lock: PruneLock,
    /// Keys of Angos's configuration no typed field renders, merged last
    /// into the hot file. Checked by Angos only (advisory
    /// `ExtraConfigUsed`).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    #[schemars(schema_with = "open_object")]
    pub extra_config: ExtraConfig,
}

impl Default for AngosLocalInstanceSpec {
    /// What a spec of `{storage: {}}` reads as, its defaults filled.
    fn default() -> Self {
        Self {
            image: Image::default(),
            image_pull_secrets: Vec::new(),
            replicas: one(),
            resources: None,
            service: Service::default(),
            service_account: ServiceAccount::default(),
            network_policy: None,
            pod_annotations: BTreeMap::new(),
            pod_labels: BTreeMap::new(),
            node_selector: BTreeMap::new(),
            tolerations: Vec::new(),
            affinity: None,
            priority_class_name: None,
            env: Vec::new(),
            env_from: Vec::new(),
            extra_volumes: Vec::new(),
            extra_volume_mounts: Vec::new(),
            pod_security_context: None,
            security_context: None,
            termination_grace_period_seconds: None,
            log_level: info(),
            server: Server::default(),
            storage: Storage::default(),
            global: Global::default(),
            cache: None,
            ui: None,
            auth: Auth::default(),
            tracing: None,
            reload: Reload::default(),
            repository_selector: RepositorySelector::default(),
            prune_lock: PruneLock::default(),
            extra_config: ExtraConfig::new(),
        }
    }
}

fn open_list(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
    schemars::json_schema!({
        "type": "array",
        "items": {"type": "object", "x-kubernetes-preserve-unknown-fields": true},
    })
}

fn one() -> i32 {
    1
}

fn info() -> String {
    "info".into()
}

fn yes() -> bool {
    true
}

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Image {
    /// Default: `ghcr.io/project-angos/angos`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repository: Option<String>,
    /// Default: the Angos version the operator targets.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tag: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pull_policy: Option<PullPolicy>,
}

impl Image {
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, Default, PartialEq, Eq, JsonSchema)]
pub enum PullPolicy {
    Always,
    #[default]
    IfNotPresent,
    Never,
}

impl PullPolicy {
    pub fn as_str(self) -> &'static str {
        match self {
            PullPolicy::Always => "Always",
            PullPolicy::IfNotPresent => "IfNotPresent",
            PullPolicy::Never => "Never",
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Service {
    #[serde(default, rename = "type")]
    pub r#type: ServiceType,
    /// The Service's port (the container's is `server.port`).
    #[serde(default = "service_port")]
    pub port: i32,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub annotations: BTreeMap<String, String>,
}

fn service_port() -> i32 {
    8000
}

impl Default for Service {
    fn default() -> Self {
        Self {
            r#type: ServiceType::default(),
            port: service_port(),
            annotations: BTreeMap::new(),
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, Default, PartialEq, Eq, JsonSchema)]
pub enum ServiceType {
    #[default]
    ClusterIP,
    NodePort,
    LoadBalancer,
}

impl ServiceType {
    pub fn as_str(self) -> &'static str {
        match self {
            ServiceType::ClusterIP => "ClusterIP",
            ServiceType::NodePort => "NodePort",
            ServiceType::LoadBalancer => "LoadBalancer",
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ServiceAccount {
    /// `false`: use `name`, which must exist.
    #[serde(default = "yes")]
    pub create: bool,
    /// Default: the instance's name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub annotations: BTreeMap<String, String>,
}

impl Default for ServiceAccount {
    fn default() -> Self {
        Self {
            create: true,
            name: None,
            annotations: BTreeMap::new(),
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct NetworkPolicy {
    /// Who may reach the registry's port; empty: anyone.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    #[schemars(schema_with = "open_list")]
    pub ingress_from: Vec<NetworkPolicyPeer>,
    /// Where the pods may connect besides DNS (the object store, the
    /// OIDC providers, the upstream registries, the API server).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    #[schemars(schema_with = "open_list")]
    pub egress_to: Vec<NetworkPolicyPeer>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct PruneLock {
    #[serde(default = "day_hours")]
    #[schemars(range(min = 0))]
    pub window_hours: u32,
}

fn day_hours() -> u32 {
    24
}

impl Default for PruneLock {
    fn default() -> Self {
        Self {
            window_hours: day_hours(),
        }
    }
}

/// Which `AngosRepository`s (and `AngosIdentity`s) the instance serves.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct RepositorySelector {
    /// Only the objects whose labels match; all of them when unset.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selector: Option<LabelSelector>,
    /// The namespaces whose objects may attach to this instance (with an
    /// `instanceRef` naming it): its own always.
    #[serde(default, skip_serializing_if = "AttachNamespaces::is_default")]
    pub namespaces: AttachNamespaces,
}

impl RepositorySelector {
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }
}

/// What the instance shares with the repositories and identities of other
/// namespaces (its `repositorySelector.namespaces`).
pub fn object_sharing(instance: &LocalInstance) -> Option<SharedWith> {
    instance.spec.repository_selector.namespaces.shared_with()
}

/// The names of what an instance owns.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Names<'a>(pub &'a str);

impl Names<'_> {
    /// The `Deployment`, `Service`, `ServiceAccount` and `NetworkPolicy`.
    pub fn workload(&self) -> String {
        self.0.to_string()
    }
    /// The hot layer's `Secret`.
    pub fn config(&self) -> String {
        format!("{}-config", self.0)
    }
    /// The last hot layer every pod accepted.
    pub fn config_applied(&self) -> String {
        format!("{}-config-applied", self.0)
    }
    /// The startup layer's `Secret` prefix (`<prefix>-<hash8>`).
    pub fn boot_prefix(&self) -> String {
        format!("{}-boot", self.0)
    }
    /// The generated token-service key.
    pub fn token_key(&self) -> String {
        format!("{}-token-key", self.0)
    }
    /// The claim of an FS store the operator creates.
    pub fn data(&self) -> String {
        format!("{}-data", self.0)
    }
    /// The claim of an FS metadata store the operator creates.
    pub fn metadata(&self) -> String {
        format!("{}-metadata", self.0)
    }
}

#[cfg(test)]
mod tests {
    use kube::CustomResourceExt;

    use super::*;

    #[test]
    fn the_name_and_the_short_name_are_checked() {
        let crd = serde_json::to_value(LocalInstance::crd()).unwrap();
        assert_eq!(crd["spec"]["names"]["shortNames"][0], "ali");
        let rules =
            &crd["spec"]["versions"][0]["schema"]["openAPIV3Schema"]["x-kubernetes-validations"];
        assert!(rules.to_string().contains("DNS-1035"), "{rules}");
    }

    #[test]
    fn a_minimal_instance_reads_with_its_defaults() {
        let spec: LocalInstanceSpec =
            serde_json::from_value(serde_json::json!({"storage": {"fs": {}}})).unwrap();
        assert_eq!(spec.replicas, 1);
        assert_eq!(spec.server.port, 8000);
        assert_eq!(spec.storage.fs.unwrap().root_dir, "/data");
        assert_eq!(spec.log_level, "info");
        assert_eq!(spec.reload.timeout_seconds, 180);
        assert!(spec.service_account.create);
    }

    #[test]
    fn an_instance_shares_only_through_its_selector() {
        let mut instance = LocalInstance::new("reg", LocalInstanceSpec::default());
        assert_eq!(object_sharing(&instance), None);
        instance.spec.repository_selector.namespaces.from = NamespacesFrom::All;
        assert!(object_sharing(&instance).is_some());
    }
}
