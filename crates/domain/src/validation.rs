//! What Angos **1.12.3** refuses when it loads a configuration, checked by
//! the operator before writing it (spec 2.2): at admission and at every
//! reconcile. A refusal here is `InvalidSpec`, nothing is written; what
//! slips through is caught when the pods load the file (spec 6.1).
//!
//! [`RULES`] lists every rule with the code of Angos it reproduces; it is
//! re-read at every upgrade of Angos (spec 9.3). CEL is not compiled here
//! (the `CelChecker` port, spec 2.6): only what needs no CEL engine.
//!
//! Messages name the TOML path of Angos's file; the caller maps it to the
//! CRD field that rendered it.

use std::collections::{BTreeMap, BTreeSet};
use std::net::IpAddr;
use std::sync::LazyLock;

use base64::Engine;
use regex::Regex;

use crate::config::{AngosConfig, ByteSize, ClientAuth, ClientTls, Policy, Store, Upstream};

/// One rule of Angos's loading, and where Angos checks it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rule {
    pub id: &'static str,
    /// The code of Angos 1.12.3 that refuses it (path:lines).
    pub angos: &'static str,
    pub what: &'static str,
}

macro_rules! rules {
    ($($name:ident = $id:literal, $angos:literal, $what:literal;)*) => {
        $(
            #[doc = $what]
            pub const $name: Rule = Rule { id: $id, angos: $angos, what: $what };
        )*
        /// Every rule, in the order they are checked.
        pub const RULES: &[Rule] = &[$($name),*];
    };
}

rules! {
    STORE_ROOT_DIR = "store-root-dir", "src/configuration/mod.rs:275-285",
        "blob_store.fs.root_dir (and metadata_store.fs's) is not empty";
    STORE_S3_FIELDS = "store-s3-fields", "src/registry/s3_connection.rs:16-26",
        "an S3 store has its endpoint, bucket, region and credentials";
    SIZE_FORMAT = "size-format", "bytesize 2.7 (serde)",
        "a size is <integer><unit>, unit B, KB, KiB, MB, MiB, GB, GiB, TB or TiB";
    SIZE_POSITIVE = "size-positive", "src/configuration/mod.rs:292-316",
        "global.max_manifest_size, max_blob_size, max_index_size and blob_stream_frame_size are > 0";
    NON_ZERO = "non-zero", "src/configuration/global.rs:37-133 (NonZero*)",
        "a count Angos reads as NonZero* is > 0";
    PART_SIZE = "s3-part-size", "crates/s3-client (multipart minimum)",
        "multipart_part_size is at least 5 MiB (Angos raises it silently)";
    JOB_QUEUE_FLOORS = "job-queue-floors", "src/jobs/store.rs:296-302",
        "global.job_queue.claim_ttl_secs >= 3 and pending_refresh_interval_secs >= 5";
    TLS_CLIENT_AUTH = "tls-client-auth", "src/configuration/listeners.rs:69-81",
        "server.tls.client_auth = \"required\" needs client_ca_bundle";
    CLIENT_TLS_PAIR = "client-tls-pair", "crates/mtls-client/src/lib.rs:41-60",
        "a client certificate and its key are set together";
    BASIC_AUTH_PAIR = "basic-auth-pair", "crates/mtls-client/src/lib.rs:78-87",
        "an upstream's or downstream's username and password are set together";
    TOKEN_KEY = "token-key", "src/configuration/base64_string.rs:26-34, src/auth/token_service.rs:71-79",
        "auth.token_service.secret_key is padded base64 of at least 32 bytes";
    TOKEN_REALM = "token-realm", "src/auth/token_service.rs:171-201",
        "auth.token_service.realm is an http(s) URL with a host and a path ending in /token";
    TOKEN_TTL = "token-ttl", "src/auth/token_service.rs:120-124",
        "auth.token_service.ttl_secs is in 1..=86400";
    REPOSITORY_PREFIX = "repository-prefix", "src/registry/repository.rs:312-316, crates/oci/src/types/namespace.rs:8-30",
        "a repository's key is an OCI namespace of at most 255 characters";
    PREFIX_OVERLAP = "prefix-overlap", "src/registry/repository_resolver.rs:60-66",
        "no repository's key is a namespace-prefix of another's (team and team/app)";
    NAMESPACE_UNIQUE = "namespace-unique", "src/registry/repository_resolver.rs:69-77",
        "no two repositories answer to the same namespace";
    UPSTREAM_URL = "upstream-url", "src/registry/repository.rs:54-63",
        "an upstream's or downstream's URL is absolute and its path a namespace prefix";
    DOWNSTREAM_NAME = "downstream-name", "src/registry/repository.rs:149-161",
        "a downstream's name is not empty and unique in its repository";
    USERNAME_UNIQUE = "username-unique", "src/auth/basic_auth/mod.rs:81-100",
        "no two identities have the same username";
    USERNAME_NOT_OIDC = "username-not-oidc", "src/auth/authenticator.rs:324-337",
        "no identity's username is the name of an OIDC provider";
    UI_PROVIDER = "ui-provider", "src/configuration/mod.rs:196-208",
        "ui.oidc.provider names an auth.oidc provider";
    WEBHOOK_REF = "webhook-ref", "src/configuration/mod.rs:318-322,343-344,352-365",
        "global and repository authorization_webhook name an auth.webhook";
    EVENT_WEBHOOK_REF = "event-webhook-ref", "src/configuration/mod.rs:324-328,345,369-382",
        "global and repository event_webhooks name an event_webhook";
    EVENT_WEBHOOK_EVENTS = "event-webhook-events", "src/configuration/mod.rs:124-132",
        "an event_webhook lists at least one event";
    EVENT_WEBHOOK_TOKEN = "event-webhook-token", "src/event_webhook/config.rs:55-67",
        "an event_webhook's token is not empty";
    EVENT_WEBHOOK_RETRIES = "event-webhook-retries", "src/event_webhook/config.rs:69-83",
        "an event_webhook's max_retries is at most 16";
    WEBHOOK_URL = "webhook-url", "src/auth/webhook/config.rs:13-57, src/event_webhook/config.rs:17-83 (url::Url)",
        "a webhook's url is an absolute URL";
    FORWARD_HEADERS = "forward-headers", "src/auth/webhook/config.rs:49-57",
        "forward_headers are HTTP header names";
    SCAN_NEEDS_GLOBAL = "scan-needs-global", "src/configuration/mod.rs:338-342",
        "a repository's scan needs [global.scan]";
    POLICY_SET = "policy-set", "src/configuration/mod.rs:157-179",
        "global.index and a repository's scan and index set a default or rules";
    PULL_TIME = "pull-time", "src/configuration/mod.rs:183-190, src/command/prune/mod.rs:55-79",
        "with update_pull_time off, no scan, index or retention rule mentions last_pulled_at or top_pulled";
    EMPTY_RULE = "empty-rule", "src/policy/cel_rule.rs:31-41",
        "a CEL rule is not blank";
    REGEX = "regex", "src/configuration/regex_pattern.rs:64-72",
        "immutable_tags_exclusions, namespace_filter and repository_filter are regexes";
    TRUSTED_PROXY = "trusted-proxy", "src/configuration/trusted_proxy.rs:24-49",
        "a trusted proxy is an IP or a CIDR";
    REDIS = "redis", "crates/cache/src/redis_backend.rs:12-32",
        "cache.redis has a redis URL and a key_prefix";
    SAMPLING_RATE = "sampling-rate", "src/configuration/observability.rs:26-33",
        "observability.tracing.sampling_rate is a finite number in [0, 1]";
    TRACING_ENDPOINT = "tracing-endpoint", "src/configuration/observability.rs:3-46",
        "observability.tracing.endpoint is set";
    PASSWORD_HASH = "password-hash", "src/auth/basic_auth/mod.rs:23-46",
        "an identity's password is an Argon2 PHC string";
}

