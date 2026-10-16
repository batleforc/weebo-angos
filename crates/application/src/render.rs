//! An `AngosLocalInstance` as Angos's configuration (spec 2.1–2.3): the
//! typed spec and the values its Secrets hold (read beforehand,
//! [`Resolved`]) rendered into the model of `domain::config`, checked with
//! `domain::validation`, then cut into the startup and hot layers, with
//! `extraConfig` merged last into the hot one.
//!
//! Pure: no client; what pods mount is named here ([`paths`]) and built
//! by `crate::workload`.

use std::collections::BTreeMap;

use api::config::{ClientAuth as SpecClientAuth, FsStorage, S3Storage};
use api::local_instance::LocalInstanceSpec;
use domain::config::{
    AccessMode, AngosConfig, ByteSize, Cache, ClientAuth, ClientTls, FsStore, Global, JobQueue,
    Layers, Observability, Oidc, Policy, PullHistory, Redis, S3Store, Server, ServerTls, Store,
    TokenService, Tracing, Ui, UiOidc, render as render_table,
};
use domain::validation::{self, Violation};
use weebo_kit_application::workload::content_hash;

/// Where the pods find what the configuration names.
pub mod paths {
    /// The startup layer (`<instance>-boot-<hash8>`).
    pub const BOOT_DIR: &str = "/etc/angos/boot";
    /// The hot layer (`<instance>-config`), watched by Angos.
    pub const HOT_DIR: &str = "/etc/angos/hot";
    /// Both files' name in their directory.
    pub const FILE: &str = "config.toml";
    /// The server's `kubernetes.io/tls` Secret.
    pub const TLS_DIR: &str = "/etc/angos/tls";
    /// The client CA of `server.tls.clientCA`.
    pub const CLIENT_CA_DIR: &str = "/etc/angos/client-ca";
    /// The client CA's file name.
    pub const CLIENT_CA_FILE: &str = "ca.crt";
    /// The instance's account token (API server audience) and the
    /// cluster's CA, for `auth.kubernetes`: Angos reads the JWKS with them.
    pub const KUBE_DIR: &str = "/var/run/secrets/angos/kube";
    /// The default JWKS of the cluster's ServiceAccount tokens.
    pub const KUBE_JWKS: &str = "https://kubernetes.default.svc/openid/v1/jwks";
}

/// What the instance's Secrets hold, read before rendering.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Resolved {
    /// `(access key id, secret key)` of the blob store, when S3.
    pub s3: Option<(String, String)>,
    /// The same for an S3 metadata store.
    pub metadata_s3: Option<(String, String)>,
    pub redis_url: Option<String>,
    /// The token service's key (base64).
    pub token_key: Option<String>,
    /// Per OIDC provider name: `(issuer, audience)`.
    pub oidc: BTreeMap<String, (String, String)>,
    pub ui_client_id: Option<String>,
    /// The cluster's ServiceAccount issuer, for `auth.kubernetes` without
    /// an explicit one.
    pub cluster_issuer: Option<String>,
}

/// The two files of an instance and what tells them apart.
#[derive(Debug, Clone, PartialEq)]
pub struct Rendered {
    pub layers: Layers,
    pub boot_file: String,
    pub hot_file: String,
    /// The startup layer's hash — with whether TLS is on, which Angos
    /// can't change hot (spec 2.3): it names the `Secret` and rolls the
    /// pods.
    pub boot_hash: String,
    /// The hot layer's hash.
    pub hot_hash: String,
    /// `extraConfig` was merged.
    pub extra_config_used: bool,
}

/// Why an instance can't be rendered: each problem names the spec field
/// or the Angos key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenderError(pub Vec<String>);

impl std::fmt::Display for RenderError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let shown: Vec<&str> = self.0.iter().take(5).map(String::as_str).collect();
        write!(f, "{}", shown.join("; "))?;
        if self.0.len() > 5 {
            write!(f, " and {} more", self.0.len() - 5)?;
        }
        Ok(())
    }
}

