//! The configuration file of Angos **1.12.3**, as the operator renders it:
//! a serde mirror of Angos's `Configuration` (`src/configuration/mod.rs`),
//! every key Angos reads, under its TOML name. Angos has no configuration
//! crate and ignores unknown keys silently: this model is the only check
//! before a pod loads the file, and it is versioned with the Angos version
//! the operator targets (spec 2.1, 6.8) — re-read it at every upgrade.
//!
//! Every optional key is `Option` and left out when unset: Angos applies
//! its own default (noted on each field). The operator's own defaults
//! (where they differ from Angos's) are set by whoever builds the model,
//! not here.
//!
//! The file is rendered as two layers (spec 2.3, D11), merged by Angos in
//! order (`-c boot -c hot`): [`AngosConfig::split`] cuts the rendered table
//! at [`BOOT_KEYS`]. Angos merges tables recursively and replaces any other
//! value whole (`src/configuration/merge.rs`): a table may be spread over
//! both layers, an array never is ([`merge`] reproduces the rule, the
//! tests check the cut).

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// The Angos version this model is the configuration of.
pub const ANGOS_VERSION: &str = "1.12.3";

/// The keys read at startup only, or that must never change under a
/// running pod (the storage): the startup layer, a new `Secret` named by
/// its hash, a rollout (spec 2.3). Every other key is in the hot layer.
pub const BOOT_KEYS: &[&[&str]] = &[
    &["server", "bind_address"],
    &["server", "port"],
    &["blob_store"],
    &["metadata_store"],
    &["global", "max_concurrent_requests"],
    &["global", "shutdown_drain_secs"],
    &["global", "job_queue"],
    &["observability"],
];

/// `Configuration` (`src/configuration/mod.rs:45-67`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct AngosConfig {
    pub server: Server,
    #[serde(default, skip_serializing_if = "Global::is_empty")]
    pub global: Global,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ui: Option<Ui>,
    /// Absent: the in-memory cache.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache: Option<Cache>,
    /// Required by Angos: the operator always renders one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blob_store: Option<Store>,
    /// Absent: the blob store's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata_store: Option<Store>,
    #[serde(default, skip_serializing_if = "Auth::is_empty")]
    pub auth: Auth,
    /// `[repository."<prefix>"]`.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub repository: BTreeMap<String, Repository>,
    /// `[event_webhook.<name>]`.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub event_webhook: BTreeMap<String, EventWebhook>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub observability: Option<Observability>,
}

/// `[server]`: `ListenerBaseConfig` + `[server.tls]`
/// (`src/configuration/server.rs`, `listeners.rs`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Server {
    /// Required by Angos (no serde default); always `0.0.0.0`.
    pub bind_address: String,
    /// Angos: 8000.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub port: Option<u16>,
    /// Seconds, > 0. Angos: 3600.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub query_timeout: Option<u64>,
    /// Seconds, > 0. Angos: 60.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub query_timeout_grace_period: Option<u64>,
    /// Seconds, > 0. Angos: 10.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub handshake_timeout: Option<u64>,
    /// Present: a TLS listener. Switching TLS on or off is not applied
    /// hot: the operator rolls the pods.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tls: Option<ServerTls>,
}

impl Default for Server {
    fn default() -> Self {
        Self {
            bind_address: "0.0.0.0".into(),
            port: None,
            query_timeout: None,
            query_timeout_grace_period: None,
            handshake_timeout: None,
            tls: None,
        }
    }
}

/// `[server.tls]` (`listeners.rs:48-82`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ServerTls {
    pub server_certificate_bundle: String,
    pub server_private_key: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_ca_bundle: Option<String>,
    /// Angos: `optional`; `required` needs `client_ca_bundle`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_auth: Option<ClientAuth>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ClientAuth {
    Optional,
    Required,
}

/// A client's TLS, always flattened into its table (`crates/mtls-client`):
/// certificate and key both or neither.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ClientTls {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub server_ca_bundle: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_certificate_bundle: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_private_key: Option<String>,
}

