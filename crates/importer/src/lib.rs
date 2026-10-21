//! weebo-si's Angos chart (`2.argo/helm/angos`) as weebo-angos CRs (spec
//! 6.10): its values (`helm get values --all -o yaml <release>`, or the
//! chart's `values.yaml`) read, the objects printed for review.
//!
//! What the chart renders by template rather than by value (the bucket,
//! the region, the Vault paths of its ExternalSecret, the OIDC provider
//! `weebo`, the cache's key prefix) is the chart's at weebo-si `develop`
//! b2fda552, written here as constants.
//!
//! The store is the same one, byte for byte (bucket, endpoint, no key
//! prefix, the repositories' prefixes): no data moves. The token service
//! keeps its key (nobody reconnects). The maintenances come suspended,
//! the prune as a dry run: the cutover reads its report first.

use std::collections::BTreeMap;

use anyhow::{Context, bail};
use api::local_instance::LocalInstance;
use api::maintenance::Maintenance;
use api::repository::Repository;
use serde_json::{Value, json};

/// The chart's bucket (`templates/angos/secret.yaml`).
pub const BUCKET: &str = "angos";
/// The chart's region, truncated as it is (`us-east-`): kept as is, the
/// store being identified by it (spec 2.4), and reported.
pub const REGION: &str = "us-east-";
/// The cache's key prefix in the chart.
pub const CACHE_PREFIX: &str = "angos-cache-";
/// The chart's OIDC provider of the browser and the CLI.
pub const PROVIDER: &str = "weebo";
/// The Vault paths the chart's ExternalSecret reads.
const VAULT: &[(&str, &str, &str)] = &[
    ("access-key-id", "mv/angos/s3", "accesskey"),
    ("secret-key", "mv/angos/s3", "secretkey"),
    ("redis-url", "mv/angos/config", "url"),
    ("token-key", "mv/angos/config", "token_service_key"),
    ("AUTHENTIK_URL", "mv/angos/auth", "AUTHENTIK_URL"),
    (
        "AUTHENTIK_CLIENT_ID",
        "mv/angos/auth",
        "AUTHENTIK_CLIENT_ID",
    ),
];
const VAULT_SCANNER: &[(&str, &str, &str)] = &[
    ("scan-token", "mv/angos/scanner", "token"),
    ("scanner-password", "mv/angos/scanner", "password"),
];

/// What the import is for.
#[derive(Debug, Clone)]
pub struct Options {
    /// The namespace of every object: the release's.
    pub namespace: String,
    /// The instance's name. Not the chart's `angos`: its Service and
    /// Deployment keep that name until the cutover.
    pub name: String,
    /// The Secret the instance reads its credentials from, written by the
    /// ExternalSecret printed with them.
    pub credentials: String,
    /// The SecretStore of that ExternalSecret (the chart's `cred-store`);
    /// `None`: no ExternalSecret, the Secret is made otherwise.
    pub secret_store: Option<String>,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            namespace: "angos".into(),
            name: "registry".into(),
            credentials: "angos-operator".into(),
            secret_store: Some("cred-store".into()),
        }
    }
}

/// The objects to apply, in order, and what to check before.
#[derive(Debug, Clone, Default)]
pub struct Import {
    pub objects: Vec<Value>,
    pub warnings: Vec<String>,
}

fn at<'a>(values: &'a Value, path: &[&str]) -> &'a Value {
    path.iter().fold(values, |v, key| &v[*key])
}

fn string(values: &Value, path: &[&str]) -> Option<String> {
    at(values, path).as_str().map(String::from)
}