/// The access rules always rendered first in the global policy (spec
/// 2.6): the probes, the UI's assets, `/token` and the API version for
/// anyone authenticated.
pub fn builtin_rules(ui_enabled: bool) -> Vec<String> {
    let mut rules = vec!["request.action in ['healthz', 'readyz']".to_string()];
    if ui_enabled {
        rules.push("request.action in ['ui-asset', 'ui-config']".into());
    }
    rules.push(
        "request.action in ['get-api-version', 'get-token'] && (identity.username != null || \
         identity.oidc != null || size(identity.certificate.common_names) > 0)"
            .into(),
    );
    rules
}

/// The rule always rendered last: a namespace that no repository declares
/// is refused (registries are declared, not discovered).
pub const REPOSITORY_RULE: &str = "has_repository_policy()";

fn s3_store(spec: &S3Storage, credentials: Option<&(String, String)>) -> S3Store {
    let (access_key_id, secret_key) = credentials.cloned().unwrap_or_default();
    let t = spec.transport.clone().unwrap_or_default();
    S3Store {
        access_key_id,
        secret_key,
        endpoint: spec.endpoint.clone(),
        bucket: spec.bucket.clone(),
        region: spec.region.clone(),
        key_prefix: spec.key_prefix.clone(),
        multipart_copy_threshold: t.multipart_copy_threshold.map(ByteSize),
        multipart_copy_chunk_size: t.multipart_copy_chunk_size.map(ByteSize),
        multipart_copy_jobs: t.multipart_copy_jobs,
        multipart_part_size: t.multipart_part_size.map(ByteSize),
        multipart_uniform_parts: t.multipart_uniform_parts,
        operation_timeout_secs: t.operation_timeout_secs,
        operation_attempt_timeout_secs: t.operation_attempt_timeout_secs,
        max_attempts: t.max_attempts,
        circuit_breaker_threshold: t.circuit_breaker_threshold,
        circuit_breaker_cooldown_secs: t.circuit_breaker_cooldown_secs,
        children_scan_concurrency: t.children_scan_concurrency,
        presign_ttl_secs: t.presign_ttl_secs,
    }
}

fn fs_store(spec: &FsStorage) -> FsStore {
    FsStore {
        root_dir: spec.root_dir.clone(),
        sync_to_disk: spec.sync_to_disk,
    }
}

