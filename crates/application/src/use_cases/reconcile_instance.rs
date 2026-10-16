//! An `AngosLocalInstance`: its workload and both configuration layers
//! (spec 2.3, 2.4, 6.1, 6.2). One pass:
//!
//! 1. the Secrets the spec references are read (credential label
//!    required), the token-service key generated once;
//! 2. the spec is rendered and checked (`InvalidSpec`: nothing written);
//! 3. the startup layer: a new one that moves the storage waits for
//!    `angos.weebo.io/confirm-storage` (`StorageChangePending`) — the pods
//!    keep the current one;
//! 4. the hot layer: the desired one, or — after a refusal, until something
//!    changes or 10 minutes passed — the last one every pod accepted (D12);
//!    a new content written is recorded at once (`configWrittenTime`);
//! 5. the objects are applied (none taken over), stale startup `Secret`s
//!    removed;
//! 6. the rollout: pods not ready on the startup layer are
//!    `WorkloadNotReady`, crashing ones `BootRejected` (their last error);
//! 7. the hot layer's confirmation, pod by pod, from their logs: applied
//!    (`ConfigReloaded`, copied to `<name>-config-applied`), refused
//!    (`ConfigRejected`, the accepted one put back) or pending
//!    (`ConfigPropagating`; past `reload.timeoutSeconds`, `Wait` or roll).

use std::collections::BTreeMap;

use api::annotations;
use api::config::{OnUnconfirmed, ReloadMode};
use api::local_instance::{LocalInstanceSpec, Names};
use api::status::{InstanceStatus, PodReload};
use domain::error::ReasonCode;
use domain::{Advisory, Failure};
use k8s_openapi::jiff::{SignedDuration, Timestamp};
use weebo_kit_api::annotations as weebo_kit_api_annotations;
use weebo_kit_application::workload::{LogQuery, Owner};

use super::{Outcome, workload_failure};
use crate::ports::{
    ClusterIssuer, CredentialError, Credentials, KeyGenerator, OwnedObjects, PodLogs, StatusLedger,
};
use crate::reload::{self, Confirmation, LOG_SLACK};
use crate::render::{self, Rendered, Resolved, paths};
use crate::workload::{self, WorkloadInput, boot_secret, selector};

/// A refused hot layer is tried again after this long even if nothing
/// changed (a JWKS unreachable for a while…).
pub const RETRY_REJECTED: SignedDuration = SignedDuration::from_mins(10);

/// How many startup `Secret`s are kept besides the running one (a
/// `kubectl rollout undo`).
pub const KEEP_BOOT_SECRETS: usize = 2;

/// The key of the generated token-service key, and of the rotation it was
/// made for.
pub const TOKEN_KEY: &str = "key";
pub const TOKEN_ROTATION: &str = "rotation";

/// What a pass is about.
#[derive(Debug, Clone)]
pub struct InstancePass<'a> {
    pub owner: &'a Owner,
    pub spec: &'a LocalInstanceSpec,
    pub annotations: &'a BTreeMap<String, String>,
    /// The status as read (the previous pass's).
    pub status: &'a InstanceStatus,
    pub now: Timestamp,
    /// `repository:tag` of an instance without `image`.
    pub default_image: &'a str,
    /// At most this many bytes of a pod's log are read.
    pub log_limit: i64,
}

/// What a pass needs.
pub struct InstancePorts<'a> {
    pub objects: &'a dyn OwnedObjects,
    pub logs: &'a dyn PodLogs,
    pub credentials: &'a dyn Credentials,
    pub issuer: &'a dyn ClusterIssuer,
    pub keys: &'a dyn KeyGenerator,
    pub ledger: &'a dyn StatusLedger,
    pub repositories: &'a dyn crate::ports::RepositoryDirectory,
    pub identities: &'a dyn crate::ports::IdentityDirectory,
    pub cel: &'a dyn crate::ports::CelChecker,
}

/// A pass's result: the `Ready` outcome, the status to publish, the Events
/// to announce.
#[derive(Debug, Clone)]
pub struct InstanceReport {
    pub outcome: Outcome,
    pub status: InstanceStatus,
    pub events: Vec<(ReasonCode, String)>,
}

fn time(text: &Option<String>) -> Option<Timestamp> {
    text.as_deref()?.parse().ok()
}

fn advisory(reason: ReasonCode, message: impl Into<String>) -> Advisory {
    Advisory {
        reason,
        message: message.into(),
    }
}

fn credential_failure(e: CredentialError) -> Failure {
    Failure::new(e.reason(), e.to_string())
}