/// `[global]` (`src/configuration/global.rs:37-133`). Sizes are
/// [`ByteSize`] strings (`"5MiB"`); counts marked "> 0" are Angos's
/// `NonZero*`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Global {
    /// > 0. Angos: 64. Startup only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_concurrent_requests: Option<u64>,
    /// > 0. Angos: 4.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_concurrent_cache_jobs: Option<u64>,
    /// > 0. Angos: 4.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_concurrent_replication_jobs: Option<u64>,
    /// > 0. Angos: 2.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_concurrent_scan_jobs: Option<u64>,
    /// > 0. Angos: 4.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_concurrent_index_jobs: Option<u64>,
    /// > 0. Angos: `50GiB`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_index_size: Option<ByteSize>,
    /// > 0. Angos: 500000.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_index_entries: Option<u64>,
    /// > 0. Angos: `5MiB`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_manifest_size: Option<ByteSize>,
    /// > 0. Angos: `100GiB`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_blob_size: Option<ByteSize>,
    /// > 0. Angos: `128KiB`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blob_stream_frame_size: Option<ByteSize>,
    /// Angos: false.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub update_pull_time: Option<bool>,
    /// Angos: true.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enable_blob_redirect: Option<bool>,
    /// Angos: true.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enable_manifest_redirect: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub access_policy: Option<Policy<AccessMode>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retention_policy: Option<RetentionPolicy>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub immutable_tags: Option<bool>,
    /// Regexes, unanchored.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub immutable_tags_exclusions: Vec<String>,
    /// Angos: true.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allow_missing_manifest_references: Option<bool>,
    /// An `auth.webhook` key.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub authorization_webhook: Option<String>,
    /// `event_webhook` keys.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub event_webhooks: Vec<String>,
    /// Present (even empty): the durable job queue. Startup only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub job_queue: Option<JobQueue>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scan: Option<Scan>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub index: Option<Policy<IndexAction>>,
    /// Angos: 30. Startup only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shutdown_drain_secs: Option<u64>,
    /// > 0. Angos: 128.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub namespace_walk_concurrency: Option<u64>,
    /// > 0. Angos: 16.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub listing_read_concurrency: Option<u64>,
    /// > 0. Angos: 100000.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revision_cache_entries: Option<u64>,
    /// Angos: 300.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gc_grace_secs: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pull_history: Option<PullHistory>,
    /// IPs or CIDRs.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub trusted_proxies: Vec<String>,
}

impl Global {
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }
}

/// A size as `bytesize` reads it: `<integer><unit>` (spec: units `B`,
/// `KB`/`KiB`, `MB`/`MiB`, `GB`/`GiB`, `TB`/`TiB`; decimal and binary
/// differ).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ByteSize(pub String);

impl ByteSize {
    /// The size in bytes, `None` when not of the accepted form.
    pub fn bytes(&self) -> Option<u64> {
        let text = self.0.trim();
        let split = text
            .find(|c: char| !c.is_ascii_digit())
            .unwrap_or(text.len());
        let (number, unit) = text.split_at(split);
        let number: u64 = number.parse().ok()?;
        let factor: u64 = match unit.trim() {
            "" | "B" => 1,
            "KB" => 1_000,
            "KiB" => 1 << 10,
            "MB" => 1_000_000,
            "MiB" => 1 << 20,
            "GB" => 1_000_000_000,
            "GiB" => 1 << 30,
            "TB" => 1_000_000_000_000,
            "TiB" => 1 << 40,
            _ => return None,
        };
        number.checked_mul(factor)
    }
}

/// `PolicyConfig<A>` (`src/policy/config.rs`): a matching rule yields the
/// opposite of `default`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(bound(deserialize = "A: Deserialize<'de>"))]
pub struct Policy<A> {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<A>,
    /// CEL rules.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub rules: Vec<String>,
}

impl<A> Default for Policy<A> {
    fn default() -> Self {
        Self {
            default: None,
            rules: Vec::new(),
        }
    }
}