/// A rule broken, at a TOML path of Angos's file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Violation {
    pub rule: &'static str,
    pub path: String,
    pub message: String,
}

impl std::fmt::Display for Violation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.path, self.message)
    }
}

/// Angos's `Namespace` (`crates/oci/src/types/namespace.rs:11-19`).
static NAMESPACE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"^[a-z0-9]+(?:(?:__|[._]|-+)[a-z0-9]+)*(?:/[a-z0-9]+(?:(?:__|[._]|-+)[a-z0-9]+)*)*$",
    )
    .expect("the namespace pattern compiles")
});

/// An HTTP header name (RFC 9110 token).
static HEADER: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^[!#$%&'*+\-.^_`|~0-9A-Za-z]+$").expect("the header pattern compiles")
});

/// Whether `name` is a namespace Angos accepts.
pub fn is_namespace(name: &str) -> bool {
    name.len() <= 255 && NAMESPACE.is_match(name)
}

/// Angos's `namespace_belongs_to` (`crates/oci/src/types/namespace.rs:112`).
pub fn belongs_to(namespace: &str, prefix: &str) -> bool {
    namespace == prefix
        || namespace
            .strip_prefix(prefix)
            .is_some_and(|rest| rest.starts_with('/'))
}

/// Whether a CEL rule mentions the pull time: Angos's own substring test
/// (`src/policy/retention_policy.rs:46-50`), string literals included.
pub fn uses_pull_time(rule: &str) -> bool {
    rule.contains("last_pulled_at") || rule.contains("top_pulled")
}

struct Check {
    out: Vec<Violation>,
}

impl Check {
    fn fail(&mut self, rule: Rule, path: impl Into<String>, message: impl Into<String>) {
        self.out.push(Violation {
            rule: rule.id,
            path: path.into(),
            message: message.into(),
        });
    }

    fn size(&mut self, path: &str, size: Option<&ByteSize>, positive: bool) -> Option<u64> {
        let size = size?;
        match size.bytes() {
            None => {
                self.fail(
                    SIZE_FORMAT,
                    path,
                    format!("{:?} is not a size (e.g. \"5MiB\")", size.0),
                );
                None
            }
            Some(0) if positive => {
                self.fail(SIZE_POSITIVE, path, "must be greater than zero");
                Some(0)
            }
            bytes => bytes,
        }
    }

    fn non_zero(&mut self, path: &str, value: Option<u64>) {
        if value == Some(0) {
            self.fail(NON_ZERO, path, "must be greater than zero");
        }
    }

    fn client_tls(&mut self, path: &str, tls: &ClientTls) {
        if tls.client_certificate_bundle.is_some() != tls.client_private_key.is_some() {
            self.fail(
                CLIENT_TLS_PAIR,
                path,
                "client_certificate_bundle and client_private_key are required together for mTLS",
            );
        }
    }

    fn rules(&mut self, path: &str, rules: &[String]) {
        for (i, rule) in rules.iter().enumerate() {
            if rule.trim().is_empty() {
                self.fail(
                    EMPTY_RULE,
                    format!("{path}[{i}]"),
                    "a CEL rule cannot be empty",
                );
            }
        }
    }

    fn regexes(&mut self, path: &str, patterns: &[String]) {
        for (i, pattern) in patterns.iter().enumerate() {
            if let Err(e) = Regex::new(pattern) {
                self.fail(REGEX, format!("{path}[{i}]"), e.to_string());
            }
        }
    }

    fn url(&mut self, rule: Rule, path: &str, url: &str) -> Option<url::Url> {
        match url::Url::parse(url) {
            Ok(url) if url.has_host() => Some(url),
            Ok(_) => {
                self.fail(rule, path, format!("{url:?} has no host"));
                None
            }
            Err(e) => {
                self.fail(rule, path, format!("{url:?} is not an absolute URL: {e}"));
                None
            }
        }
    }

