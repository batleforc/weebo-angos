//! What an `AngosLocalInstance` says of Angos's own configuration: typed
//! fields (spec 2.1) rendered by the operator into the two files of the
//! instance. Every field names the Angos key it renders; Angos's own
//! default applies when a field is unset, unless the field says otherwise.

use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

pub use weebo_kit_api::LocalSecretKeyRef;

/// A size as Angos reads it: an integer and a unit (`B`, `KB`, `KiB`, `MB`,
/// `MiB`, `GB`, `GiB`, `TB`, `TiB`; decimal and binary units differ).
pub type Size = String;

fn optional_size_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
    schemars::json_schema!({
        "type": "string",
        "pattern": "^[0-9]+(B|KB|KiB|MB|MiB|GB|GiB|TB|TiB)?$",
        "nullable": true,
    })
}

pub(crate) fn open_object(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
    schemars::json_schema!({"type": "object", "x-kubernetes-preserve-unknown-fields": true})
}

/// `[server]`: the listener.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Server {
    /// `server.port`, and the container's port. Read at startup only: a
    /// change rolls the pods.
    #[serde(default = "default_port")]
    #[schemars(range(min = 1, max = 65535))]
    pub port: u16,
    /// `server.query_timeout` (seconds, > 0). Angos: 3600.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(range(min = 1))]
    pub query_timeout_secs: Option<u64>,
    /// `server.query_timeout_grace_period` (seconds, > 0). Angos: 60.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(range(min = 1))]
    pub query_timeout_grace_period_secs: Option<u64>,
    /// `server.handshake_timeout` (seconds, > 0). Angos: 10.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(range(min = 1))]
    pub handshake_timeout_secs: Option<u64>,
    /// `global.trusted_proxies`: the IPs or CIDRs whose forwarded headers
    /// Angos trusts. Behind an ingress, without it, the client IP Angos
    /// sees is the ingress controller's.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub trusted_proxies: Vec<String>,
    /// `[server.tls]`: Angos serves TLS itself, from a `kubernetes.io/tls`
    /// Secret mounted in the pods. Certificates are reloaded by Angos when
    /// the Secret changes; turning TLS on or off rolls the pods.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tls: Option<ServerTls>,
}

fn default_port() -> u16 {
    8000
}