impl<A> Policy<A> {
    /// Angos's `is_set`: a default or a rule.
    pub fn is_set(&self) -> bool {
        self.default.is_some() || !self.rules.is_empty()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AccessMode {
    Allow,
    Deny,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ScanAction {
    Scan,
    Skip,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum IndexAction {
    Index,
    Skip,
}

/// `RetentionPolicyConfig`: any matching rule retains.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct RetentionPolicy {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub rules: Vec<String>,
}

/// `[global.pull_history]` (`src/registry/metadata_store/mod.rs:158-193`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct PullHistory {
    /// > 0. Angos: 1000.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_pulls: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_age_secs: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compact_after_secs: Option<u64>,
    /// > 0.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compact_after_pulls: Option<u64>,
}

/// `[global.job_queue]` (`src/jobs/store.rs:211-310`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct JobQueue {
    /// ≥ 3. Angos: 60.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub claim_ttl_secs: Option<u64>,
    /// ≥ 5. Angos: 15.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pending_refresh_interval_secs: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pending_ready_horizon_secs: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_attempts: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry_backoff_min_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry_backoff_max_ms: Option<u64>,
}

/// `[global.scan]` (`src/scan/mod.rs:43-75`): the scanner's endpoint and
/// the global scan policy.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Scan {
    pub url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token: Option<String>,
    /// Angos: 600.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_secs: Option<u64>,
    #[serde(flatten)]
    pub tls: ClientTls,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<ScanAction>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub rules: Vec<String>,
}

/// `[ui]` (`src/configuration/ui.rs`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Ui {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub oidc: Option<UiOidc>,
}

/// `[ui.oidc]`: a public PKCE client.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct UiOidc {
    /// An `auth.oidc` key.
    pub provider: String,
    pub client_id: String,
    /// One space-separated string. Angos: `openid profile email`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scopes: Option<String>,
}

/// `cache`: `"memory"` or `[cache.redis]` (`crates/cache/src/config.rs`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Cache {
    #[serde(rename = "memory")]
    Memory,
    #[serde(rename = "redis")]
    Redis(Redis),
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Redis {
    pub url: String,
    /// Required by Angos.
    pub key_prefix: String,
}

/// `blob_store` / `metadata_store`: `[*.fs]` or `[*.s3]`
/// (`src/registry/blob_store/config.rs`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Store {
    #[serde(rename = "fs")]
    Fs(FsStore),
    #[serde(rename = "s3")]
    S3(Box<S3Store>),
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct FsStore {
    /// Required, not empty.
    pub root_dir: String,
    /// Angos: false.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sync_to_disk: Option<bool>,
}

/// `S3ConnectionConfig` + `TransportFields`, one flat table.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct S3Store {
    pub access_key_id: String,
    pub secret_key: String,
    pub endpoint: String,
    pub bucket: String,
    pub region: String,
    /// Angos: `""`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key_prefix: Option<String>,
    /// Angos: `5GB`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub multipart_copy_threshold: Option<ByteSize>,
    /// Angos: `100MB`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub multipart_copy_chunk_size: Option<ByteSize>,
    /// Angos: 4.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub multipart_copy_jobs: Option<u64>,
    /// Angos: `50MiB`; S3 needs ≥ 5 MiB (Angos raises it silently).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub multipart_part_size: Option<ByteSize>,
    /// Angos: false.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub multipart_uniform_parts: Option<bool>,
    /// Angos: 900.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub operation_timeout_secs: Option<u64>,
    /// Angos: 300.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub operation_attempt_timeout_secs: Option<u64>,
    /// Angos: 3.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_attempts: Option<u32>,
    /// Angos: 5.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub circuit_breaker_threshold: Option<u32>,
    /// Angos: 10.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub circuit_breaker_cooldown_secs: Option<u64>,
    /// Angos: 16.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub children_scan_concurrency: Option<u64>,
    /// Angos: 1800.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub presign_ttl_secs: Option<u64>,
}

/// `[auth]` (`src/auth/authenticator.rs:22-32`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Auth {
    /// `[auth.identity.<id>]`.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub identity: BTreeMap<String, Identity>,
    /// `[auth.oidc.<provider>]`.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub oidc: BTreeMap<String, Oidc>,
    /// `[auth.webhook.<name>]`.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub webhook: BTreeMap<String, AuthWebhook>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token_service: Option<TokenService>,
}

impl Auth {
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }
}

/// `[auth.identity.<id>]`: a Basic identity.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Identity {
    pub username: String,
    /// An Argon2 PHC string.
    pub password: String,
}