    fn store(&mut self, path: &str, store: &Store) {
        match store {
            Store::Fs(fs) => {
                if fs.root_dir.trim().is_empty() {
                    self.fail(
                        STORE_ROOT_DIR,
                        format!("{path}.fs.root_dir"),
                        "must not be empty",
                    );
                }
            }
            Store::S3(s3) => {
                for (key, value) in [
                    ("endpoint", &s3.endpoint),
                    ("bucket", &s3.bucket),
                    ("region", &s3.region),
                    ("access_key_id", &s3.access_key_id),
                    ("secret_key", &s3.secret_key),
                ] {
                    if value.trim().is_empty() {
                        self.fail(STORE_S3_FIELDS, format!("{path}.s3.{key}"), "is required");
                    }
                }
                let s3_path = format!("{path}.s3");
                for (key, size) in [
                    ("multipart_copy_threshold", &s3.multipart_copy_threshold),
                    ("multipart_copy_chunk_size", &s3.multipart_copy_chunk_size),
                ] {
                    self.size(&format!("{s3_path}.{key}"), size.as_ref(), false);
                }
                let part = format!("{s3_path}.multipart_part_size");
                if let Some(bytes) = self.size(&part, s3.multipart_part_size.as_ref(), false)
                    && bytes < 5 << 20
                {
                    self.fail(PART_SIZE, part, "must be at least 5MiB (S3's minimum part)");
                }
            }
        }
    }

    fn client(&mut self, path: &str, client: &Upstream) {
        if let Some(url) = self.url(UPSTREAM_URL, &format!("{path}.url"), &client.url) {
            let prefix = url.path().trim_matches('/');
            if !prefix.is_empty() && !is_namespace(prefix) {
                self.fail(
                    UPSTREAM_URL,
                    format!("{path}.url"),
                    format!("the path {prefix:?} is not a valid namespace prefix"),
                );
            }
        }
        if client.username.is_some() != client.password.is_some() {
            self.fail(
                BASIC_AUTH_PAIR,
                path,
                "username and password are required together for basic auth",
            );
        }
        self.client_tls(path, &client.tls);
    }

    fn policy<A>(&mut self, path: &str, policy: &Policy<A>, must_be_set: bool) {
        if must_be_set && !policy.is_set() {
            self.fail(POLICY_SET, path, "sets neither default nor rules");
        }
        self.rules(&format!("{path}.rules"), &policy.rules);
    }
}