/// The configuration model of `spec`, its Secrets' values filled.
pub fn model(spec: &LocalInstanceSpec, resolved: &Resolved) -> AngosConfig {
    let mut config = AngosConfig::default();

    // Server.
    let server = &spec.server;
    config.server = Server {
        port: Some(server.port),
        query_timeout: server.query_timeout_secs,
        query_timeout_grace_period: server.query_timeout_grace_period_secs,
        handshake_timeout: server.handshake_timeout_secs,
        tls: server.tls.as_ref().map(|tls| ServerTls {
            server_certificate_bundle: format!("{}/tls.crt", paths::TLS_DIR),
            server_private_key: format!("{}/tls.key", paths::TLS_DIR),
            client_ca_bundle: tls
                .client_ca
                .as_ref()
                .map(|_| format!("{}/{}", paths::CLIENT_CA_DIR, paths::CLIENT_CA_FILE)),
            client_auth: Some(match tls.client_auth {
                SpecClientAuth::Optional => ClientAuth::Optional,
                SpecClientAuth::Required => ClientAuth::Required,
            }),
        }),
        ..Default::default()
    };

    // Storage.
    let storage = &spec.storage;
    config.blob_store = match (&storage.s3, &storage.fs) {
        (Some(s3), _) => Some(Store::S3(Box::new(s3_store(s3, resolved.s3.as_ref())))),
        (None, Some(fs)) => Some(Store::Fs(fs_store(fs))),
        (None, None) => None,
    };
    config.metadata_store = storage
        .metadata
        .as_ref()
        .and_then(|m| match (&m.s3, &m.fs) {
            (Some(s3), _) => Some(Store::S3(Box::new(s3_store(
                s3,
                resolved.metadata_s3.as_ref(),
            )))),
            (None, Some(fs)) => Some(Store::Fs(fs_store(fs))),
            (None, None) => None,
        });

    // Global.
    let g = &spec.global;
    let jobs = g.jobs.clone().unwrap_or_default();
    let limits = g.limits.clone().unwrap_or_default();
    let listing = g.listing.clone().unwrap_or_default();
    let redirects = g.redirects.clone().unwrap_or_default();
    let ui_enabled = spec.ui.as_ref().is_some_and(|ui| ui.enabled);
    let mut rules = builtin_rules(ui_enabled);
    rules.push(REPOSITORY_RULE.into());
    config.global = Global {
        max_concurrent_requests: g.max_concurrent_requests,
        max_concurrent_cache_jobs: jobs.cache,
        max_concurrent_replication_jobs: jobs.replication,
        max_concurrent_scan_jobs: jobs.scan,
        max_concurrent_index_jobs: jobs.index,
        max_index_size: limits.max_index_size.map(ByteSize),
        max_index_entries: limits.max_index_entries,
        max_manifest_size: limits.max_manifest_size.map(ByteSize),
        max_blob_size: limits.max_blob_size.map(ByteSize),
        blob_stream_frame_size: limits.blob_stream_frame_size.map(ByteSize),
        update_pull_time: g.update_pull_time,
        // The operator's default differs from Angos's: no redirect to S3
        // unless asked (the clients must reach the bucket).
        enable_blob_redirect: Some(redirects.blob),
        enable_manifest_redirect: Some(redirects.manifest),
        access_policy: Some(Policy {
            default: Some(AccessMode::Deny),
            rules,
        }),
        retention_policy: None,
        immutable_tags: g.immutable_tags,
        immutable_tags_exclusions: g.immutable_tags_exclusions.clone(),
        allow_missing_manifest_references: g.allow_missing_manifest_references,
        authorization_webhook: None,
        event_webhooks: Vec::new(),
        job_queue: None::<JobQueue>,
        scan: None,
        index: None,
        shutdown_drain_secs: g.shutdown_drain_secs,
        namespace_walk_concurrency: listing.namespace_walk_concurrency,
        listing_read_concurrency: listing.listing_read_concurrency,
        revision_cache_entries: listing.revision_cache_entries,
        gc_grace_secs: g.gc_grace_secs,
        pull_history: g.pull_history.as_ref().map(|h| PullHistory {
            max_pulls: h.max_pulls,
            max_age_secs: h.max_age_secs,
            compact_after_secs: h.compact_after_secs,
            compact_after_pulls: h.compact_after_pulls,
        }),
        trusted_proxies: server.trusted_proxies.clone(),
    };

    // Cache.
    config.cache = spec.cache.as_ref().map(|cache| {
        Cache::Redis(Redis {
            url: resolved.redis_url.clone().unwrap_or_default(),
            key_prefix: cache.redis.key_prefix.clone(),
        })
    });

    // UI.
    config.ui = spec.ui.as_ref().map(|ui| Ui {
        enabled: Some(ui.enabled),
        name: ui.name.clone(),
        oidc: ui.oidc.as_ref().map(|oidc| UiOidc {
            provider: oidc.provider.clone(),
            client_id: oidc
                .client_id
                .clone()
                .or_else(|| resolved.ui_client_id.clone())
                .unwrap_or_default(),
            scopes: (!oidc.scopes.is_empty()).then(|| oidc.scopes.join(" ")),
        }),
    });

    // Authentication.
    for provider in &spec.auth.oidc {
        let (issuer, audience) = resolved
            .oidc
            .get(&provider.name)
            .cloned()
            .unwrap_or_default();
        config.auth.oidc.insert(
            provider.name.clone(),
            Oidc {
                issuer: provider.issuer.clone().unwrap_or(issuer),
                required_audience: Some(provider.audience.clone().unwrap_or(audience)),
                jwks_uri: provider.jwks_uri.clone(),
                required_claims: provider.required_claims.clone(),
                allowed_algorithms: provider.allowed_algorithms.clone(),
                jwks_refresh_interval: provider.jwks_refresh_interval,
                clock_skew_tolerance: provider.clock_skew_tolerance,
                http_request_timeout_secs: provider.http_request_timeout_secs,
                jwks_refresh_timeout_secs: provider.jwks_refresh_timeout_secs,
                ..Default::default()
            },
        );
    }
    if let Some(kube) = &spec.auth.kubernetes {
        config.auth.oidc.insert(
            kube.name.clone(),
            Oidc {
                issuer: kube
                    .issuer
                    .clone()
                    .or_else(|| resolved.cluster_issuer.clone())
                    .unwrap_or_default(),
                required_audience: Some(kube.audience.clone()),
                jwks_uri: Some(
                    kube.jwks_uri
                        .clone()
                        .unwrap_or_else(|| paths::KUBE_JWKS.into()),
                ),
                bearer_token_file: Some(format!("{}/token", paths::KUBE_DIR)),
                tls: ClientTls {
                    server_ca_bundle: Some(format!("{}/ca.crt", paths::KUBE_DIR)),
                    ..Default::default()
                },
                ..Default::default()
            },
        );
    }
    config.auth.token_service = spec.auth.token_service.as_ref().map(|ts| TokenService {
        secret_key: resolved.token_key.clone().unwrap_or_default(),
        realm: Some(ts.realm.clone()),
        ttl_secs: ts.ttl_secs,
    });

    // Observability.
    config.observability = spec.tracing.as_ref().map(|t| Observability {
        tracing: Some(Tracing {
            endpoint: t.endpoint.clone(),
            sampling_rate: t.sampling_rate,
        }),
    });
    config
}