impl Default for Server {
    fn default() -> Self {
        Self {
            port: default_port(),
            query_timeout_secs: None,
            query_timeout_grace_period_secs: None,
            handshake_timeout_secs: None,
            trusted_proxies: Vec::new(),
            tls: None,
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[schemars(extend("x-kubernetes-validations" = [
    {"rule": "self.clientAuth != 'Required' || has(self.clientCA)", "message": "clientAuth: Required needs clientCA"}
]))]
pub struct ServerTls {
    /// A `kubernetes.io/tls` Secret of the namespace (`tls.crt`,
    /// `tls.key`), labelled `angos.weebo.io/credential: "true"`.
    pub secret_name: String,
    /// The CA that client certificates must chain to
    /// (`client_ca_bundle`): a key of a Secret of the namespace, labelled
    /// `angos.weebo.io/credential: "true"`.
    #[serde(default, rename = "clientCA", skip_serializing_if = "Option::is_none")]
    pub client_ca: Option<LocalSecretKeyRef>,
    /// `client_auth`: `Optional` (a certificate is checked when given) or
    /// `Required`.
    #[serde(default)]
    pub client_auth: ClientAuth,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, Default, PartialEq, Eq, JsonSchema)]
pub enum ClientAuth {
    #[default]
    Optional,
    Required,
}

/// Where Angos keeps blobs and metadata. Read at startup only: a change of
/// the store waits for the annotation `angos.weebo.io/confirm-storage`
/// (set to `status.pendingBootHash`), then rolls the pods. Pointing an
/// instance at another, empty store makes it serve an empty registry: no
/// data moves.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[schemars(extend("x-kubernetes-validations" = [
    {"rule": "has(self.s3) != has(self.fs)", "message": "storage sets exactly one of s3 and fs"}
]))]
pub struct Storage {
    /// `[blob_store.s3]`: an S3-compatible bucket.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub s3: Option<S3Storage>,
    /// `[blob_store.fs]`: a directory of the pods, on a volume.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fs: Option<FsStorage>,
    /// `[metadata_store]`: unset, the blob store holds the metadata too.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<MetadataStorage>,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[schemars(extend("x-kubernetes-validations" = [
    {"rule": "has(self.s3) != has(self.fs)", "message": "storage.metadata sets exactly one of s3 and fs"}
]))]
pub struct MetadataStorage {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub s3: Option<S3Storage>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fs: Option<FsStorage>,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct S3Storage {
    /// The S3 endpoint URL, e.g. `https://s3.example.com`.
    pub endpoint: String,
    // Drawn by the docs' CRD IDE (the kit's shared schematic kinds).
    #[schemars(extend("x-weebo-produces" = { "kind": { "group": "schematic.weebo.io", "kind": "S3Bucket" }, "name": ["."] }))]
    pub bucket: String,
    /// The bucket's region (`us-east-1` for most S3-compatible stores).
    pub region: String,
    /// A prefix of every key Angos writes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key_prefix: Option<String>,
    /// The access key, in a Secret of the namespace labelled
    /// `angos.weebo.io/credential: "true"`. A change of the Secret's content
    /// alone rolls the pods without confirmation.
    pub credentials_secret_ref: S3Credentials,
    /// Tuning of the S3 client; Angos's defaults when unset.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transport: Option<S3Transport>,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct S3Credentials {
    pub name: String,
    #[serde(default = "access_key_id_key")]
    pub access_key_id_key: String,
    #[serde(default = "secret_key_key")]
    pub secret_key_key: String,
}

fn access_key_id_key() -> String {
    "access-key-id".into()
}

fn secret_key_key() -> String {
    "secret-key".into()
}

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct S3Transport {
    /// `multipart_part_size`, at least `5MiB` (S3's minimum part).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(schema_with = "optional_size_schema")]
    pub multipart_part_size: Option<Size>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(schema_with = "optional_size_schema")]
    pub multipart_copy_threshold: Option<Size>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(schema_with = "optional_size_schema")]
    pub multipart_copy_chunk_size: Option<Size>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(range(min = 1))]
    pub multipart_copy_jobs: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub multipart_uniform_parts: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub operation_timeout_secs: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub operation_attempt_timeout_secs: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_attempts: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub circuit_breaker_threshold: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub circuit_breaker_cooldown_secs: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(range(min = 1))]
    pub children_scan_concurrency: Option<u64>,
    /// How long a presigned URL lives (redirects). Angos: 1800.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub presign_ttl_secs: Option<u64>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct FsStorage {
    /// `root_dir`: where the volume is mounted.
    #[serde(default = "default_root_dir")]
    pub root_dir: String,
    /// `sync_to_disk`: fsync every write. Angos: false.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sync_to_disk: Option<bool>,
    /// The volume under `rootDir`: an existing claim, or one the operator
    /// creates once (never resized). Unset: an `emptyDir` — the data goes
    /// with the pod.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub volume: Option<FsVolume>,
}

fn default_root_dir() -> String {
    "/data".into()
}