/// The status fields a pass writes, for a merge patch (`null` clears).
pub fn status_fields(status: &InstanceStatus) -> serde_json::Map<String, serde_json::Value> {
    let mut value = serde_json::to_value(status).expect("a status serializes");
    let map = value.as_object_mut().expect("a status is an object");
    map.remove("conditions");
    map.remove("observedGeneration");
    for key in [
        "endpoint",
        "version",
        "availableReplicas",
        "bootHash",
        "pendingBootHash",
        "configHash",
        "appliedConfigHash",
        "lastAppliedTime",
        "configWrittenTime",
        "writtenConfigHash",
        "rejectedConfigHash",
        "rejectionMessage",
        "lastRejectionTime",
        "rolledOutFor",
        "repositories",
        "identities",
        "releasedRepositories",
    ] {
        map.entry(key).or_insert(serde_json::Value::Null);
    }
    map.entry("pods").or_insert(serde_json::json!([]));
    map.entry("bootHistory").or_insert(serde_json::json!([]));
    map.clone()
}

/// The values the spec's Secrets hold.
async fn resolve(
    pass: &InstancePass<'_>,
    ports: &InstancePorts<'_>,
    events: &mut Vec<(ReasonCode, String)>,
) -> Result<Resolved, Failure> {
    let spec = pass.spec;
    let ns = pass.owner.namespace.as_str();
    let read = |name: String, key: String| async move {
        ports
            .credentials
            .read(ns, &name, &key)
            .await
            .map(|v| v.trim().to_string())
            .map_err(credential_failure)
    };
    let mut resolved = Resolved::default();
    if let Some(s3) = &spec.storage.s3 {
        let c = &s3.credentials_secret_ref;
        resolved.s3 = Some((
            read(c.name.clone(), c.access_key_id_key.clone()).await?,
            read(c.name.clone(), c.secret_key_key.clone()).await?,
        ));
    }
    if let Some(s3) = spec.storage.metadata.as_ref().and_then(|m| m.s3.as_ref()) {
        let c = &s3.credentials_secret_ref;
        resolved.metadata_s3 = Some((
            read(c.name.clone(), c.access_key_id_key.clone()).await?,
            read(c.name.clone(), c.secret_key_key.clone()).await?,
        ));
    }
    if let Some(cache) = &spec.cache {
        let r = &cache.redis.url_secret_ref;
        resolved.redis_url = Some(read(r.name.clone(), r.key.clone()).await?);
    }
    for provider in &spec.auth.oidc {
        let issuer = match (&provider.issuer, &provider.issuer_from) {
            (Some(issuer), _) => issuer.clone(),
            (None, Some(r)) => read(r.name.clone(), r.key.clone()).await?,
            (None, None) => String::new(),
        };
        let audience = match (&provider.audience, &provider.audience_from) {
            (Some(audience), _) => audience.clone(),
            (None, Some(r)) => read(r.name.clone(), r.key.clone()).await?,
            (None, None) => String::new(),
        };
        resolved
            .oidc
            .insert(provider.name.clone(), (issuer, audience));
    }
    if let Some(r) = spec
        .ui
        .as_ref()
        .and_then(|ui| ui.oidc.as_ref())
        .and_then(|oidc| oidc.client_id_from.as_ref())
    {
        resolved.ui_client_id = Some(read(r.name.clone(), r.key.clone()).await?);
    }
    if spec
        .auth
        .kubernetes
        .as_ref()
        .is_some_and(|k| k.issuer.is_none())
    {
        resolved.cluster_issuer = Some(ports.issuer.issuer().await.map_err(|e| {
            Failure::new(
                ReasonCode::KubernetesApiError,
                format!(
                    "the cluster's ServiceAccount issuer couldn't be read ({e}): set \
                     spec.auth.kubernetes.issuer"
                ),
            )
        })?);
    }
    if let Some(ts) = &spec.auth.token_service {
        resolved.token_key = Some(match &ts.key_secret_ref {
            Some(r) => read(r.name.clone(), r.key.clone()).await?,
            None => token_key(pass, ports, events).await?,
        });
    }
    Ok(resolved)
}

/// The generated token-service key: made once in `<name>-token-key`, made
/// again when `angos.weebo.io/rotate` changes.
async fn token_key(
    pass: &InstancePass<'_>,
    ports: &InstancePorts<'_>,
    events: &mut Vec<(ReasonCode, String)>,
) -> Result<String, Failure> {
    let name = Names(&pass.owner.name).token_key();
    let rotation = pass
        .annotations
        .get(annotations::ROTATE)
        .cloned()
        .unwrap_or_default();
    let read = |key: &'static str| {
        let name = name.clone();
        async move {
            ports
                .objects
                .secret_text(pass.owner, &name, key)
                .await
                .map_err(workload_failure)
        }
    };
    let existing = read(TOKEN_KEY).await?;
    if let Some(key) = &existing
        && read(TOKEN_ROTATION).await?.unwrap_or_default() == rotation
    {
        return Ok(key.clone());
    }
    let key = ports.keys.token_key();
    let mut labels = workload::labels(&pass.owner.name, "token-key");
    labels.insert(api::CREDENTIAL_LABEL.into(), "true".into());
    let secret = k8s_openapi::api::core::v1::Secret {
        metadata: k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta {
            name: Some(name.clone()),
            namespace: Some(pass.owner.namespace.clone()),
            labels: Some(labels),
            ..Default::default()
        },
        data: Some(BTreeMap::from([
            (
                TOKEN_KEY.to_string(),
                k8s_openapi::ByteString(key.clone().into_bytes()),
            ),
            (
                TOKEN_ROTATION.to_string(),
                k8s_openapi::ByteString(rotation.into_bytes()),
            ),
        ])),
        ..Default::default()
    };
    ports
        .objects
        .apply(
            pass.owner,
            &[weebo_kit_application::workload::OwnedObject::of(
                &secret, false,
            )],
        )
        .await
        .map_err(workload_failure)?;
    if existing.is_some() {
        events.push((
            ReasonCode::TokenKeyRotated,
            "the token service's key was rotated: every token issued before is invalid".into(),
        ));
    }
    Ok(key)
}