/// Every leaf path of `table`, dotted.
fn leaves(table: &toml::Table, prefix: &str, out: &mut Vec<String>) {
    for (key, value) in table {
        let path = format!("{prefix}{key}");
        match value {
            toml::Value::Table(inner) if !inner.is_empty() => {
                leaves(inner, &format!("{path}."), out)
            }
            _ => out.push(path),
        }
    }
}

fn lookup<'a>(table: &'a toml::Table, path: &[&str]) -> Option<&'a toml::Value> {
    let (first, rest) = path.split_first()?;
    let value = table.get(*first)?;
    match rest {
        [] => Some(value),
        _ => lookup(value.as_table()?, rest),
    }
}

/// `extraConfig` as a TOML table, refused where it would override what the
/// operator renders, a startup key, or anything of the startup layer.
fn extra(spec: &LocalInstanceSpec, rendered: &toml::Table) -> Result<toml::Table, Vec<String>> {
    let value = serde_json::to_value(&spec.extra_config).expect("a JSON map serializes");
    let table: toml::Table = toml::Table::try_from(value)
        .map_err(|e| vec![format!("spec.extraConfig is not TOML: {e}")])?;
    let mut paths = Vec::new();
    leaves(&table, "", &mut paths);
    let mut problems = Vec::new();
    for path in paths {
        let keys: Vec<&str> = path.split('.').collect();
        if domain::config::BOOT_KEYS
            .iter()
            .any(|boot| keys.starts_with(boot) || boot.starts_with(&keys[..]))
        {
            problems.push(format!(
                "spec.extraConfig.{path}: a startup key, set through the typed fields"
            ));
        } else if keys
            .iter()
            .enumerate()
            .any(|(i, _)| lookup(rendered, &keys[..=i]).is_some_and(|v| !v.is_table()))
        {
            problems.push(format!(
                "spec.extraConfig.{path}: rendered from a typed field of the spec, set it there"
            ));
        }
    }
    if problems.is_empty() {
        Ok(table)
    } else {
        Err(problems)
    }
}