impl Default for FsStorage {
    fn default() -> Self {
        Self {
            root_dir: default_root_dir(),
            sync_to_disk: None,
            volume: None,
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[schemars(extend("x-kubernetes-validations" = [
    {"rule": "has(self.claimName) != has(self.size)", "message": "volume sets claimName or size, not both"}
]))]
pub struct FsVolume {
    /// An existing `PersistentVolumeClaim` of the namespace.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub claim_name: Option<String>,
    /// The size of the claim the operator creates (`<instance>-data`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub storage_class: Option<String>,
    #[serde(default)]
    pub access_mode: AccessMode,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, Default, PartialEq, Eq, JsonSchema)]
pub enum AccessMode {
    #[default]
    ReadWriteOnce,
    ReadWriteMany,
    ReadWriteOncePod,
}

impl AccessMode {
    pub fn as_str(self) -> &'static str {
        match self {
            AccessMode::ReadWriteOnce => "ReadWriteOnce",
            AccessMode::ReadWriteMany => "ReadWriteMany",
            AccessMode::ReadWriteOncePod => "ReadWriteOncePod",
        }
    }
}

/// `[global]`: what applies to every repository.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Global {
    /// `max_concurrent_requests` (Angos's worker threads). Startup only.
    /// Angos: 64.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(range(min = 1))]
    pub max_concurrent_requests: Option<u64>,
    /// `shutdown_drain_secs`: how long a stopping pod drains. Startup
    /// only. Angos: 30; the pods' grace period follows it. At most an hour.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(range(max = 3600))]
    pub shutdown_drain_secs: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub jobs: Option<Jobs>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limits: Option<Limits>,
    /// Blob and manifest downloads redirected to presigned S3 URLs.
    /// **Off by default** (Angos: on): the clients must then reach the
    /// bucket themselves.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub redirects: Option<Redirects>,
    /// `immutable_tags`: a tag can't be moved once pushed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub immutable_tags: Option<bool>,
    /// `immutable_tags_exclusions`: regexes of the tags left mutable.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub immutable_tags_exclusions: Vec<String>,
    /// `update_pull_time`: record when an image is pulled (needed by any
    /// rule on `last_pulled_at` or `top_pulled`). Angos: false.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub update_pull_time: Option<bool>,
    /// `allow_missing_manifest_references`. Angos: true.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allow_missing_manifest_references: Option<bool>,
    /// `gc_grace_secs`. Angos: 300.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gc_grace_secs: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub listing: Option<Listing>,
    /// `[global.pull_history]`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pull_history: Option<PullHistory>,
    /// The global access policy, checked before any repository's.
    #[serde(default)]
    pub access: GlobalAccess,
    /// `retention_policy`: CEL rules on `image` (OR with the
    /// repositories').
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retention: Option<crate::repository::Rules>,
    /// `index` policy of every repository without its own.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub index: Option<crate::repository::ImagePolicy>,
}

/// The instance's own verbs, with the repositories' vocabulary of subjects.
/// The operator always lets through the probes, the UI's assets, `/token`
/// for any identity, and leaves a repository's namespaces to its own
/// policy; a namespace no repository declares is refused.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct GlobalAccess {
    /// Who may list the catalog, the repositories and their namespaces.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub catalog: Vec<String>,
    /// Who may list, retry and delete background jobs: named identities,
    /// groups or ServiceAccounts only.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub jobs: Vec<String>,
    /// Who may do anything, in every repository too (the rule is added to
    /// each repository's policy): named identities, groups or
    /// ServiceAccounts only.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub admin: Vec<String>,
    /// Who may scrape `/metrics`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub metrics: Vec<String>,
    /// More CEL rules (`request`, `identity`): a matching one allows.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub rules: Vec<String>,
}