/// `global.access`, `global.retention`, `global.index` checked, and the
/// repositories bound to the instance rendered (or recorded as not).
async fn policies(
    pass: &InstancePass<'_>,
    ports: &InstancePorts<'_>,
    status: &mut InstanceStatus,
    resolved: &mut Resolved,
    advisories: &mut Vec<Advisory>,
) -> Result<(), Failure> {
    use crate::rules;
    let spec = pass.spec;
    let kube = spec.auth.kubernetes.as_ref().map(|k| k.name.as_str());
    let invalid =
        |problems: Vec<String>| Failure::new(ReasonCode::InvalidSpec, problems.join("; "));
    let mut warnings = Vec::new();
    let global = rules::global_access(&spec.global.access, kube, ports.cel).map_err(invalid)?;
    warnings.extend(global.warnings);
    resolved.global_rules = global.rules;
    if let Some(retention) = &spec.global.retention {
        let checked =
            rules::image_rules("spec.global.retention.rules", &retention.rules, ports.cel)
                .map_err(invalid)?;
        warnings.extend(checked.warnings);
        resolved.retention = Some(domain::config::RetentionPolicy {
            rules: checked.rules,
        });
    }
    if let Some(index) = &spec.global.index {
        let (policy, more) =
            rules::index_policy("spec.global.index", index, ports.cel).map_err(invalid)?;
        warnings.extend(more);
        resolved.index = Some(policy);
    }
    let views = ports
        .repositories
        .bound_to(pass.owner, &spec.repository_selector)
        .await
        .map_err(|e| Failure::new(ReasonCode::KubernetesApiError, e))?;
    let in_service: std::collections::BTreeSet<String> = status
        .repositories
        .iter()
        .filter(|(_, r)| r.state == api::status::RenderState::Rendered)
        .map(|(key, _)| key.clone())
        .collect();
    let previous = std::mem::take(&mut status.repositories);
    let views: Vec<_> = views.into_iter().filter(|v| !v.released()).collect();
    let lost = crate::repositories::conflicts(&views, &in_service);
    for view in &views {
        let key = view.key();
        let mut render = api::status::RepositoryRender {
            prefix: view.spec.prefix.clone(),
            state: api::status::RenderState::Rendered,
            message: None,
            conflict_with: None,
            generation: view.generation,
            applied_generation: previous.get(&key).and_then(|r| r.applied_generation),
        };
        if let Some(winner) = lost.get(&key) {
            render.state = api::status::RenderState::Conflict;
            render.conflict_with = Some(winner.clone());
            render.message = Some(format!(
                "its prefix {:?} overlaps the one of the older repository {winner}",
                view.spec.prefix
            ));
        } else {
            match crate::repositories::render(view, kube, ports.cel, ports.credentials).await {
                Ok((table, more)) => {
                    warnings.extend(more.into_iter().map(|w| format!("{key}: {w}")));
                    resolved
                        .repositories
                        .insert(view.spec.prefix.clone(), table);
                }
                Err((state, message)) => {
                    render.state = state;
                    render.message = Some(message);
                }
            }
        }
        status.repositories.insert(key, render);
    }
    // What left the configuration: a prune waits after it (spec 6.6).
    for (key, render) in &previous {
        let still = status
            .repositories
            .get(key)
            .is_some_and(|r| r.state == api::status::RenderState::Rendered);
        if render.state == api::status::RenderState::Rendered && !still {
            status
                .released_repositories
                .entry(key.clone())
                .or_insert_with(|| pass.now.to_string());
        }
    }
    let keep = SignedDuration::from_hours(2 * i64::from(spec.prune_lock.window_hours.max(24)));
    status.released_repositories.retain(|key, at| {
        !status.repositories.contains_key(key)
            && at
                .parse::<Timestamp>()
                .is_ok_and(|at| pass.now.duration_since(at) < keep)
    });
    if !warnings.is_empty() {
        advisories.push(advisory(ReasonCode::RuleMayFail, warnings.join("; ")));
    }
    identities(pass, ports, status, resolved).await
}