/// What Angos would refuse in `config`, every rule of [`RULES`] (empty:
/// nothing). CEL rules are only checked not blank here.
pub fn validate(config: &AngosConfig) -> Vec<Violation> {
    let mut check = Check { out: Vec::new() };
    let c = &mut check;

    // Storage.
    match &config.blob_store {
        Some(store) => c.store("blob_store", store),
        None => c.fail(STORE_S3_FIELDS, "blob_store", "is required"),
    }
    if let Some(store) = &config.metadata_store {
        c.store("metadata_store", store);
    }

    // Server.
    if let Some(tls) = &config.server.tls
        && tls.client_auth == Some(ClientAuth::Required)
        && tls.client_ca_bundle.is_none()
    {
        c.fail(
            TLS_CLIENT_AUTH,
            "server.tls.client_auth",
            "client_auth = \"required\" requires client_ca_bundle to be set",
        );
    }
    for (key, value) in [
        ("query_timeout", config.server.query_timeout),
        (
            "query_timeout_grace_period",
            config.server.query_timeout_grace_period,
        ),
        ("handshake_timeout", config.server.handshake_timeout),
    ] {
        c.non_zero(&format!("server.{key}"), value);
    }

    // Global.
    let g = &config.global;
    for (key, size) in [
        ("max_manifest_size", &g.max_manifest_size),
        ("max_blob_size", &g.max_blob_size),
        ("max_index_size", &g.max_index_size),
        ("blob_stream_frame_size", &g.blob_stream_frame_size),
    ] {
        c.size(&format!("global.{key}"), size.as_ref(), true);
    }
    for (key, value) in [
        ("max_concurrent_requests", g.max_concurrent_requests),
        ("max_concurrent_cache_jobs", g.max_concurrent_cache_jobs),
        (
            "max_concurrent_replication_jobs",
            g.max_concurrent_replication_jobs,
        ),
        ("max_concurrent_scan_jobs", g.max_concurrent_scan_jobs),
        ("max_concurrent_index_jobs", g.max_concurrent_index_jobs),
        ("max_index_entries", g.max_index_entries),
        ("namespace_walk_concurrency", g.namespace_walk_concurrency),
        ("listing_read_concurrency", g.listing_read_concurrency),
        ("revision_cache_entries", g.revision_cache_entries),
    ] {
        c.non_zero(&format!("global.{key}"), value);
    }
    if let Some(history) = &g.pull_history {
        c.non_zero("global.pull_history.max_pulls", history.max_pulls);
        c.non_zero(
            "global.pull_history.compact_after_pulls",
            history.compact_after_pulls,
        );
    }
    if let Some(queue) = &g.job_queue {
        if queue.claim_ttl_secs.is_some_and(|v| v < 3) {
            c.fail(
                JOB_QUEUE_FLOORS,
                "global.job_queue.claim_ttl_secs",
                "must be at least 3",
            );
        }
        if queue.pending_refresh_interval_secs.is_some_and(|v| v < 5) {
            c.fail(
                JOB_QUEUE_FLOORS,
                "global.job_queue.pending_refresh_interval_secs",
                "must be at least 5",
            );
        }
    }
    if let Some(policy) = &g.access_policy {
        c.policy("global.access_policy", policy, false);
    }
    if let Some(retention) = &g.retention_policy {
        c.rules("global.retention_policy.rules", &retention.rules);
    }
    if let Some(index) = &g.index {
        c.policy("global.index", index, true);
    }
    if let Some(scan) = &g.scan {
        c.url(WEBHOOK_URL, "global.scan.url", &scan.url);
        c.client_tls("global.scan", &scan.tls);
        c.rules("global.scan.rules", &scan.rules);
    }
    c.regexes(
        "global.immutable_tags_exclusions",
        &g.immutable_tags_exclusions,
    );
    for (i, proxy) in g.trusted_proxies.iter().enumerate() {
        if let Err(e) = trusted_proxy(proxy) {
            c.fail(TRUSTED_PROXY, format!("global.trusted_proxies[{i}]"), e);
        }
    }

    // References to webhooks.
    let auth_webhooks: BTreeSet<&str> = config.auth.webhook.keys().map(String::as_str).collect();
    let event_webhooks: BTreeSet<&str> = config.event_webhook.keys().map(String::as_str).collect();
    if let Some(name) = &g.authorization_webhook
        && !auth_webhooks.contains(name.as_str())
    {
        c.fail(
            WEBHOOK_REF,
            "global.authorization_webhook",
            format!("Webhook '{name}' not found (referenced globally)"),
        );
    }
    for name in &g.event_webhooks {
        if !event_webhooks.contains(name.as_str()) {
            c.fail(
                EVENT_WEBHOOK_REF,
                "global.event_webhooks",
                format!("Event webhook '{name}' not found (referenced globally)"),
            );
        }
    }

    // Repositories.
    let pull_time = g.update_pull_time == Some(true);
    let mut pull_rules: Vec<(String, &[String])> = Vec::new();
    if let Some(scan) = &g.scan {
        pull_rules.push(("global.scan".into(), &scan.rules));
    }
    if let Some(index) = &g.index {
        pull_rules.push(("global.index".into(), &index.rules));
    }
    if let Some(retention) = &g.retention_policy {
        pull_rules.push(("global.retention_policy".into(), &retention.rules));
    }
    let mut namespaces: BTreeMap<String, &str> = BTreeMap::new();
    let prefixes: Vec<&String> = config.repository.keys().collect();
    for (i, a) in prefixes.iter().enumerate() {
        for b in &prefixes[i + 1..] {
            if belongs_to(a, b) || belongs_to(b, a) {
                c.fail(
                    PREFIX_OVERLAP,
                    format!("repository.\"{a}\""),
                    format!("repository prefixes overlap: '{a}' and '{b}'"),
                );
            }
        }
    }
    for (prefix, repo) in &config.repository {
        let path = format!("repository.\"{prefix}\"");
        if !is_namespace(prefix) {
            c.fail(
                REPOSITORY_PREFIX,
                &path,
                format!("repository name '{prefix}' is not a valid namespace"),
            );
        }
        if let Some(namespace) = repo
            .namespace
            .as_deref()
            .map(str::to_lowercase)
            .filter(|n| !n.is_empty())
            && let Some(other) = namespaces.insert(namespace.clone(), prefix)
        {
            c.fail(
                NAMESPACE_UNIQUE,
                format!("{path}.namespace"),
                format!(
                    "repositories '{other}' and '{prefix}' both mirror the namespace '{namespace}'"
                ),
            );
        }
        for (i, upstream) in repo.upstream.iter().enumerate() {
            c.client(&format!("{path}.upstream[{i}]"), upstream);
        }
        let mut names = BTreeSet::new();
        for (i, downstream) in repo.downstream.iter().enumerate() {
            let at = format!("{path}.downstream[{i}]");
            c.client(&at, &downstream.client);
            c.non_zero(
                &format!("{at}.max_concurrent_pushes"),
                downstream.max_concurrent_pushes,
            );
            c.regexes(
                &format!("{at}.namespace_filter"),
                &downstream.namespace_filter,
            );
            if downstream.name.trim().is_empty() {
                c.fail(DOWNSTREAM_NAME, format!("{at}.name"), "must not be empty");
            } else if !names.insert(downstream.name.as_str()) {
                c.fail(
                    DOWNSTREAM_NAME,
                    format!("{at}.name"),
                    format!("duplicate downstream name '{}'", downstream.name),
                );
            }
        }
        if let Some(policy) = &repo.access_policy {
            c.policy(&format!("{path}.access_policy"), policy, false);
        }
        if let Some(retention) = &repo.retention_policy {
            c.rules(&format!("{path}.retention_policy.rules"), &retention.rules);
            pull_rules.push((format!("{path}.retention_policy"), &retention.rules));
        }
        if let Some(scan) = &repo.scan {
            if g.scan.is_none() {
                c.fail(
                    SCAN_NEEDS_GLOBAL,
                    format!("{path}.scan"),
                    format!(
                        "repository '{prefix}' has a scan table but [global.scan] is not configured"
                    ),
                );
            }
            c.policy(&format!("{path}.scan"), scan, true);
            pull_rules.push((format!("{path}.scan"), &scan.rules));
        }
        if let Some(index) = &repo.index {
            c.policy(&format!("{path}.index"), index, true);
            pull_rules.push((format!("{path}.index"), &index.rules));
        }
        c.regexes(
            &format!("{path}.immutable_tags_exclusions"),
            &repo.immutable_tags_exclusions,
        );
        if let Some(name) = repo
            .authorization_webhook
            .as_deref()
            .filter(|n| !n.is_empty())
            && !auth_webhooks.contains(name)
        {
            c.fail(
                WEBHOOK_REF,
                format!("{path}.authorization_webhook"),
                format!("Webhook '{name}' not found (referenced in '{prefix}' repository)"),
            );
        }
        for name in &repo.event_webhooks {
            if !event_webhooks.contains(name.as_str()) {
                c.fail(
                    EVENT_WEBHOOK_REF,
                    format!("{path}.event_webhooks"),
                    format!(
                        "Event webhook '{name}' not found (referenced in '{prefix}' repository)"
                    ),
                );
            }
        }
    }
    if !pull_time {
        for (path, rules) in pull_rules {
            if rules.iter().any(|r| uses_pull_time(r)) {
                c.fail(
                    PULL_TIME,
                    format!("{path}.rules"),
                    "use last_pulled_at or top_pulled but update_pull_time is disabled, so pull \
                     times are never recorded",
                );
            }
        }
    }

    // Event webhooks.
    for (name, hook) in &config.event_webhook {
        let path = format!("event_webhook.{name}");
        c.url(WEBHOOK_URL, &format!("{path}.url"), &hook.url);
        if hook.events.is_empty() {
            c.fail(
                EVENT_WEBHOOK_EVENTS,
                format!("{path}.events"),
                format!("event_webhook.{name} must have at least one event"),
            );
        }
        if hook.token.as_deref().is_some_and(str::is_empty) {
            c.fail(
                EVENT_WEBHOOK_TOKEN,
                format!("{path}.token"),
                "must not be empty",
            );
        }
        if let Some(retries) = hook.max_retries
            && retries > 16
        {
            c.fail(
                EVENT_WEBHOOK_RETRIES,
                format!("{path}.max_retries"),
                format!("max_retries={retries} exceeds the supported maximum of 16"),
            );
        }
        if let Some(filter) = &hook.repository_filter {
            c.regexes(&format!("{path}.repository_filter"), filter);
        }
        c.client_tls(&path, &hook.tls);
    }

    // Authentication.
    let auth = &config.auth;
    let mut usernames: BTreeMap<&str, &str> = BTreeMap::new();
    for (id, identity) in &auth.identity {
        let path = format!("auth.identity.{id}");
        if let Some(other) = usernames.insert(identity.username.as_str(), id) {
            c.fail(
                USERNAME_UNIQUE,
                format!("{path}.username"),
                format!(
                    "duplicate basic-auth username '{}' across [auth.identity] entries ({other} and {id})",
                    identity.username
                ),
            );
        }
        if auth.oidc.contains_key(&identity.username) {
            c.fail(
                USERNAME_NOT_OIDC,
                format!("{path}.username"),
                format!(
                    "basic-auth username '{}' is also an OIDC provider name",
                    identity.username
                ),
            );
        }
        if !identity.password.starts_with("$argon2") {
            c.fail(
                PASSWORD_HASH,
                format!("{path}.password"),
                "is not an Argon2 PHC string",
            );
        }
    }
    for (name, oidc) in &auth.oidc {
        c.client_tls(&format!("auth.oidc.{name}"), &oidc.tls);
    }
    for (name, hook) in &auth.webhook {
        let path = format!("auth.webhook.{name}");
        c.url(WEBHOOK_URL, &format!("{path}.url"), &hook.url);
        c.client_tls(&path, &hook.tls);
        for header in &hook.forward_headers {
            if !HEADER.is_match(header) {
                c.fail(
                    FORWARD_HEADERS,
                    format!("{path}.forward_headers"),
                    format!("invalid forward_headers entry '{header}'"),
                );
            }
        }
    }
    if let Some(token) = &auth.token_service {
        match base64::engine::general_purpose::STANDARD.decode(&token.secret_key) {
            Ok(key) if key.len() >= 32 => {}
            Ok(_) => c.fail(
                TOKEN_KEY,
                "auth.token_service.secret_key",
                "must decode to at least 32 bytes",
            ),
            Err(e) => c.fail(
                TOKEN_KEY,
                "auth.token_service.secret_key",
                format!("invalid base64: {e}"),
            ),
        }
        if let Some(ttl) = token.ttl_secs
            && !(1..=86400).contains(&ttl)
        {
            c.fail(
                TOKEN_TTL,
                "auth.token_service.ttl_secs",
                "must be between 1 and 86400",
            );
        }
        if let Some(realm) = &token.realm
            && let Some(url) = c.url(TOKEN_REALM, "auth.token_service.realm", realm)
        {
            if !matches!(url.scheme(), "http" | "https") {
                c.fail(
                    TOKEN_REALM,
                    "auth.token_service.realm",
                    "must be an http or https URL",
                );
            }
            if !url.path().ends_with("/token") {
                c.fail(
                    TOKEN_REALM,
                    "auth.token_service.realm",
                    "must have a path ending in /token",
                );
            }
        }
    }
    if let Some(oidc) = config.ui.as_ref().and_then(|ui| ui.oidc.as_ref())
        && !auth.oidc.contains_key(&oidc.provider)
    {
        c.fail(
            UI_PROVIDER,
            "ui.oidc.provider",
            format!(
                "ui.oidc.provider '{}' has no matching auth.oidc provider",
                oidc.provider
            ),
        );
    }

    // Cache and observability.
    if let Some(crate::config::Cache::Redis(redis)) = &config.cache {
        if !["redis://", "rediss://", "unix://", "redis+unix://"]
            .iter()
            .any(|scheme| redis.url.starts_with(scheme))
        {
            c.fail(REDIS, "cache.redis.url", "invalid Redis URL");
        }
        if redis.key_prefix.is_empty() {
            c.fail(REDIS, "cache.redis.key_prefix", "is required");
        }
    }
    if let Some(tracing) = config
        .observability
        .as_ref()
        .and_then(|o| o.tracing.as_ref())
    {
        if tracing.endpoint.trim().is_empty() {
            c.fail(
                TRACING_ENDPOINT,
                "observability.tracing.endpoint",
                "is required",
            );
        }
        let rate = tracing.sampling_rate;
        if !rate.is_finite() || !(0.0..=1.0).contains(&rate) {
            c.fail(
                SAMPLING_RATE,
                "observability.tracing.sampling_rate",
                format!("sampling_rate must be a finite number in [0.0, 1.0], got {rate}"),
            );
        }
    }

    check.out
}