/// `max_concurrent_*_jobs`.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Jobs {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(range(min = 1))]
    pub cache: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(range(min = 1))]
    pub replication: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(range(min = 1))]
    pub scan: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(range(min = 1))]
    pub index: Option<u64>,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Limits {
    /// `max_index_size`. Angos: `50GiB`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(schema_with = "optional_size_schema")]
    pub max_index_size: Option<Size>,
    /// `max_index_entries`. Angos: 500000.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(range(min = 1))]
    pub max_index_entries: Option<u64>,
    /// `max_manifest_size`. Angos: `5MiB`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(schema_with = "optional_size_schema")]
    pub max_manifest_size: Option<Size>,
    /// `max_blob_size`. Angos: `100GiB`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(schema_with = "optional_size_schema")]
    pub max_blob_size: Option<Size>,
    /// `blob_stream_frame_size`. Angos: `128KiB`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(schema_with = "optional_size_schema")]
    pub blob_stream_frame_size: Option<Size>,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Redirects {
    /// `enable_blob_redirect`.
    #[serde(default)]
    pub blob: bool,
    /// `enable_manifest_redirect`.
    #[serde(default)]
    pub manifest: bool,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Listing {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(range(min = 1))]
    pub namespace_walk_concurrency: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(range(min = 1))]
    pub listing_read_concurrency: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(range(min = 1))]
    pub revision_cache_entries: Option<u64>,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct PullHistory {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(range(min = 1))]
    pub max_pulls: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_age_secs: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compact_after_secs: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(range(min = 1))]
    pub compact_after_pulls: Option<u64>,
}

/// `[cache]`: unset, Angos caches in memory.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Cache {
    /// `[cache.redis]`: a Redis or Valkey the operator doesn't create.
    pub redis: RedisCache,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct RedisCache {
    /// The `redis://` URL (credentials included), in a Secret of the
    /// namespace labelled `angos.weebo.io/credential: "true"`.
    pub url_secret_ref: LocalSecretKeyRef,
    /// `key_prefix` of every key Angos writes.
    #[schemars(length(min = 1))]
    pub key_prefix: String,
}

/// `[ui]`: Angos's web UI.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Ui {
    #[serde(default)]
    pub enabled: bool,
    /// The name the UI shows. Angos: `Angos`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Sign-in to the UI through an OIDC provider of `auth.oidc` (a public
    /// PKCE client, no secret).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub oidc: Option<UiOidc>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[schemars(extend("x-kubernetes-validations" = [
    {"rule": "has(self.clientId) != has(self.clientIdFrom)", "message": "ui.oidc sets clientId or clientIdFrom"}
]))]
pub struct UiOidc {
    /// The `name` of an `auth.oidc` provider.
    pub provider: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_id: Option<String>,
    /// The client id from a Secret of the namespace (e.g. the
    /// `AUTHENTIK_CLIENT_ID` a weebo-authentik application writes),
    /// labelled `angos.weebo.io/credential: "true"`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_id_from: Option<LocalSecretKeyRef>,
    /// The scopes asked. Angos: `openid profile email`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub scopes: Vec<String>,
}

/// `[auth]`: who may authenticate.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Auth {
    /// `[auth.oidc.<name>]`: OIDC providers whose tokens Angos accepts.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub oidc: Vec<OidcProvider>,
    /// The cluster's ServiceAccount tokens as an OIDC provider (workloads
    /// pull with a projected token).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kubernetes: Option<KubernetesAuth>,
    /// `[auth.token_service]`: the tokens `docker login` gets from
    /// `/token`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token_service: Option<TokenService>,
    /// `[auth.webhook.<name>]`: HTTP services asked whether a request is
    /// allowed (after the access policies). One may be the instance's
    /// `default` (`global.authorization_webhook`): it then decides every
    /// request — the probes included — and **fails closed**.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    #[schemars(extend("x-kubernetes-validations" = [
        {"rule": "self.filter(w, w.default).size() <= 1", "message": "at most one authorization webhook is the default"},
        {"rule": "self.all(w, self.exists_one(o, o.name == w.name))", "message": "authorization webhook names are unique"}
    ]))]
    #[schemars(length(max = 32))]
    pub webhooks: Vec<AuthWebhook>,
}

/// A client's TLS towards a service: the CA its certificate is checked
/// with (beside the public roots) and the certificate presented. Mounted in
/// the pods: a change rolls them.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ClientTls {
    /// A PEM bundle (`server_ca_bundle`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ca: Option<CaBundle>,
    /// A `kubernetes.io/tls` Secret (`tls.crt`, `tls.key`) labelled
    /// `angos.weebo.io/credential: "true"`, presented for mTLS.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_certificate_secret_name: Option<String>,
}