/// `[auth.oidc.<provider>]` (`src/auth/oidc/mod.rs:29-85`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Oidc {
    /// Compared exactly with the issuer discovered (trailing slash
    /// included).
    pub issuer: String,
    #[serde(flatten)]
    pub tls: ClientTls,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bearer_token_file: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub jwks_uri: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub required_claims: Vec<String>,
    /// Angos: 3600.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub jwks_refresh_interval: Option<u64>,
    /// Unset, Angos accepts any audience: the operator always sets it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub required_audience: Option<String>,
    /// Angos: 60.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clock_skew_tolerance: Option<u64>,
    /// Angos: `["RS256"]`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub allowed_algorithms: Vec<String>,
    /// Angos: 30.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub http_request_timeout_secs: Option<u64>,
    /// Angos: 5.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub jwks_refresh_timeout_secs: Option<u64>,
}

/// `[auth.webhook.<name>]` (`src/auth/webhook/config.rs`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct AuthWebhook {
    pub url: String,
    /// Required by Angos.
    pub timeout_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bearer_token: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub basic_auth: Option<BasicAuth>,
    #[serde(flatten)]
    pub tls: ClientTls,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub forward_headers: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct BasicAuth {
    pub username: String,
    pub password: String,
}

/// `[auth.token_service]` (`src/auth/token_service.rs:52-66`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct TokenService {
    /// Base64 (padded), ≥ 32 bytes decoded.
    pub secret_key: String,
    /// `http(s)://<host>/…/token`. Unset, Angos derives it from the
    /// request's `Host`: the operator always sets it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub realm: Option<String>,
    /// 1..=86400. Angos: 3600.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ttl_secs: Option<u64>,
}

/// `[repository."<prefix>"]` (`src/registry/repository.rs:202-241`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Repository {
    /// The `?ns=` value it answers to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub namespace: Option<String>,
    /// Non-empty: a pull-through cache (pushes refused).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub upstream: Vec<Upstream>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub downstream: Vec<Downstream>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub access_policy: Option<Policy<AccessMode>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retention_policy: Option<RetentionPolicy>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub immutable_tags: Option<bool>,
    /// Non-empty: replaces the global list.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub immutable_tags_exclusions: Vec<String>,
    /// An `auth.webhook` key. `""` counts as unset in Angos (the global
    /// webhook still applies).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub authorization_webhook: Option<String>,
    /// Non-empty: replaces the global list.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub event_webhooks: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scan: Option<Policy<ScanAction>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub index: Option<Policy<IndexAction>>,
}

/// `[[repository."<prefix>".upstream]]` (`crates/oci-client/src/lib.rs`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Upstream {
    pub url: String,
    /// Angos: 5.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_redirect: Option<u8>,
    /// Angos: 30.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub connect_timeout_secs: Option<u64>,
    /// Angos: 300.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub read_timeout_secs: Option<u64>,
    #[serde(flatten)]
    pub tls: ClientTls,
    /// With `password`, both or neither.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub username: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub password: Option<String>,
}

/// `[[repository."<prefix>".downstream]]` (`src/replication/config.rs`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Downstream {
    /// Not empty, unique in its repository.
    pub name: String,
    #[serde(flatten)]
    pub client: Upstream,
    /// Angos: `event+reconcile`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<ReplicationMode>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub namespace_filter: Vec<String>,
    /// > 0. Angos: 4.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_concurrent_pushes: Option<u64>,
    /// Propagates deletions. Angos: false.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prune: Option<bool>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ReplicationMode {
    #[serde(rename = "event+reconcile")]
    EventAndReconcile,
    #[serde(rename = "event-only")]
    EventOnly,
    #[serde(rename = "reconcile-only")]
    ReconcileOnly,
}

/// `[event_webhook.<name>]` (`src/event_webhook/config.rs`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EventWebhook {
    pub url: String,
    pub policy: DeliveryPolicy,
    /// Not empty when set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token: Option<String>,
    /// Angos: 5000.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<u64>,
    /// ≤ 16. Angos: 3 for `required`, 0 otherwise.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_retries: Option<u32>,
    /// Not empty.
    pub events: Vec<EventKind>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repository_filter: Option<Vec<String>>,
    #[serde(flatten)]
    pub tls: ClientTls,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DeliveryPolicy {
    Required,
    Optional,
    Async,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
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

/// `[observability]` (`src/configuration/observability.rs`). Startup only.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Observability {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tracing: Option<Tracing>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Tracing {
    pub endpoint: String,
    /// In `[0.0, 1.0]`.
    pub sampling_rate: f64,
}