/// The identities bound to the instance rendered, or recorded as not.
async fn identities(
    pass: &InstancePass<'_>,
    ports: &InstancePorts<'_>,
    status: &mut InstanceStatus,
    resolved: &mut Resolved,
) -> Result<(), Failure> {
    use api::status::{IdentityRender, RenderState};
    let spec = pass.spec;
    let views = ports
        .identities
        .bound_to(pass.owner, &spec.repository_selector)
        .await
        .map_err(|e| Failure::new(ReasonCode::KubernetesApiError, e))?;
    let in_service: std::collections::BTreeSet<String> = status
        .identities
        .iter()
        .filter(|(_, r)| r.state == RenderState::Rendered)
        .map(|(key, _)| key.clone())
        .collect();
    let providers: std::collections::BTreeSet<String> = spec
        .auth
        .oidc
        .iter()
        .map(|p| p.name.clone())
        .chain(spec.auth.kubernetes.iter().map(|k| k.name.clone()))
        .collect();
    let lost = crate::identities::conflicts(&views, &in_service, &providers);
    let previous = std::mem::take(&mut status.identities);
    for view in &views {
        let key = view.key();
        let render = if let Some((winner, why)) = lost.get(&key) {
            crate::identities::refused(view, RenderState::Conflict, why.clone(), winner.clone())
        } else {
            match crate::identities::hash_of(view, ports.credentials).await {
                Ok((hash, version)) => {
                    resolved.identities.insert(view.username.clone(), hash);
                    IdentityRender {
                        username: view.username.clone(),
                        state: RenderState::Rendered,
                        version: Some(version),
                        applied_version: previous.get(&key).and_then(|r| r.applied_version),
                        message: None,
                        conflict_with: None,
                    }
                }
                Err(CredentialError::NotAllowed(m)) => {
                    crate::identities::refused(view, RenderState::SecretNotAllowed, m, None)
                }
                Err(CredentialError::Kubernetes(m)) => {
                    return Err(Failure::new(ReasonCode::KubernetesApiError, m));
                }
                Err(CredentialError::Missing(m)) => {
                    crate::identities::refused(view, RenderState::SecretMissing, m, None)
                }
            }
        };
        status.identities.insert(key, render);
    }
    Ok(())
}

/// The startup layer the pods are to run: the rendered one, unless it
/// moves the storage without the confirmation.
struct Boot {
    file: String,
    hash: String,
    pending: Option<String>,
}

async fn choose_boot(
    pass: &InstancePass<'_>,
    ports: &InstancePorts<'_>,
    rendered: &Rendered,
    advisories: &mut Vec<Advisory>,
) -> Result<Boot, Failure> {
    let desired = Boot {
        file: rendered.boot_file.clone(),
        hash: rendered.boot_hash.clone(),
        pending: None,
    };
    let Some(current_hash) = pass.status.boot_hash.clone() else {
        // The instance's first startup layer: nothing to move.
        return Ok(desired);
    };
    if current_hash == rendered.boot_hash {
        return Ok(desired);
    }
    let current_file = ports
        .objects
        .secret_text(
            pass.owner,
            &boot_secret(&pass.owner.name, &current_hash),
            paths::FILE,
        )
        .await
        .map_err(workload_failure)?;
    let identity = |file: &str| {
        toml::from_str::<toml::Table>(file)
            .ok()
            .map(|t| domain::config::storage_identity(&t))
    };
    let new_identity = identity(&rendered.boot_file);
    let old_identity = current_file.as_deref().and_then(identity);
    if old_identity.is_some() && old_identity == new_identity {
        // Same store: only its credentials, or another startup key, moved.
        if store_differs(current_file.as_deref(), &rendered.boot_file) {
            advisories.push(advisory(
                ReasonCode::StorageCredentialsRotated,
                "the store's credentials changed: the pods roll onto them",
            ));
        }
        return Ok(desired);
    }
    let confirmed = pass.annotations.get(annotations::CONFIRM_STORAGE) == Some(&rendered.boot_hash);
    if confirmed {
        return Ok(desired);
    }
    let Some(current_file) = current_file else {
        // The running layer is gone: nothing to compare, nothing to keep.
        return Ok(desired);
    };
    advisories.push(advisory(
        ReasonCode::StorageChangePending,
        format!(
            "the storage changes ({} → {}): no data moves, the instance would serve what the new \
             store holds. To apply it: kubectl annotate angoslocalinstance {} {}={} --overwrite",
            describe_store(&current_file),
            describe_store(&rendered.boot_file),
            pass.owner.name,
            annotations::CONFIRM_STORAGE,
            rendered.boot_hash
        ),
    ));
    Ok(Boot {
        file: current_file,
        hash: current_hash,
        pending: Some(rendered.boot_hash.clone()),
    })
}

fn store_differs(current: Option<&str>, desired: &str) -> bool {
    let stores = |file: &str| {
        toml::from_str::<toml::Table>(file).ok().map(|t| {
            (
                t.get("blob_store").cloned(),
                t.get("metadata_store").cloned(),
            )
        })
    };
    current.and_then(stores) != stores(desired)
}