/// A CA bundle in a ConfigMap or a Secret of the namespace (a Secret
/// labelled `angos.weebo.io/credential: "true"`).
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[schemars(extend("x-kubernetes-validations" = [
    {"rule": "has(self.configMapKeyRef) != has(self.secretKeyRef)", "message": "a CA bundle is in a ConfigMap or a Secret"}
]))]
pub struct CaBundle {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(extend("x-weebo-link" = { "to": [{ "kind": "ConfigMap" }] }))]
    pub config_map_key_ref: Option<KeyRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub secret_key_ref: Option<LocalSecretKeyRef>,
}

/// A key of a ConfigMap of the namespace.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct KeyRef {
    pub name: String,
    pub key: String,
}

/// `[auth.webhook.<name>]`.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[schemars(extend("x-kubernetes-validations" = [
    {"rule": "!(has(self.bearerTokenSecretRef) && has(self.basicAuthSecretRef))", "message": "an authorization webhook authenticates with a bearer token or Basic credentials, not both"}
]))]
pub struct AuthWebhook {
    /// The key repositories name it by (`authorizationWebhook`).
    #[schemars(regex(pattern = r"^[A-Za-z0-9_-]+$"), length(min = 1, max = 63))]
    pub name: String,
    #[schemars(regex(pattern = r"^https?://"))]
    // Drawn by the docs' CRD IDE (the kit's shared schematic kinds).
    #[schemars(extend("x-weebo-produces" = { "kind": { "group": "schematic.weebo.io", "kind": "HttpEndpoint" }, "name": ["."] }))]
    pub url: String,
    /// How long Angos waits for an answer (required by Angos).
    #[schemars(range(min = 1))]
    pub timeout_ms: u64,
    /// A bearer token, from a Secret labelled
    /// `angos.weebo.io/credential: "true"`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bearer_token_secret_ref: Option<LocalSecretKeyRef>,
    /// Basic credentials (`username`, `password` keys), from a Secret
    /// labelled `angos.weebo.io/credential: "true"`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub basic_auth_secret_ref: Option<crate::repository::BasicCredentials>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tls: Option<ClientTls>,
    /// Request headers forwarded to the webhook.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub forward_headers: Vec<String>,
    /// `global.authorization_webhook`: asked for every request without a
    /// repository's own.
    #[serde(default)]
    pub default: bool,
}

/// `[global.scan]`: the vulnerability scanner Angos sends pushed images
/// to (deployed apart, e.g. `angos scanner trivy`), and which images it
/// scans.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Scan {
    /// The scanner's base URL (Angos posts to `<url>/scan`).
    #[schemars(regex(pattern = r"^https?://"))]
    // Drawn by the docs' CRD IDE (the kit's shared schematic kinds).
    #[schemars(extend("x-weebo-produces" = { "kind": { "group": "schematic.weebo.io", "kind": "HttpEndpoint" }, "name": ["."] }))]
    pub url: String,
    /// The scanner's token, from a Secret labelled
    /// `angos.weebo.io/credential: "true"`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token_secret_ref: Option<LocalSecretKeyRef>,
    /// Angos: 600.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(range(min = 1))]
    pub timeout_secs: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tls: Option<ClientTls>,
    /// What applies when no rule matches (unset: `Skip`); a matching rule
    /// does the opposite. A repository's own `scan` replaces it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<ScanDefault>,
    /// CEL rules on `image`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub rules: Vec<String>,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, JsonSchema)]
pub enum ScanDefault {
    Scan,
    Skip,
}