fn strings(value: &Value) -> Vec<String> {
    value
        .as_array()
        .map(|items| {
            items
                .iter()
                .filter_map(|i| i.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default()
}

fn flag(values: &Value, path: &[&str], default: bool) -> bool {
    at(values, path).as_bool().unwrap_or(default)
}

fn key(credentials: &str, key: &str) -> Value {
    json!({"name": credentials, "key": key})
}

/// A name for a repository's CR: its prefix, as a DNS-1123 label.
pub fn cr_name(prefix: &str) -> String {
    let mut name: String = prefix
        .to_ascii_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    name = name.trim_matches('-').to_string();
    name.truncate(63);
    name.trim_end_matches('-').to_string()
}

/// The chart's values as weebo-angos CRs.
#[expect(
    clippy::indexing_slicing,
    reason = "serde_json::Value: reading a missing key gives Null, and the keys written are on \
              objects built by json! here"
)]
pub fn import(values: &Value, options: &Options) -> anyhow::Result<Import> {
    let angos = &values["angos"];
    if !angos.is_object() {
        bail!("no `angos` key: not the values of weebo-si's angos chart");
    }
    if !flag(angos, &["enabled"], true) {
        bail!("angos.enabled is false: nothing to import");
    }
    let mut out = Import::default();
    let ns = options.namespace.as_str();
    let creds = options.credentials.as_str();
    let scanner = flag(angos, &["scanner", "enabled"], false);
    let scanner_name = string(angos, &["scanner", "username"]).unwrap_or_else(|| "scanner".into());

    // The credentials, from the chart's Vault paths.
    if let Some(store) = &options.secret_store {
        let data: Vec<Value> = VAULT
            .iter()
            .chain(if scanner { VAULT_SCANNER } else { &[] })
            .map(|(secret_key, key, property)| {
                json!({"secretKey": secret_key, "remoteRef": {"key": key, "property": property}})
            })
            .collect();
        out.objects.push(json!({
            "apiVersion": "external-secrets.io/v1",
            "kind": "ExternalSecret",
            "metadata": {"name": creds, "namespace": ns},
            "spec": {
                "secretStoreRef": {"name": store, "kind": "SecretStore"},
                "target": {
                    "name": creds,
                    // The operator reads only the Secrets that opt in.
                    "template": {"metadata": {"labels": {(api::CREDENTIAL_LABEL): "true"}}},
                },
                "data": data,
            },
        }));
    }

    // The instance.
    let s3_url = string(angos, &["s3Url"]).context("angos.s3Url is unset")?;
    out.warnings.push(format!(
        "storage.s3.region is the chart's {REGION:?} (truncated, spec §2 of the analysis): kept so \
         the store is the same one; correct it to the region the endpoint expects only after the \
         cutover (a storage change, confirmed)"
    ));
    let mut spec = json!({
        "storage": {"s3": {
            "endpoint": s3_url,
            "bucket": BUCKET,
            "region": REGION,
            "credentialsSecretRef": {"name": creds, "accessKeyIdKey": "access-key-id",
                                     "secretKeyKey": "secret-key"},
        }},
        // The chart's Deployment.
        "resources": {"requests": {"cpu": "100m", "memory": "150Mi"},
                      "limits": {"memory": "500Mi"}},
        "cache": {"redis": {"urlSecretRef": key(creds, "redis-url"), "keyPrefix": CACHE_PREFIX}},
        "global": {"access": {
            "catalog": strings(at(angos, &["policy", "browse"])),
            "jobs": strings(at(angos, &["policy", "jobs"])),
            "admin": strings(at(angos, &["policy", "admin"])),
        }},
        "auth": {
            "oidc": [{
                "name": PROVIDER,
                "issuerFrom": key(creds, "AUTHENTIK_URL"),
                "audienceFrom": key(creds, "AUTHENTIK_CLIENT_ID"),
                "requiredClaims": ["email"],
            }],
        },
    });
    let repository = string(angos, &["image", "registry"]);
    let tag = string(angos, &["image", "tag"]);
    let mut image = serde_json::Map::new();
    if let Some(r) = repository.filter(|r| r != api::angos::IMAGE_REPOSITORY) {
        out.warnings.push(format!(
            "image.repository {r}: the operator's managedInstances.allowedImages must list it"
        ));
        image.insert("repository".into(), r.into());
    }
    if let Some(tag) = tag {
        if !api::angos::SUPPORTED_VERSIONS.contains(&tag.as_str()) {
            out.warnings.push(format!(
                "image.tag {tag} isn't a version this operator is tested with ({}): annotate the \
                 instance angos.weebo.io/approved-upgrade={tag}, or move the chart first",
                api::angos::SUPPORTED_VERSIONS.join(", ")
            ));
        }
        image.insert("tag".into(), tag.into());
    }
    if !image.is_empty() {
        spec["image"] = Value::Object(image);
    }
    if let Some(dns) = string(angos, &["dns"]) {
        let mut ts = json!({"realm": format!("https://{dns}/token"),
                            "keySecretRef": key(creds, "token-key")});
        if let Some(ttl) = at(angos, &["tokenService", "ttlSecs"]).as_u64() {
            ts["ttlSecs"] = ttl.into();
        }
        spec["auth"]["tokenService"] = ts;
    } else {
        out.warnings
            .push("angos.dns is unset: no token service (realm unknown)".into());
    }
    if flag(angos, &["kubeOidc", "enabled"], false) {
        let mut kube = json!({
            "name": "kube",
            "audience": string(angos, &["kubeOidc", "audience"]).unwrap_or_else(|| "angos".into()),
        });
        if let Some(issuer) = string(angos, &["kubeOidc", "issuer"]) {
            kube["issuer"] = issuer.into();
        }
        spec["auth"]["kubernetes"] = kube;
        // The chart's ServiceAccount, bound to the issuer discovery in
        // gitops: kept, not created by the operator.
        let account =
            string(angos, &["kubeOidc", "serviceAccountName"]).unwrap_or_else(|| "angos".into());
        spec["serviceAccount"] = json!({"create": false, "name": account});
        out.warnings.push(format!(
            "the ServiceAccount {account} and its ClusterRoleBinding to \
             system:service-account-issuer-discovery stay in gitops (the chart's rbac.yaml)"
        ));
    }
    if flag(angos, &["ui", "enabled"], false) {
        let mut ui = json!({"enabled": true, "name": "Weebo Registry"});
        if flag(angos, &["ui", "nativeAuth"], false) {
            let scopes = string(angos, &["ui", "scope"])
                .unwrap_or_else(|| "openid email profile offline_access groups".into());
            ui["oidc"] = json!({
                "provider": PROVIDER,
                "clientIdFrom": key(creds, "AUTHENTIK_CLIENT_ID"),
                "scopes": scopes.split_whitespace().collect::<Vec<_>>(),
            });
        }
        spec["ui"] = ui;
    }
    if flag(angos, &["caTrust", "enabled"], false) {
        let config_map =
            string(angos, &["caTrust", "bundleConfigMap"]).context("caTrust.bundleConfigMap")?;
        let file = string(angos, &["caTrust", "bundleKey"]).context("caTrust.bundleKey")?;
        let dir =
            string(angos, &["caTrust", "mountPath"]).unwrap_or_else(|| "/etc/ssl/weebo".into());
        spec["extraVolumes"] = json!([{"name": "weebo-ca", "configMap": {
            "name": config_map, "items": [{"key": file, "path": file}]}}]);
        spec["extraVolumeMounts"] =
            json!([{"name": "weebo-ca", "mountPath": dir, "readOnly": true}]);
        // As the chart: the bundle carries the public roots too.
        spec["env"] = json!([{"name": "SSL_CERT_FILE", "value": format!("{dir}/{file}")}]);
    }
    if scanner {
        let port = at(angos, &["scanner", "port"]).as_u64().unwrap_or(8766);
        let mut scan = json!({
            "url": format!("http://angos-scanner:{port}"),
            "tokenSecretRef": key(creds, "scan-token"),
        });
        if let Some(t) = at(angos, &["scanner", "timeoutSecs"]).as_u64() {
            scan["timeoutSecs"] = t.into();
        }
        spec["scan"] = scan;
        if let Some(jobs) = at(angos, &["scanner", "maxConcurrentScanJobs"]).as_u64() {
            spec["global"]["jobs"] = json!({"scan": jobs});
        }
        out.warnings.push(
            "the scanner (StatefulSet angos-scanner) stays in gitops: its registry password is the \
             AngosIdentity's, adopted"
                .into(),
        );
    }
    let instance: LocalInstance = serde_json::from_value(json!({
        "apiVersion": "angos.weebo.io/v1alpha1", "kind": "AngosLocalInstance",
        "metadata": {"name": options.name, "namespace": ns},
        "spec": spec,
    }))
    .context("the instance doesn't read as an AngosLocalInstance")?;
    out.objects.push(serde_json::to_value(&instance)?);

    // The repositories, prefixes byte for byte.
    let repositories = at(angos, &["repositories"])
        .as_object()
        .cloned()
        .unwrap_or_default();
    let mut names = BTreeMap::new();
    for (prefix, repo) in &repositories {
        let name = cr_name(prefix);
        if let Some(other) = names.insert(name.clone(), prefix.clone()) {
            bail!("the repositories {other:?} and {prefix:?} would both be named {name}");
        }
        let access = &repo["access"];
        let mut spec = json!({
            "instanceRef": {"name": options.name},
            "prefix": prefix,
            "access": {
                "pull": strings(&access["pull"]),
                "browse": strings(&access["browse"]),
                "push": strings(&access["push"]),
                "delete": strings(&access["delete"]),
            },
        });
        let upstreams: Vec<Value> = strings(&repo["upstreams"])
            .into_iter()
            .map(|url| json!({"url": url}))
            .collect();
        if !upstreams.is_empty() {
            spec["upstreams"] = Value::Array(upstreams);
        }
        if repo["scan"].as_bool().unwrap_or(false) && scanner {
            spec["scan"] = json!({"default": "Scan"});
            // The chart lets the scanner pull what it scans, nothing else.
            spec["access"]["rules"] = json!([format!(
                "request.action in [\"get-manifest\", \"get-blob\"] && identity.username == \
                 \"{scanner_name}\""
            )]);
        }
        let repository: Repository = serde_json::from_value(json!({
            "apiVersion": "angos.weebo.io/v1alpha1", "kind": "AngosRepository",
            "metadata": {"name": name, "namespace": ns},
            "spec": spec,
        }))
        .with_context(|| format!("the repository {prefix:?} doesn't read"))?;
        out.objects.push(serde_json::to_value(&repository)?);
    }

    // The scanner's identity, its password adopted.
    if scanner {
        out.objects.push(json!({
            "apiVersion": "angos.weebo.io/v1alpha1", "kind": "AngosIdentity",
            "metadata": {"name": cr_name(&scanner_name), "namespace": ns},
            "spec": {
                "instanceRef": {"name": options.name},
                "username": scanner_name,
                "adoptSecretRef": key(creds, "scanner-password"),
            },
        }));
        out.warnings.push(
            "the scanner's password hash was terraform's (2.terra/vault/terra-map/angos.tf): \
             `tofu state rm` it once the AngosIdentity owns the identity"
                .into(),
        );
    }

    // The maintenances, suspended; the prune as a dry run.
    if flag(angos, &["maintenance", "enabled"], false) {
        if string(angos, &["maintenance", "timeZone"]).is_some_and(|z| !z.is_empty()) {
            out.warnings.push(
                "maintenance.timeZone is set: AngosMaintenance schedules are UTC, shift them"
                    .into(),
            );
        }
        for (command, task) in [("scrub", "Scrub"), ("prune", "Prune")] {
            let job = &angos["maintenance"]["jobs"][command];
            if !job["enabled"].as_bool().unwrap_or(false) {
                continue;
            }
            let args = strings(&job["args"]);
            let mut params = json!({"dryRun": task == "Prune" || args.iter().any(|a| a == "--dry-run" || a == "-d")});
            if let Some(n) = args
                .iter()
                .skip_while(|a| *a != "--concurrency")
                .nth(1)
                .and_then(|n| n.parse::<u32>().ok())
            {
                params["concurrency"] = n.into();
            }
            if args.iter().any(|a| a == "--delete-unknown") {
                params["deleteUnknown"] = true.into();
            }
            let mut spec = json!({
                "instanceRef": {"name": options.name},
                "task": task,
                "params": params,
                "suspend": true,
            });
            if let Some(schedule) = job["schedule"].as_str() {
                spec["schedule"] = schedule.into();
            }
            let maintenance: Maintenance = serde_json::from_value(json!({
                "apiVersion": "angos.weebo.io/v1alpha1", "kind": "AngosMaintenance",
                "metadata": {"name": format!("{}-{command}", options.name), "namespace": ns},
                "spec": spec,
            }))
            .with_context(|| format!("the maintenance {command} doesn't read"))?;
            out.objects.push(serde_json::to_value(&maintenance)?);
        }
        out.warnings.push(
            "the maintenances come suspended: suspend the chart's CronJobs (angos-prune first), \
             read the prune's dry run (zero deletion) before resuming them"
                .into(),
        );
    }
    Ok(out)
}

/// The import as one YAML stream, the warnings as a leading comment.
pub fn to_yaml(import: &Import) -> anyhow::Result<String> {
    let mut text = String::new();
    if !import.warnings.is_empty() {
        text.push_str("# Before applying:\n");
        for warning in &import.warnings {
            text.push_str(&format!("# - {warning}\n"));
        }
    }
    for object in &import.objects {
        text.push_str("---\n");
        text.push_str(&serde_norway::to_string(object)?);
    }
    Ok(text)
}