/// The two files of an instance (spec 2.3).
#[derive(Debug, Clone, PartialEq)]
pub struct Layers {
    /// Read at startup: a new `Secret`, a rollout.
    pub boot: toml::Table,
    /// Reloaded by Angos.
    pub hot: toml::Table,
}

impl Layers {
    /// Each layer as the file Angos reads.
    pub fn files(&self) -> (String, String) {
        (render(&self.boot), render(&self.hot))
    }
}

/// `table` as a TOML file.
pub fn render(table: &toml::Table) -> String {
    toml::to_string(table).expect("a TOML table renders")
}

impl AngosConfig {
    /// The whole file as one table.
    pub fn to_table(&self) -> toml::Table {
        toml::Table::try_from(self).expect("the configuration model serializes to a TOML table")
    }

    /// The two layers: [`BOOT_KEYS`] in the startup one, the rest in the
    /// hot one.
    pub fn split(&self) -> Layers {
        let mut hot = self.to_table();
        let mut boot = toml::Table::new();
        for path in BOOT_KEYS {
            if let Some(value) = take(&mut hot, path) {
                put(&mut boot, path, value);
            }
        }
        Layers { boot, hot }
    }
}

/// Removes the value at `path`, and the tables the removal leaves empty
/// (a table empty from the start, `[global.job_queue]`, means something
/// and is kept).
fn take(table: &mut toml::Table, path: &[&str]) -> Option<toml::Value> {
    match path {
        [] => None,
        [key] => table.remove(*key),
        [key, rest @ ..] => {
            let toml::Value::Table(inner) = table.get_mut(*key)? else {
                return None;
            };
            let value = take(inner, rest)?;
            if inner.is_empty() {
                table.remove(*key);
            }
            Some(value)
        }
    }
}

fn put(table: &mut toml::Table, path: &[&str], value: toml::Value) {
    match path {
        [] => {}
        [key] => {
            table.insert((*key).to_string(), value);
        }
        [key, rest @ ..] => {
            let inner = table
                .entry((*key).to_string())
                .or_insert_with(|| toml::Value::Table(toml::Table::new()));
            if let toml::Value::Table(inner) = inner {
                put(inner, rest, value);
            }
        }
    }
}