/// The store of a startup file, in words: never its credentials.
fn describe_store(file: &str) -> String {
    let Ok(table) = toml::from_str::<toml::Table>(file) else {
        return "unknown".into();
    };
    let describe = |store: Option<&toml::Value>| -> String {
        let Some(store) = store else {
            return "the blob store".into();
        };
        if let Some(s3) = store.get("s3") {
            let get = |k: &str| s3.get(k).and_then(toml::Value::as_str).unwrap_or("");
            format!(
                "s3 {}/{}{}",
                get("endpoint"),
                get("bucket"),
                match get("key_prefix") {
                    "" => String::new(),
                    prefix => format!(" prefix {prefix}"),
                }
            )
        } else if let Some(fs) = store.get("fs") {
            format!(
                "fs {}",
                fs.get("root_dir")
                    .and_then(toml::Value::as_str)
                    .unwrap_or("")
            )
        } else {
            "unknown".into()
        }
    };
    let blobs = describe(table.get("blob_store"));
    match table.get("metadata_store") {
        Some(meta) => format!("{blobs}, metadata {}", describe(Some(meta))),
        None => blobs,
    }
}

/// Records the startup layer about to run in `status.bootHistory`, and
/// removes the `Secret`s of those beyond the [`KEEP_BOOT_SECRETS`] kept
/// after the running one (by name: the operator never lists Secrets).
async fn prune_boot_secrets(
    pass: &InstancePass<'_>,
    ports: &InstancePorts<'_>,
    status: &mut InstanceStatus,
    boot_hash: &str,
) -> Result<(), Failure> {
    let mut history: Vec<String> = vec![boot_hash.to_string()];
    for hash in &status.boot_history {
        if hash != boot_hash {
            history.push(hash.clone());
        }
    }
    let keep = KEEP_BOOT_SECRETS + 1;
    for hash in history.iter().skip(keep) {
        ports
            .objects
            .remove(
                pass.owner,
                "v1",
                "Secret",
                &boot_secret(&pass.owner.name, hash),
            )
            .await
            .map_err(workload_failure)?;
    }
    history.truncate(keep);
    status.boot_history = history;
    Ok(())
}

/// The pass.
pub async fn reconcile_instance(
    pass: InstancePass<'_>,
    ports: &InstancePorts<'_>,
) -> InstanceReport {
    let mut status = pass.status.clone();
    let mut events = Vec::new();
    let mut advisories = Vec::new();
    let ready = run(&pass, ports, &mut status, &mut events, &mut advisories).await;
    let mut outcome = match ready {
        Ok(message) => Outcome::synced(message),
        Err(failure) => Outcome::failed(failure),
    };
    outcome.advisories = advisories;
    InstanceReport {
        outcome,
        status,
        events,
    }
}