/// `spec` as Angos's two files, checked.
pub fn render(spec: &LocalInstanceSpec, resolved: &Resolved) -> Result<Rendered, RenderError> {
    let config = model(spec, resolved);
    let mut problems: Vec<String> = validation::validate(&config)
        .iter()
        .map(Violation::to_string)
        .collect();
    let mut layers = config.split();
    let extra_config_used = !spec.extra_config.is_empty();
    if extra_config_used {
        match extra(spec, &config.to_table()) {
            Ok(table) => domain::config::merge(&mut layers.hot, table),
            Err(more) => problems.extend(more),
        }
    }
    if !problems.is_empty() {
        return Err(RenderError(problems));
    }
    let (boot_file, hot_file) = layers.files();
    let tls = if spec.server.tls.is_some() {
        "tls"
    } else {
        "plain"
    };
    let boot_hash = content_hash(format!("{boot_file}\n#listener={tls}").as_bytes());
    let hot_hash = content_hash(hot_file.as_bytes());
    Ok(Rendered {
        layers,
        boot_file,
        hot_file,
        boot_hash,
        hot_hash,
        extra_config_used,
    })
}

/// The hot file as written, its table re-rendered (to compare with the
/// one a Secret holds).
pub fn hot_hash_of(file: &str) -> String {
    content_hash(file.as_bytes())
}