/// `[event_webhook.<name>]`: an HTTP endpoint told of pushes, pulls and
/// deletions.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct EventWebhook {
    /// The key repositories name it by (`eventWebhooks`).
    #[schemars(regex(pattern = r"^[A-Za-z0-9_-]+$"), length(min = 1, max = 63))]
    pub name: String,
    #[schemars(regex(pattern = r"^https?://"))]
    // Drawn by the docs' CRD IDE (the kit's shared schematic kinds).
    #[schemars(extend("x-weebo-produces" = { "kind": { "group": "schematic.weebo.io", "kind": "HttpEndpoint" }, "name": ["."] }))]
    pub url: String,
    /// `Required`: the request fails when the webhook does; `Optional`:
    /// tried, never blocking; `Async`: sent in the background.
    pub policy: DeliveryPolicy,
    /// The bearer token Angos sends, from a Secret labelled
    /// `angos.weebo.io/credential: "true"`.
    pub token_secret_ref: LocalSecretKeyRef,
    /// Angos: 5000.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(range(min = 1))]
    pub timeout_ms: Option<u64>,
    /// At most 16. Angos: 3 for `Required`, 0 otherwise.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(range(max = 16))]
    pub max_retries: Option<u32>,
    #[schemars(length(min = 1))]
    pub events: Vec<EventKind>,
    /// Regexes of the namespaces reported; empty: all.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub repository_filter: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tls: Option<ClientTls>,
    /// In `global.event_webhooks`: told of every repository without its
    /// own list.
    #[serde(default)]
    pub default: bool,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, JsonSchema)]
pub enum DeliveryPolicy {
    Required,
    Optional,
    Async,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, JsonSchema)]