async fn run(
    pass: &InstancePass<'_>,
    ports: &InstancePorts<'_>,
    status: &mut InstanceStatus,
    events: &mut Vec<(ReasonCode, String)>,
    advisories: &mut Vec<Advisory>,
) -> Result<String, Failure> {
    let spec = pass.spec;
    let owner = pass.owner;
    let names = Names(&owner.name);
    status.endpoint = Some(format!(
        "{}.{}.svc:{}",
        names.workload(),
        owner.namespace,
        spec.service.port
    ));

    // 1–2. The Secrets, the rules and the repositories, then the files.
    let mut resolved = resolve(pass, ports, events).await?;
    policies(pass, ports, status, &mut resolved, advisories).await?;
    let rendered = render::render(spec, &resolved).map_err(|e| {
        Failure::new(
            ReasonCode::InvalidSpec,
            format!("the configuration Angos would refuse: {e}"),
        )
    })?;
    if rendered.extra_config_used {
        advisories.push(advisory(
            ReasonCode::ExtraConfigUsed,
            "spec.extraConfig is rendered: what it sets is checked by Angos only",
        ));
    }
    if spec.auth.kubernetes.is_some() {
        advisories.push(advisory(
            ReasonCode::IssuerDiscoveryNotBound,
            format!(
                "auth.kubernetes reads the cluster's JWKS with the account {}: bind it to the \
                 ClusterRole system:service-account-issuer-discovery (not done by the operator)",
                spec.service_account
                    .name
                    .clone()
                    .unwrap_or_else(|| owner.name.clone())
            ),
        ));
    }

    // 3. The startup layer.
    let boot = choose_boot(pass, ports, &rendered, advisories).await?;
    status.pending_boot_hash = boot.pending.clone();

    // 4. The hot layer.
    let desired_hash = rendered.hot_hash.clone();
    status.config_hash = Some(desired_hash.clone());
    let applied_file = ports
        .objects
        .secret_text(owner, &names.config_applied(), paths::FILE)
        .await
        .map_err(workload_failure)?;
    let recently_rejected = status.rejected_config_hash.as_deref() == Some(&desired_hash)
        && time(&status.last_rejection_time)
            .is_some_and(|at| pass.now.duration_since(at) < RETRY_REJECTED);
    let held = recently_rejected && applied_file.is_some();
    let target = if held {
        applied_file
            .clone()
            .expect("held only with an applied file")
    } else {
        rendered.hot_file.clone()
    };
    let target_hash = render::hot_hash_of(&target);
    let current = ports
        .objects
        .secret_text(owner, &names.config(), paths::FILE)
        .await
        .map_err(workload_failure)?;
    let writes = current.as_deref() != Some(target.as_str());

    // 5. The objects — what rolls the pods held back when they'd be
    // recreated without consent (spec 6.2).
    let mut image = workload::image(spec, pass.default_image);
    let mut boot = boot;
    let running = ports
        .objects
        .rollout(owner, &names.workload())
        .await
        .map_err(workload_failure)?;
    let consent = pass
        .annotations
        .get(&api::ANNOTATIONS.key(weebo_kit_api_annotations::ALLOW_DISRUPTIVE_UPDATE))
        .is_some_and(|v| v == "true");
    if let Some(running) = running
        .as_ref()
        .filter(|_| workload::single_writer(spec) && !consent)
    {
        let running_boot = running.annotation(annotations::BOOT_HASH).map(String::from);
        let running_image = running
            .annotation(workload::IMAGE_ANNOTATION)
            .map(String::from);
        let rolls = running_boot.as_deref() != Some(boot.hash.as_str())
            || running_image.as_deref() != Some(image.as_str());
        if rolls
            && let (Some(running_boot), Some(running_image)) = (running_boot, running_image)
            && let Some(file) = ports
                .objects
                .secret_text(owner, &boot_secret(&owner.name, &running_boot), paths::FILE)
                .await
                .map_err(workload_failure)?
        {
            advisories.push(advisory(
                ReasonCode::DisruptiveUpdatePending,
                format!(
                    "the pods share a single-writer volume: rolling them recreates them (an \
                     outage). The change waits for: kubectl annotate angoslocalinstance {} {}=true",
                    owner.name,
                    api::ANNOTATIONS.key(weebo_kit_api_annotations::ALLOW_DISRUPTIVE_UPDATE)
                ),
            ));
            boot = Boot {
                file,
                hash: running_boot,
                pending: boot.pending,
            };
            image = running_image;
        }
    }
    let input = WorkloadInput {
        name: &owner.name,
        namespace: &owner.namespace,
        spec,
        image: &image,
        boot_file: &boot.file,
        boot_hash: &boot.hash,
        hot_file: &target,
        rollout_hash: (spec.reload.mode == ReloadMode::Rollout).then_some(target_hash.as_str()),
    };
    ports
        .objects
        .apply(owner, &workload::objects(&input))
        .await
        .map_err(workload_failure)?;
    if writes {
        status.config_written_time = Some(pass.now.to_string());
        status.written_config_hash = Some(target_hash.clone());
        status.pods.clear();
        let mut fields = serde_json::Map::new();
        for key in ["configWrittenTime", "writtenConfigHash", "configHash"] {
            fields.insert(
                key.into(),
                status_fields(status).remove(key).unwrap_or_default(),
            );
        }
        fields.insert("pods".into(), serde_json::json!([]));
        ports
            .ledger
            .record(&owner.namespace, &owner.name, fields)
            .await
            .map_err(|e| Failure::new(ReasonCode::KubernetesApiError, e))?;
    }
    for (api_version, kind, name) in workload::removed(&input) {
        ports
            .objects
            .remove(owner, api_version, kind, &name)
            .await
            .map_err(workload_failure)?;
    }
    prune_boot_secrets(pass, ports, status, &boot.hash).await?;

    // 6. The rollout.
    let rollout = ports
        .objects
        .rollout(owner, &names.workload())
        .await
        .map_err(workload_failure)?;
    let pods = ports
        .objects
        .pods(owner, &selector(&owner.name))
        .await
        .map_err(workload_failure)?;
    let on_boot: Vec<_> = pods
        .iter()
        .filter(|p| p.annotations.get(annotations::BOOT_HASH) == Some(&boot.hash))
        .collect();
    status.available_replicas = rollout.as_ref().map(|r| r.available);
    let rolled = rollout.as_ref().is_some_and(|r| {
        r.complete()
            && r.annotation(annotations::BOOT_HASH) == Some(boot.hash.as_str())
            && r.annotation(workload::IMAGE_ANNOTATION) == Some(image.as_str())
    });
    if !rolled {
        if let Some(crashing) = on_boot.iter().find(|p| {
            p.restarts > 0
                && p.waiting
                    .as_deref()
                    .is_some_and(|w| w == "CrashLoopBackOff" || w == "Error")
        }) {
            let logged = ports
                .logs
                .read(
                    &owner.namespace,
                    &crashing.name,
                    &LogQuery {
                        container: Some("angos".into()),
                        since: None,
                        limit_bytes: Some(pass.log_limit),
                    },
                )
                .await
                .ok()
                .and_then(|lines| reload::last_error(&lines));
            return Err(Failure::new(
                ReasonCode::BootRejected,
                format!(
                    "pod {} doesn't start on the startup layer {}: {} (the previous pods keep \
                     serving; fix the spec)",
                    crashing.name,
                    &boot.hash[..8],
                    logged
                        .or_else(|| crashing.last_error.clone())
                        .unwrap_or_else(|| "it keeps crashing".into())
                ),
            ));
        }
        let ready = on_boot.iter().filter(|p| p.ready).count();
        return Err(Failure::new(
            ReasonCode::WorkloadNotReady,
            format!(
                "{ready}/{} pods ready on the startup layer {} ({image})",
                spec.replicas,
                &boot.hash[..8]
            ),
        ));
    }
    if pass
        .status
        .boot_hash
        .as_deref()
        .is_some_and(|previous| previous != boot.hash)
    {
        events.push((
            ReasonCode::ConfigRolledOut,
            format!("the pods rolled onto the startup layer {}", &boot.hash[..8]),
        ));
    }
    status.boot_hash = Some(boot.hash.clone());
    status.version = Some(workload::tag(&image).to_string());

    // 7. The hot layer's confirmation.
    if status.applied_config_hash.is_none() {
        // The first configuration: the pods started on it.
        accept(pass, ports, status, &target, &target_hash, events).await?;
    } else if status.applied_config_hash.as_deref() != Some(target_hash.as_str()) && !writes {
        let written = time(&status.config_written_time).unwrap_or(pass.now);
        let since = written.checked_sub(LOG_SLACK).unwrap_or(written);
        let mut answers = Vec::new();
        let mut unreadable = Vec::new();
        let mut dead_watchers = Vec::new();
        for pod in on_boot.iter().filter(|p| p.ready) {
            let log = if pod.started.is_some_and(|s| s > written) {
                None
            } else {
                match ports
                    .logs
                    .read(
                        &owner.namespace,
                        &pod.name,
                        &LogQuery {
                            container: Some("angos".into()),
                            since: Some(since),
                            limit_bytes: Some(pass.log_limit),
                        },
                    )
                    .await
                {
                    Ok(lines) => Some(lines),
                    Err(e) => {
                        unreadable.push(format!("{}: {e}", pod.name));
                        None
                    }
                }
            };
            let (state, why) = reload::pod_answer(pod, written, log.as_deref());
            if state == api::status::PodReloadState::Pending && log.is_some() {
                // A pod whose watcher died never answers: read its start.
                if let Ok(start) = ports
                    .logs
                    .read(
                        &owner.namespace,
                        &pod.name,
                        &LogQuery {
                            container: Some("angos".into()),
                            since: None,
                            limit_bytes: Some(reload::STARTUP_LOG_BYTES),
                        },
                    )
                    .await
                    && let Some(failure) = reload::watcher_failure(&start)
                {
                    dead_watchers.push(format!("{}: {failure}", pod.name));
                }
            }
            answers.push((pod.name.clone(), state, why));
        }
        status.pods = answers
            .iter()
            .map(|(name, state, _)| PodReload {
                name: name.clone(),
                state: *state,
                since: Some(written.to_string()),
            })
            .collect();
        match reload::confirmation(&answers) {
            Confirmation::Applied => {
                accept(pass, ports, status, &target, &target_hash, events).await?;
            }
            Confirmation::Rejected { pod, why } => {
                status.rejected_config_hash = Some(target_hash.clone());
                status.rejection_message = Some(why.clone());
                status.last_rejection_time = Some(pass.now.to_string());
                // D12: the accepted layer goes back, so a pod that restarts
                // starts.
                if let Some(applied) = &applied_file {
                    let restore = WorkloadInput {
                        hot_file: applied,
                        rollout_hash: input.rollout_hash,
                        ..input.clone()
                    };
                    ports
                        .objects
                        .apply(owner, &workload::objects(&restore))
                        .await
                        .map_err(workload_failure)?;
                    status.config_written_time = Some(pass.now.to_string());
                    status.written_config_hash = status.applied_config_hash.clone();
                }
                if why == reload::CRASHED {
                    // Its watcher is dead: restarted, the pods read the
                    // accepted layer back.
                    ports
                        .objects
                        .annotate_template(
                            owner,
                            &names.workload(),
                            &BTreeMap::from([(
                                annotations::RESTARTED_AT.to_string(),
                                format!("crashed-{}", &target_hash[..8]),
                            )]),
                            false,
                        )
                        .await
                        .map_err(workload_failure)?;
                }
                status.pods.clear();
                return Err(Failure::new(
                    ReasonCode::ConfigRejected,
                    format!(
                        "pod {pod} refused the configuration: {why}. The last accepted one was put \
                         back; it is tried again when the spec, a repository or a Secret changes"
                    ),
                ));
            }
            Confirmation::Pending(waiting) => {
                let overdue = pass.now.duration_since(written)
                    > SignedDuration::from_secs(spec.reload.timeout_seconds as i64);
                let roll = !unreadable.is_empty()
                    || !dead_watchers.is_empty()
                    || (overdue && spec.reload.on_unconfirmed == OnUnconfirmed::Rollout);
                if roll && status.rolled_out_for.as_deref() != Some(target_hash.as_str()) {
                    ports
                        .objects
                        .annotate_template(
                            owner,
                            &names.workload(),
                            &BTreeMap::from([(
                                annotations::RESTARTED_AT.to_string(),
                                target_hash.clone(),
                            )]),
                            false,
                        )
                        .await
                        .map_err(workload_failure)?;
                    status.rolled_out_for = Some(target_hash.clone());
                }
                if !dead_watchers.is_empty() {
                    advisories.push(advisory(
                        ReasonCode::ReloadUnconfirmed,
                        format!(
                            "a pod's configuration watcher stopped ({}): it never reloads, the \
                             pods were rolled instead (on a node short of inotify instances, raise \
                             fs.inotify.max_user_instances)",
                            dead_watchers.join("; ")
                        ),
                    ));
                } else if !unreadable.is_empty() {
                    advisories.push(advisory(
                        ReasonCode::ReloadUnconfirmed,
                        format!(
                            "the pods' logs can't be read ({}): the pods were rolled instead",
                            unreadable.join("; ")
                        ),
                    ));
                } else if overdue {
                    advisories.push(advisory(
                        ReasonCode::ReloadUnconfirmed,
                        format!(
                            "no answer from {} after {}s{}",
                            waiting.join(", "),
                            spec.reload.timeout_seconds,
                            if roll { ": the pods were rolled" } else { "" }
                        ),
                    ));
                }
                return Err(Failure::new(
                    ReasonCode::ConfigPropagating,
                    format!(
                        "configuration {} written, waiting for {}",
                        &target_hash[..8],
                        if waiting.is_empty() {
                            "a ready pod".to_string()
                        } else {
                            waiting.join(", ")
                        }
                    ),
                ));
            }
        }
    } else if writes {
        return Err(Failure::new(
            ReasonCode::ConfigPropagating,
            format!("configuration {} written", &target_hash[..8]),
        ));
    }

    if held {
        return Err(Failure::new(
            ReasonCode::ConfigRejected,
            format!(
                "the configuration was refused: {}. The last accepted one runs; it is tried again \
                 when the spec, a repository or a Secret changes",
                status.rejection_message.clone().unwrap_or_default()
            ),
        ));
    }
    // Every pod runs what was rendered this pass: what it holds is
    // confirmed, for the identities' targets and the repositories' status.
    for render in status.repositories.values_mut() {
        if render.state == api::status::RenderState::Rendered {
            render.applied_generation = render.generation;
        }
    }
    for render in status.identities.values_mut() {
        if render.state == api::status::RenderState::Rendered {
            render.applied_version = render.version;
        }
    }
    Ok(format!(
        "Angos {} serves {}",
        status.version.clone().unwrap_or_default(),
        status.endpoint.clone().unwrap_or_default()
    ))
}