/// Angos's merge of two files (`src/configuration/merge.rs:6-33`): tables
/// merged recursively, any other value of `overlay` replacing `base`'s
/// whole (arrays included).
pub fn merge(base: &mut toml::Table, overlay: toml::Table) {
    for (key, value) in overlay {
        match (base.get_mut(&key), value) {
            (Some(toml::Value::Table(base)), toml::Value::Table(overlay)) => merge(base, overlay),
            (_, value) => {
                base.insert(key, value);
            }
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A configuration setting every section, with arrays everywhere one
    /// can be.
    pub(crate) fn full() -> AngosConfig {
        let tls = ClientTls {
            server_ca_bundle: Some("/etc/angos/ca/ca.crt".into()),
            client_certificate_bundle: Some("/etc/angos/client/tls.crt".into()),
            client_private_key: Some("/etc/angos/client/tls.key".into()),
        };
        let upstream = Upstream {
            url: "https://registry-1.docker.io".into(),
            username: Some("u".into()),
            password: Some("p".into()),
            tls: tls.clone(),
            ..Default::default()
        };
        AngosConfig {
            server: Server {
                port: Some(8000),
                query_timeout: Some(3600),
                tls: Some(ServerTls {
                    server_certificate_bundle: "/etc/angos/tls/tls.crt".into(),
                    server_private_key: "/etc/angos/tls/tls.key".into(),
                    client_ca_bundle: Some("/etc/angos/tls/ca.crt".into()),
                    client_auth: Some(ClientAuth::Optional),
                }),
                ..Default::default()
            },
            global: Global {
                max_concurrent_requests: Some(64),
                max_manifest_size: Some(ByteSize("5MiB".into())),
                access_policy: Some(Policy {
                    default: Some(AccessMode::Deny),
                    rules: vec!["request.action in ['healthz', 'readyz']".into()],
                }),
                retention_policy: Some(RetentionPolicy {
                    rules: vec!["top_pushed(10)".into()],
                }),
                immutable_tags_exclusions: vec!["^latest$".into()],
                event_webhooks: vec!["audit".into()],
                job_queue: Some(JobQueue::default()),
                scan: Some(Scan {
                    url: "http://scanner:8766".into(),
                    rules: vec!["true".into()],
                    tls: tls.clone(),
                    ..Default::default()
                }),
                shutdown_drain_secs: Some(30),
                trusted_proxies: vec!["10.0.0.0/8".into()],
                ..Default::default()
            },
            ui: Some(Ui {
                enabled: Some(true),
                oidc: Some(UiOidc {
                    provider: "weebo".into(),
                    client_id: "angos".into(),
                    scopes: None,
                }),
                ..Default::default()
            }),
            cache: Some(Cache::Redis(Redis {
                url: "redis://valkey:6379".into(),
                key_prefix: "angos:".into(),
            })),
            blob_store: Some(Store::S3(Box::new(S3Store {
                access_key_id: "a".into(),
                secret_key: "s".into(),
                endpoint: "http://rustfs:9000".into(),
                bucket: "angos".into(),
                region: "us-east-1".into(),
                ..Default::default()
            }))),
            metadata_store: Some(Store::Fs(FsStore {
                root_dir: "/data/metadata".into(),
                sync_to_disk: None,
            })),
            auth: Auth {
                identity: BTreeMap::from([(
                    "scanner".into(),
                    Identity {
                        username: "scanner".into(),
                        password: "$argon2id$v=19$m=19456,t=2,p=1$c2FsdA$aGFzaA".into(),
                    },
                )]),
                oidc: BTreeMap::from([(
                    "weebo".into(),
                    Oidc {
                        issuer: "https://auth.example/application/o/angos/".into(),
                        required_audience: Some("angos".into()),
                        required_claims: vec!["sub".into()],
                        allowed_algorithms: vec!["RS256".into()],
                        ..Default::default()
                    },
                )]),
                webhook: BTreeMap::from([(
                    "opa".into(),
                    AuthWebhook {
                        url: "http://opa:8181/v1".into(),
                        timeout_ms: 500,
                        forward_headers: vec!["X-Forwarded-For".into()],
                        ..Default::default()
                    },
                )]),
                token_service: Some(TokenService {
                    secret_key: "a".repeat(44),
                    realm: Some("https://registry.example/token".into()),
                    ttl_secs: Some(3600),
                }),
            },
            repository: BTreeMap::from([(
                "dockerhub".into(),
                Repository {
                    upstream: vec![upstream.clone()],
                    downstream: vec![Downstream {
                        name: "dr".into(),
                        client: upstream,
                        namespace_filter: vec!["^library/".into()],
                        ..Default::default()
                    }],
                    access_policy: Some(Policy {
                        default: Some(AccessMode::Deny),
                        rules: vec!["identity.username != null".into()],
                    }),
                    event_webhooks: vec!["audit".into()],
                    scan: Some(Policy {
                        default: Some(ScanAction::Skip),
                        rules: vec!["true".into()],
                    }),
                    ..Default::default()
                },
            )]),
            event_webhook: BTreeMap::from([(
                "audit".into(),
                EventWebhook {
                    url: "https://audit.example/hook".into(),
                    policy: DeliveryPolicy::Async,
                    token: Some("t".into()),
                    timeout_ms: None,
                    max_retries: Some(3),
                    events: vec![EventKind::ManifestPush, EventKind::TagDelete],
                    repository_filter: Some(vec!["^team/".into()]),
                    tls: ClientTls::default(),
                },
            )]),
            observability: Some(Observability {
                tracing: Some(Tracing {
                    endpoint: "http://otel:4317".into(),
                    sampling_rate: 0.1,
                }),
            }),
        }
    }

    /// Every array of `table`, by its path.
    fn arrays(table: &toml::Table, prefix: &str, out: &mut Vec<String>) {
        for (key, value) in table {
            let path = format!("{prefix}{key}");
            match value {
                toml::Value::Array(_) => out.push(path),
                toml::Value::Table(inner) => arrays(inner, &format!("{path}."), out),
                _ => {}
            }
        }
    }

    #[test]
    fn the_layers_merge_back_into_the_whole_file() {
        let config = full();
        let layers = config.split();
        let mut merged = layers.boot.clone();
        merge(&mut merged, layers.hot.clone());
        assert_eq!(merged, config.to_table());
        // What reads back from the merged file is the model again.
        let back: AngosConfig = toml::from_str(&render(&merged)).unwrap();
        assert_eq!(back, config);
    }

    #[test]
    fn no_array_is_spread_over_both_layers() {
        let layers = full().split();
        let (mut boot, mut hot) = (Vec::new(), Vec::new());
        arrays(&layers.boot, "", &mut boot);
        arrays(&layers.hot, "", &mut hot);
        assert!(!hot.is_empty());
        for path in &boot {
            assert!(!hot.contains(path), "{path} is in both layers");
        }
        // A boot key never sits under an array either: the cut is at table
        // keys only.
        for path in BOOT_KEYS {
            let joined = path.join(".");
            assert!(
                !hot.iter().any(|a| joined.starts_with(&format!("{a}."))),
                "{joined}"
            );
        }
    }

    #[test]
    fn the_startup_layer_holds_the_storage_and_the_startup_keys_only() {
        let layers = full().split();
        let keys = |t: &toml::Table| t.keys().cloned().collect::<Vec<_>>();
        assert_eq!(
            keys(&layers.boot),
            [
                "blob_store",
                "global",
                "metadata_store",
                "observability",
                "server"
            ]
        );
        let toml::Value::Table(server) = &layers.boot["server"] else {
            panic!("server is a table");
        };
        assert_eq!(keys(server), ["bind_address", "port"]);
        let toml::Value::Table(global) = &layers.boot["global"] else {
            panic!("global is a table");
        };
        assert_eq!(
            keys(global),
            [
                "job_queue",
                "max_concurrent_requests",
                "shutdown_drain_secs"
            ]
        );
        // The storage never reaches the hot layer, TLS paths do.
        assert!(!layers.hot.contains_key("blob_store"));
        assert!(!layers.hot.contains_key("metadata_store"));
        let toml::Value::Table(server) = &layers.hot["server"] else {
            panic!("server is a table");
        };
        assert_eq!(keys(server), ["query_timeout", "tls"]);
    }

    #[test]
    fn an_empty_table_that_means_something_survives_the_cut() {
        let mut config = AngosConfig::default();
        config.global.job_queue = Some(JobQueue::default());
        let layers = config.split();
        let (boot, hot) = layers.files();
        assert!(boot.contains("[global.job_queue]"), "{boot}");
        assert!(!hot.contains("global"), "{hot}");
    }

    #[test]
    fn merging_replaces_arrays_whole() {
        let mut base: toml::Table = toml::from_str("a = [1, 2]\n[t]\nx = 1\ny = 2").unwrap();
        merge(
            &mut base,
            toml::from_str("a = [3]\n[t]\ny = 3\nz = 4").unwrap(),
        );
        assert_eq!(
            base,
            toml::from_str::<toml::Table>("a = [3]\n[t]\nx = 1\ny = 3\nz = 4").unwrap()
        );
    }

    #[test]
    fn sizes_read_as_bytesize_does() {
        let size = |s: &str| ByteSize(s.into()).bytes();
        assert_eq!(size("5MiB"), Some(5 << 20));
        assert_eq!(size("5MB"), Some(5_000_000));
        assert_eq!(size("128KiB"), Some(128 << 10));
        assert_eq!(size("0GiB"), Some(0));
        assert_eq!(size("42"), Some(42));
        assert_eq!(size("5 MiB"), Some(5 << 20));
        assert_eq!(size("5mb"), None);
        assert_eq!(size("MiB"), None);
    }

    #[test]
    fn the_server_always_binds_every_address() {
        let table = AngosConfig::default().to_table();
        assert_eq!(table["server"]["bind_address"].as_str(), Some("0.0.0.0"));
    }
}