/// The startup file of `table` (what a startup `Secret` holds).
pub fn file_of(table: &toml::Table) -> String {
    render_table(table)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn spec(value: serde_json::Value) -> LocalInstanceSpec {
        serde_json::from_value(value).unwrap()
    }

    fn fs() -> LocalInstanceSpec {
        spec(json!({"storage": {"fs": {}}}))
    }

    #[test]
    fn a_minimal_instance_renders_two_valid_files() {
        let rendered = render(&fs(), &Resolved::default()).unwrap();
        assert!(
            rendered.boot_file.contains("[blob_store.fs]"),
            "{}",
            rendered.boot_file
        );
        assert!(rendered.boot_file.contains("bind_address = \"0.0.0.0\""));
        assert!(rendered.boot_file.contains("port = 8000"));
        assert!(!rendered.hot_file.contains("blob_store"));
        // The operator's defaults: no redirect, deny but the probes.
        assert!(rendered.hot_file.contains("enable_blob_redirect = false"));
        assert!(rendered.hot_file.contains("default = \"deny\""));
        assert!(rendered.hot_file.contains("'healthz', 'readyz'"));
        assert!(rendered.hot_file.contains(REPOSITORY_RULE));
        assert!(!rendered.extra_config_used);
    }

    #[test]
    fn the_repository_rule_comes_last() {
        let config = model(&fs(), &Resolved::default());
        let rules = &config.global.access_policy.unwrap().rules;
        assert_eq!(rules.last().map(String::as_str), Some(REPOSITORY_RULE));
        assert_eq!(rules[0], "request.action in ['healthz', 'readyz']");
    }

    #[test]
    fn hashes_follow_their_own_layer() {
        let base = render(&fs(), &Resolved::default()).unwrap();
        let mut hot = fs();
        hot.global.gc_grace_secs = Some(600);
        let hot = render(&hot, &Resolved::default()).unwrap();
        assert_eq!(base.boot_hash, hot.boot_hash);
        assert_ne!(base.hot_hash, hot.hot_hash);
        let mut boot = fs();
        boot.server.port = 9000;
        let boot = render(&boot, &Resolved::default()).unwrap();
        assert_ne!(base.boot_hash, boot.boot_hash);
        assert_eq!(base.hot_hash, boot.hot_hash);
        // TLS on or off rolls, though its keys are hot.
        let tls = spec(json!({"storage": {"fs": {}}, "server": {"tls": {"secretName": "tls"}}}));
        let tls = render(&tls, &Resolved::default()).unwrap();
        assert_ne!(base.boot_hash, tls.boot_hash);
        assert!(tls.hot_file.contains("/etc/angos/tls/tls.crt"));
    }

    #[test]
    fn secrets_fill_the_files() {
        let s = spec(json!({
            "storage": {"s3": {
                "endpoint": "http://s3", "bucket": "b", "region": "us-east-1",
                "credentialsSecretRef": {"name": "s3"},
            }},
            "cache": {"redis": {"urlSecretRef": {"name": "r", "key": "url"}, "keyPrefix": "angos:"}},
            "auth": {
                "oidc": [{"name": "weebo", "issuerFrom": {"name": "a", "key": "AUTHENTIK_URL"},
                          "audience": "angos"}],
                "kubernetes": {"audience": "angos"},
                "tokenService": {"realm": "https://registry.example/token"},
            },
            "ui": {"enabled": true, "oidc": {"provider": "weebo", "clientIdFrom": {"name": "a", "key": "id"}}},
        }));
        let resolved = Resolved {
            s3: Some(("AK".into(), "SK".into())),
            redis_url: Some("redis://valkey:6379".into()),
            token_key: Some("A".repeat(44)),
            oidc: BTreeMap::from([(
                "weebo".into(),
                ("https://auth.example/app/".into(), String::new()),
            )]),
            ui_client_id: Some("client".into()),
            cluster_issuer: Some("https://kubernetes.default.svc.cluster.local".into()),
            ..Default::default()
        };
        let rendered = render(&s, &resolved).unwrap();
        assert!(rendered.boot_file.contains("secret_key = \"SK\""));
        assert!(rendered.hot_file.contains("url = \"redis://valkey:6379\""));
        assert!(
            rendered
                .hot_file
                .contains("issuer = \"https://auth.example/app/\"")
        );
        assert!(rendered.hot_file.contains("required_audience = \"angos\""));
        assert!(rendered.hot_file.contains("client_id = \"client\""));
        assert!(
            rendered
                .hot_file
                .contains("bearer_token_file = \"/var/run/secrets/angos/kube/token\"")
        );
        assert!(rendered.hot_file.contains("'ui-asset', 'ui-config'"));
        // A credentials rotation is a new startup layer.
        let rotated = render(
            &s,
            &Resolved {
                s3: Some(("AK".into(), "SK2".into())),
                ..resolved
            },
        )
        .unwrap();
        assert_ne!(rendered.boot_hash, rotated.boot_hash);
    }

    #[test]
    fn what_angos_would_refuse_is_refused_here() {
        let mut s = fs();
        s.global.limits = Some(api::config::Limits {
            max_manifest_size: Some("0MiB".into()),
            ..Default::default()
        });
        let e = render(&s, &Resolved::default()).unwrap_err();
        assert!(e.to_string().contains("global.max_manifest_size"), "{e}");
        // A token key too short.
        let ts = spec(json!({"storage": {"fs": {}},
            "auth": {"tokenService": {"realm": "https://r.example/token"}}}));
        let e = render(
            &ts,
            &Resolved {
                token_key: Some("c2hvcnQ=".into()),
                ..Default::default()
            },
        )
        .unwrap_err();
        assert!(e.to_string().contains("at least 32 bytes"), "{e}");
    }

    #[test]
    fn extra_config_fills_only_what_the_spec_does_not_render() {
        let mut s = fs();
        s.extra_config = serde_json::from_value(json!({"global": {"max_pulls_hint": 3}})).unwrap();
        let rendered = render(&s, &Resolved::default()).unwrap();
        assert!(rendered.extra_config_used);
        assert!(rendered.hot_file.contains("max_pulls_hint = 3"));
        for refused in [
            json!({"global": {"enable_blob_redirect": true}}),
            json!({"server": {"port": 9000}}),
            json!({"blob_store": {"fs": {"root_dir": "/x"}}}),
            json!({"global": {"access_policy": {"rules": ["true"]}}}),
        ] {
            let mut s = fs();
            s.extra_config = serde_json::from_value(refused.clone()).unwrap();
            assert!(render(&s, &Resolved::default()).is_err(), "{refused}");
        }
    }
}