pub enum EventKind {
    #[serde(rename = "manifest.push")]
    ManifestPush,
    #[serde(rename = "manifest.pull")]
    ManifestPull,
    #[serde(rename = "manifest.delete")]
    ManifestDelete,
    #[serde(rename = "blob.push")]
    BlobPush,
    #[serde(rename = "blob.pull")]
    BlobPull,
    #[serde(rename = "tag.create")]
    TagCreate,
    #[serde(rename = "tag.delete")]
    TagDelete,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[schemars(extend("x-kubernetes-validations" = [
    {"rule": "has(self.issuer) != has(self.issuerFrom)", "message": "an OIDC provider sets issuer or issuerFrom"},
    {"rule": "has(self.audience) != has(self.audienceFrom)", "message": "an OIDC provider sets audience or audienceFrom: without one, Angos accepts any audience"}
]))]
pub struct OidcProvider {
    /// The provider's key: `identity.oidc.provider_name` in access rules,
    /// and never an identity's username.
    #[schemars(regex(pattern = r"^[A-Za-z0-9_-]+$"))]
    pub name: String,
    /// The issuer, compared **exactly** with the one the provider
    /// advertises (trailing slash included).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    // Drawn by the docs' CRD IDE (the kit's shared schematic kinds).
    #[schemars(extend("x-weebo-produces" = { "kind": { "group": "schematic.weebo.io", "kind": "OidcIssuer" }, "name": ["."] }))]
    pub issuer: Option<String>,
    /// The issuer from a Secret of the namespace (e.g. the `AUTHENTIK_URL`
    /// a weebo-authentik application writes), labelled
    /// `angos.weebo.io/credential: "true"`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub issuer_from: Option<LocalSecretKeyRef>,
    /// `required_audience`: the audience a token must carry.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub audience: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub audience_from: Option<LocalSecretKeyRef>,
    /// `jwks_uri`: unset, discovered from the issuer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub jwks_uri: Option<String>,
    /// `required_claims`: claims a token must carry (non-null).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub required_claims: Vec<String>,
    /// `allowed_algorithms`. Angos: `RS256`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub allowed_algorithms: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub jwks_refresh_interval: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clock_skew_tolerance: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub http_request_timeout_secs: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub jwks_refresh_timeout_secs: Option<u64>,
    /// The CA the provider's certificate is checked with (beside the
    /// public roots), and a client certificate.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tls: Option<ClientTls>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct KubernetesAuth {
    /// The provider's name in `auth.oidc` and in access rules
    /// (`namespace:` and `serviceaccount:` subjects).
    #[serde(default = "kube")]
    #[schemars(regex(pattern = r"^[A-Za-z0-9_-]+$"))]
    pub name: String,
    /// The audience workload tokens must be issued for.
    pub audience: String,
    /// The cluster's ServiceAccount issuer. Unset: read once by the
    /// operator from the API server's discovery (required when the
    /// operator is limited to one namespace).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    // Drawn by the docs' CRD IDE (the kit's shared schematic kinds).
    #[schemars(extend("x-weebo-produces" = { "kind": { "group": "schematic.weebo.io", "kind": "OidcIssuer" }, "name": ["."] }))]
    pub issuer: Option<String>,
    /// Default: `https://kubernetes.default.svc/openid/v1/jwks`, read with
    /// the instance's own account token. That account must be bound to
    /// `system:service-account-issuer-discovery` (not done by the
    /// operator).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub jwks_uri: Option<String>,
}

fn kube() -> String {
    "kube".into()
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct TokenService {
    /// `realm`: the public URL of `/token`, `https://<host>/token`
    /// (required: Angos would derive it from the request otherwise, wrong
    /// behind an ingress).
    #[schemars(regex(pattern = r"^https?://[^/]+(/.*)?/token$"))]
    pub realm: String,
    /// `ttl_secs` of a token, 1 to 86400. Angos: 3600.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(range(min = 1, max = 86400))]
    pub ttl_secs: Option<u64>,
    /// The signing key (base64, at least 32 bytes decoded). Unset: the
    /// operator generates one in `<instance>-token-key`; the annotation
    /// `angos.weebo.io/rotate` changes it — every token is then invalid.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key_secret_ref: Option<LocalSecretKeyRef>,
}

/// `[observability.tracing]`: OTLP traces. Startup only.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Tracing {
    /// The OTLP gRPC endpoint.
    // Drawn by the docs' CRD IDE (the kit's shared schematic kinds).
    #[schemars(extend("x-weebo-produces" = { "kind": { "group": "schematic.weebo.io", "kind": "HttpEndpoint" }, "name": ["."] }))]
    pub endpoint: String,
    /// The share of traces kept, from 0 to 1.
    #[schemars(range(min = 0.0, max = 1.0))]
    pub sampling_rate: f64,
}

/// How a new configuration reaches the pods.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Reload {
    /// `Hot`: Angos reloads the file in place, the operator reads in each
    /// pod's log that it did. `Rollout`: every change also rolls the pods.
    #[serde(default)]
    pub mode: ReloadMode,
    /// How long to wait for every pod's answer (the kubelet takes up to
    /// 90 s to update a mounted Secret). At most a day.
    #[serde(default = "reload_timeout")]
    #[schemars(range(min = 30, max = 86400))]
    pub timeout_seconds: u64,
    /// When no answer came in time: `Wait` (keep reading the logs) or
    /// `Rollout` (roll the pods, which read the file at start).
    #[serde(default)]
    pub on_unconfirmed: OnUnconfirmed,
}

fn reload_timeout() -> u64 {
    180
}

impl Default for Reload {
    fn default() -> Self {
        Self {
            mode: ReloadMode::default(),
            timeout_seconds: reload_timeout(),
            on_unconfirmed: OnUnconfirmed::default(),
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, Default, PartialEq, Eq, JsonSchema)]
pub enum ReloadMode {
    #[default]
    Hot,
    Rollout,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, Default, PartialEq, Eq, JsonSchema)]
pub enum OnUnconfirmed {
    #[default]
    Wait,
    Rollout,
}

/// A free-form fragment of Angos's configuration, merged last into the hot
/// file: for keys no typed field renders (never one that does, never the
/// startup keys, never part of an array).
pub type ExtraConfig = BTreeMap<String, serde_json::Value>;