/// Every ready pod runs `file`: it is the accepted one.
async fn accept(
    pass: &InstancePass<'_>,
    ports: &InstancePorts<'_>,
    status: &mut InstanceStatus,
    file: &str,
    hash: &str,
    events: &mut Vec<(ReasonCode, String)>,
) -> Result<(), Failure> {
    let names = Names(&pass.owner.name);
    let mut labels = workload::labels(&pass.owner.name, "config");
    labels.insert(api::CREDENTIAL_LABEL.into(), "true".into());
    let secret = k8s_openapi::api::core::v1::Secret {
        metadata: k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta {
            name: Some(names.config_applied()),
            namespace: Some(pass.owner.namespace.clone()),
            labels: Some(labels),
            ..Default::default()
        },
        data: Some(BTreeMap::from([(
            paths::FILE.into(),
            k8s_openapi::ByteString(file.as_bytes().to_vec()),
        )])),
        ..Default::default()
    };
    ports
        .objects
        .apply(
            pass.owner,
            &[weebo_kit_application::workload::OwnedObject::of(
                &secret, false,
            )],
        )
        .await
        .map_err(workload_failure)?;
    let first = status.applied_config_hash.is_none();
    status.applied_config_hash = Some(hash.to_string());
    status.last_applied_time = Some(pass.now.to_string());
    status.config_written_time = None;
    status.written_config_hash = None;
    status.pods.clear();
    if status.config_hash.as_deref() == Some(hash) {
        status.rejected_config_hash = None;
        status.rejection_message = None;
    }
    if !first {
        events.push((
            ReasonCode::ConfigReloaded,
            format!("every pod loaded the configuration {}", &hash[..8]),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests;