/// Angos's `TrustedProxy` (`src/configuration/trusted_proxy.rs:24-49`).
fn trusted_proxy(source: &str) -> Result<(), String> {
    let (address, prefix) = match source.split_once('/') {
        Some((address, prefix)) => (address, Some(prefix)),
        None => (source, None),
    };
    let ip: IpAddr = address
        .parse()
        .map_err(|e| format!("invalid trusted proxy '{source}': {e}"))?;
    let max = if ip.is_ipv4() { 32 } else { 128 };
    if let Some(prefix) = prefix
        && !prefix.parse::<u8>().is_ok_and(|p| p <= max)
    {
        return Err(format!(
            "invalid trusted proxy '{source}': prefix must be 0-{max}"
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use crate::config::{
        AuthWebhook, Cache, Downstream, EventWebhook, FsStore, Identity, JobQueue, Observability,
        Oidc, Redis, Repository, RetentionPolicy, S3Store, Scan, ServerTls, TokenService, Tracing,
        Ui, UiOidc,
    };

    use super::*;

    fn base() -> AngosConfig {
        AngosConfig {
            blob_store: Some(Store::Fs(FsStore {
                root_dir: "/data".into(),
                sync_to_disk: None,
            })),
            ..Default::default()
        }
    }

    /// The rules `config` breaks.
    fn broken(config: &AngosConfig) -> Vec<&'static str> {
        let mut rules: Vec<_> = validate(config).iter().map(|v| v.rule).collect();
        rules.dedup();
        rules
    }

    fn with(edit: impl FnOnce(&mut AngosConfig)) -> Vec<&'static str> {
        let mut config = base();
        edit(&mut config);
        broken(&config)
    }

    fn repository(edit: impl FnOnce(&mut Repository)) -> impl FnOnce(&mut AngosConfig) {
        move |c| {
            let mut repo = Repository::default();
            edit(&mut repo);
            c.repository.insert("team".into(), repo);
        }
    }

    fn webhook() -> EventWebhook {
        EventWebhook {
            url: "https://hook.example".into(),
            policy: crate::config::DeliveryPolicy::Async,
            token: None,
            timeout_ms: None,
            max_retries: None,
            events: vec![crate::config::EventKind::ManifestPush],
            repository_filter: None,
            tls: ClientTls::default(),
        }
    }

    #[test]
    fn every_rule_cites_angos_and_is_listed_once() {
        let mut ids = BTreeSet::new();
        for rule in RULES {
            assert!(ids.insert(rule.id), "{} twice", rule.id);
            assert!(
                !rule.angos.is_empty() && !rule.what.is_empty(),
                "{}",
                rule.id
            );
        }
    }

    #[test]
    fn a_complete_configuration_is_valid() {
        assert_eq!(validate(&crate::config::tests::full()), []);
        assert_eq!(validate(&base()), []);
    }

    #[test]
    fn store_root_dir() {
        assert_eq!(
            with(|c| c.blob_store = Some(Store::Fs(FsStore::default()))),
            [STORE_ROOT_DIR.id]
        );
        assert_eq!(
            with(|c| c.metadata_store = Some(Store::Fs(FsStore::default()))),
            [STORE_ROOT_DIR.id]
        );
    }

    #[test]
    fn store_s3_fields() {
        assert_eq!(with(|c| c.blob_store = None), [STORE_S3_FIELDS.id]);
        assert_eq!(
            with(|c| c.blob_store = Some(Store::S3(Box::default()))),
            [STORE_S3_FIELDS.id]
        );
    }

    #[test]
    fn sizes() {
        assert_eq!(
            with(|c| c.global.max_blob_size = Some(ByteSize("100 gigs".into()))),
            [SIZE_FORMAT.id]
        );
        assert_eq!(
            with(|c| c.global.max_manifest_size = Some(ByteSize("0MiB".into()))),
            [SIZE_POSITIVE.id]
        );
        let part = |size: &str| {
            let size = size.to_string();
            with(move |c| {
                c.blob_store = Some(Store::S3(Box::new(S3Store {
                    access_key_id: "a".into(),
                    secret_key: "s".into(),
                    endpoint: "http://s3".into(),
                    bucket: "b".into(),
                    region: "r".into(),
                    multipart_part_size: Some(ByteSize(size)),
                    ..Default::default()
                })))
            })
        };
        assert_eq!(part("4MiB"), [PART_SIZE.id]);
        assert_eq!(part("5MiB"), Vec::<&str>::new());
    }

    #[test]
    fn non_zero() {
        assert_eq!(
            with(|c| c.global.max_concurrent_index_jobs = Some(0)),
            [NON_ZERO.id]
        );
        assert_eq!(
            with(|c| c.server.handshake_timeout = Some(0)),
            [NON_ZERO.id]
        );
    }

    #[test]
    fn job_queue_floors() {
        assert_eq!(
            with(|c| {
                c.global.job_queue = Some(JobQueue {
                    claim_ttl_secs: Some(2),
                    pending_refresh_interval_secs: Some(4),
                    ..Default::default()
                })
            }),
            [JOB_QUEUE_FLOORS.id]
        );
        assert_eq!(
            validate(&{
                let mut c = base();
                c.global.job_queue = Some(JobQueue {
                    claim_ttl_secs: Some(3),
                    pending_refresh_interval_secs: Some(5),
                    ..Default::default()
                });
                c
            }),
            []
        );
    }

    #[test]
    fn tls_client_auth() {
        assert_eq!(
            with(|c| {
                c.server.tls = Some(ServerTls {
                    server_certificate_bundle: "c".into(),
                    server_private_key: "k".into(),
                    client_ca_bundle: None,
                    client_auth: Some(ClientAuth::Required),
                })
            }),
            [TLS_CLIENT_AUTH.id]
        );
    }

    #[test]
    fn client_tls_pair() {
        assert_eq!(
            with(|c| {
                c.auth.oidc.insert(
                    "p".into(),
                    Oidc {
                        issuer: "https://i".into(),
                        tls: ClientTls {
                            client_private_key: Some("k".into()),
                            ..Default::default()
                        },
                        ..Default::default()
                    },
                );
            }),
            [CLIENT_TLS_PAIR.id]
        );
    }

    #[test]
    fn basic_auth_pair() {
        assert_eq!(
            with(repository(|r| r.upstream = vec![Upstream {
                url: "https://registry-1.docker.io".into(),
                username: Some("u".into()),
                ..Default::default()
            }])),
            [BASIC_AUTH_PAIR.id]
        );
    }

    #[test]
    fn token_service() {
        let token = |key: &str, realm: &str, ttl: u64| {
            let (key, realm) = (key.to_string(), realm.to_string());
            with(move |c| {
                c.auth.token_service = Some(TokenService {
                    secret_key: key,
                    realm: Some(realm),
                    ttl_secs: Some(ttl),
                })
            })
        };
        let key = base64::engine::general_purpose::STANDARD.encode([7u8; 32]);
        let short = base64::engine::general_purpose::STANDARD.encode([7u8; 31]);
        let realm = "https://registry.example/token";
        assert_eq!(token(&key, realm, 3600), Vec::<&str>::new());
        assert_eq!(token(&short, realm, 3600), [TOKEN_KEY.id]);
        assert_eq!(token("not base64!", realm, 3600), [TOKEN_KEY.id]);
        assert_eq!(token(&key, realm, 0), [TOKEN_TTL.id]);
        assert_eq!(token(&key, realm, 86401), [TOKEN_TTL.id]);
        assert_eq!(
            token(&key, "https://registry.example/v2", 1),
            [TOKEN_REALM.id]
        );
        assert_eq!(
            token(&key, "ftp://registry.example/token", 1),
            [TOKEN_REALM.id]
        );
        assert_eq!(token(&key, "registry/token", 1), [TOKEN_REALM.id]);
    }

    #[test]
    fn repository_prefixes() {
        assert_eq!(
            with(|c| {
                c.repository.insert("Team".into(), Repository::default());
            }),
            [REPOSITORY_PREFIX.id]
        );
        assert_eq!(
            with(|c| {
                c.repository.insert("team".into(), Repository::default());
                c.repository
                    .insert("team/app".into(), Repository::default());
            }),
            [PREFIX_OVERLAP.id]
        );
        // A shared start is not an overlap: `team` and `teams`.
        assert_eq!(
            with(|c| {
                c.repository.insert("team".into(), Repository::default());
                c.repository.insert("teams".into(), Repository::default());
            }),
            Vec::<&str>::new()
        );
        assert_eq!(
            with(|c| {
                for prefix in ["a", "b"] {
                    c.repository.insert(
                        prefix.into(),
                        Repository {
                            namespace: Some("Mirror".into()),
                            ..Default::default()
                        },
                    );
                }
            }),
            [NAMESPACE_UNIQUE.id]
        );
    }

    #[test]
    fn upstream_url() {
        let url = |url: &str| {
            let url = url.to_string();
            with(repository(move |r| {
                r.upstream = vec![Upstream {
                    url,
                    ..Default::default()
                }]
            }))
        };
        assert_eq!(url("https://ghcr.io/org"), Vec::<&str>::new());
        assert_eq!(url("ghcr.io"), [UPSTREAM_URL.id]);
        assert_eq!(url("https://ghcr.io/Org"), [UPSTREAM_URL.id]);
    }

    #[test]
    fn downstream_name() {
        let downstream = |name: &str| Downstream {
            name: name.into(),
            client: Upstream {
                url: "https://mirror.example".into(),
                ..Default::default()
            },
            ..Default::default()
        };
        assert_eq!(
            with(repository(|r| r.downstream = vec![downstream("")])),
            [DOWNSTREAM_NAME.id]
        );
        assert_eq!(
            with(repository(
                |r| r.downstream = vec![downstream("dr"), downstream("dr")]
            )),
            [DOWNSTREAM_NAME.id]
        );
    }

    #[test]
    fn usernames() {
        let identity = |username: &str| Identity {
            username: username.into(),
            password: "$argon2id$v=19$m=19456,t=2,p=1$c2FsdA$aGFzaA".into(),
        };
        assert_eq!(
            with(|c| {
                c.auth.identity.insert("a".into(), identity("ci"));
                c.auth.identity.insert("b".into(), identity("ci"));
            }),
            [USERNAME_UNIQUE.id]
        );
        assert_eq!(
            with(|c| {
                c.auth.identity.insert("a".into(), identity("kube"));
                c.auth.oidc.insert(
                    "kube".into(),
                    Oidc {
                        issuer: "https://kubernetes.default.svc".into(),
                        ..Default::default()
                    },
                );
            }),
            [USERNAME_NOT_OIDC.id]
        );
        assert_eq!(
            with(|c| {
                c.auth.identity.insert(
                    "a".into(),
                    Identity {
                        username: "ci".into(),
                        password: "plain".into(),
                    },
                );
            }),
            [PASSWORD_HASH.id]
        );
    }

    #[test]
    fn ui_provider() {
        assert_eq!(
            with(|c| {
                c.ui = Some(Ui {
                    oidc: Some(UiOidc {
                        provider: "missing".into(),
                        client_id: "angos".into(),
                        scopes: None,
                    }),
                    ..Default::default()
                })
            }),
            [UI_PROVIDER.id]
        );
    }

    #[test]
    fn webhook_references() {
        assert_eq!(
            with(|c| c.global.authorization_webhook = Some("opa".into())),
            [WEBHOOK_REF.id]
        );
        // Unlike the repository's, an empty global name is a reference.
        assert_eq!(
            with(|c| c.global.authorization_webhook = Some(String::new())),
            [WEBHOOK_REF.id]
        );
        assert_eq!(
            with(repository(|r| r.authorization_webhook = Some(String::new()))),
            Vec::<&str>::new()
        );
        assert_eq!(
            with(repository(|r| r.authorization_webhook = Some("opa".into()))),
            [WEBHOOK_REF.id]
        );
        assert_eq!(
            with(|c| c.global.event_webhooks = vec!["audit".into()]),
            [EVENT_WEBHOOK_REF.id]
        );
        assert_eq!(
            with(repository(|r| r.event_webhooks = vec!["audit".into()])),
            [EVENT_WEBHOOK_REF.id]
        );
    }

    #[test]
    fn event_webhooks() {
        let hook = |edit: fn(&mut EventWebhook)| {
            with(move |c| {
                let mut hook = webhook();
                edit(&mut hook);
                c.event_webhook.insert("audit".into(), hook);
            })
        };
        assert_eq!(hook(|_| {}), Vec::<&str>::new());
        assert_eq!(hook(|h| h.events.clear()), [EVENT_WEBHOOK_EVENTS.id]);
        assert_eq!(
            hook(|h| h.token = Some(String::new())),
            [EVENT_WEBHOOK_TOKEN.id]
        );
        assert_eq!(
            hook(|h| h.max_retries = Some(17)),
            [EVENT_WEBHOOK_RETRIES.id]
        );
        assert_eq!(hook(|h| h.url = "hook".into()), [WEBHOOK_URL.id]);
        assert_eq!(
            hook(|h| h.repository_filter = Some(vec!["(".into()])),
            [REGEX.id]
        );
    }

    #[test]
    fn forward_headers() {
        assert_eq!(
            with(|c| {
                c.auth.webhook.insert(
                    "opa".into(),
                    AuthWebhook {
                        url: "http://opa".into(),
                        timeout_ms: 100,
                        forward_headers: vec!["X-Ok".into(), "Bad Header".into()],
                        ..Default::default()
                    },
                );
            }),
            [FORWARD_HEADERS.id]
        );
    }

    #[test]
    fn scan_needs_global() {
        assert_eq!(
            with(repository(|r| {
                r.scan = Some(Policy {
                    default: Some(crate::config::ScanAction::Scan),
                    rules: vec![],
                })
            })),
            [SCAN_NEEDS_GLOBAL.id]
        );
    }

    #[test]
    fn policies_must_be_set() {
        assert_eq!(
            with(|c| c.global.index = Some(Policy::default())),
            [POLICY_SET.id]
        );
        assert_eq!(
            with(|c| {
                c.global.scan = Some(Scan {
                    url: "http://scanner".into(),
                    ..Default::default()
                });
                c.repository.insert(
                    "team".into(),
                    Repository {
                        scan: Some(Policy::default()),
                        index: Some(Policy::default()),
                        ..Default::default()
                    },
                );
            }),
            [POLICY_SET.id]
        );
    }

    #[test]
    fn pull_time() {
        let retention = |update: Option<bool>| {
            with(move |c| {
                c.global.update_pull_time = update;
                c.global.retention_policy = Some(RetentionPolicy {
                    // Angos's own test: a substring, in a literal too.
                    rules: vec!["image.tag == 'top_pulled'".into()],
                });
            })
        };
        assert_eq!(retention(None), [PULL_TIME.id]);
        assert_eq!(retention(Some(false)), [PULL_TIME.id]);
        assert_eq!(retention(Some(true)), Vec::<&str>::new());
        assert_eq!(
            with(|c| {
                c.global.index = Some(Policy {
                    default: None,
                    rules: vec!["image.last_pulled_at > now() - days(7)".into()],
                })
            }),
            [PULL_TIME.id]
        );
    }

    #[test]
    fn empty_rules_and_regexes() {
        assert_eq!(
            with(|c| {
                c.global.access_policy = Some(Policy {
                    default: None,
                    rules: vec!["  ".into()],
                })
            }),
            [EMPTY_RULE.id]
        );
        assert_eq!(
            with(|c| c.global.immutable_tags_exclusions = vec!["[".into()]),
            [REGEX.id]
        );
    }

    #[test]
    fn trusted_proxies() {
        for ok in ["10.0.0.1", "10.0.0.0/8", "fd00::/8", "::1"] {
            assert_eq!(trusted_proxy(ok), Ok(()), "{ok}");
        }
        for bad in [
            "10.0.0.0/33",
            "10.0.0.0/",
            "fd00::/129",
            "host",
            "10.0.0.1:80",
        ] {
            assert!(trusted_proxy(bad).is_err(), "{bad}");
        }
        assert_eq!(
            with(|c| c.global.trusted_proxies = vec!["10.0.0.0/40".into()]),
            [TRUSTED_PROXY.id]
        );
    }

    #[test]
    fn redis() {
        let redis = |url: &str, prefix: &str| {
            let (url, prefix) = (url.to_string(), prefix.to_string());
            with(move |c| {
                c.cache = Some(Cache::Redis(Redis {
                    url,
                    key_prefix: prefix,
                }))
            })
        };
        assert_eq!(redis("redis://valkey:6379", "angos:"), Vec::<&str>::new());
        assert_eq!(redis("http://valkey", "angos:"), [REDIS.id]);
        assert_eq!(redis("redis://valkey", ""), [REDIS.id]);
    }

    #[test]
    fn tracing() {
        let tracing = |endpoint: &str, rate: f64| {
            let endpoint = endpoint.to_string();
            with(move |c| {
                c.observability = Some(Observability {
                    tracing: Some(Tracing {
                        endpoint,
                        sampling_rate: rate,
                    }),
                })
            })
        };
        assert_eq!(tracing("http://otel:4317", 1.0), Vec::<&str>::new());
        assert_eq!(tracing("http://otel:4317", 1.5), [SAMPLING_RATE.id]);
        assert_eq!(tracing("http://otel:4317", f64::NAN), [SAMPLING_RATE.id]);
        assert_eq!(tracing("", 0.5), [TRACING_ENDPOINT.id]);
    }

    #[test]
    fn every_rule_has_a_test_breaking_it() {
        // The tests above break each of these; keep this list in step with
        // RULES so a new rule can't go untested.
        let tested = [
            STORE_ROOT_DIR,
            STORE_S3_FIELDS,
            SIZE_FORMAT,
            SIZE_POSITIVE,
            NON_ZERO,
            PART_SIZE,
            JOB_QUEUE_FLOORS,
            TLS_CLIENT_AUTH,
            CLIENT_TLS_PAIR,
            BASIC_AUTH_PAIR,
            TOKEN_KEY,
            TOKEN_REALM,
            TOKEN_TTL,
            REPOSITORY_PREFIX,
            PREFIX_OVERLAP,
            NAMESPACE_UNIQUE,
            UPSTREAM_URL,
            DOWNSTREAM_NAME,
            USERNAME_UNIQUE,
            USERNAME_NOT_OIDC,
            UI_PROVIDER,
            WEBHOOK_REF,
            EVENT_WEBHOOK_REF,
            EVENT_WEBHOOK_EVENTS,
            EVENT_WEBHOOK_TOKEN,
            EVENT_WEBHOOK_RETRIES,
            WEBHOOK_URL,
            FORWARD_HEADERS,
            SCAN_NEEDS_GLOBAL,
            POLICY_SET,
            PULL_TIME,
            EMPTY_RULE,
            REGEX,
            TRUSTED_PROXY,
            REDIS,
            SAMPLING_RATE,
            TRACING_ENDPOINT,
            PASSWORD_HASH,
        ];
        assert_eq!(tested.len(), RULES.len());
        for rule in RULES {
            assert!(tested.contains(rule), "{} has no test", rule.id);
        }
    }
}
