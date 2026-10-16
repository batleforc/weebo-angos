//! An `AngosIdentity` (spec 4.4): its password generated (or adopted) and
//! hashed into its own Secret, which the instance renders; the clear
//! password written to the targets only once every pod of the instance
//! accepted that version — a client never gets a password the registry
//! still refuses. A rotation (the annotation `angos.weebo.io/rotate`, or
//! `rotationDays`) is a new version on the same path; the targets keep the
//! previous password until the new one is confirmed (the registry refuses
//! it from the reload to the targets' update, a few seconds).

use std::collections::BTreeMap;

use api::annotations;
use api::identity::{IdentitySpec, IdentityStatus, keys, password_secret};
use api::status::RenderState;
use base64::Engine;
use domain::Failure;
use domain::error::ReasonCode;
use k8s_openapi::ByteString;
use k8s_openapi::api::core::v1::Secret;
use k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta;
use k8s_openapi::jiff::{SignedDuration, Timestamp};
use weebo_kit_api::SecretTarget;
use weebo_kit_application::emit;
use weebo_kit_application::ports::{SecretOwner, SecretStore};
use weebo_kit_application::workload::OwnedObject;

use super::{Outcome, instance_failure, workload_failure};
use crate::ports::{
    AttachedInstances, Credentials, KeyGenerator, OwnedObjects, Owner, PasswordHasher,
};

pub const KIND: &str = "AngosIdentity";

pub struct IdentityPass<'a> {
    pub owner: &'a Owner,
    pub spec: &'a IdentitySpec,
    pub annotations: &'a BTreeMap<String, String>,
    pub status: &'a IdentityStatus,
    pub now: Timestamp,
}

pub struct IdentityPorts<'a> {
    pub instances: &'a dyn AttachedInstances,
    /// The identity's own Secret (owned by the identity).
    pub objects: &'a dyn OwnedObjects,
    pub credentials: &'a dyn Credentials,
    pub keys: &'a dyn KeyGenerator,
    pub hasher: &'a dyn PasswordHasher,
    /// Where the clear password goes.
    pub store: &'a dyn SecretStore,
}

#[derive(Debug, Clone)]
pub struct IdentityReport {
    pub outcome: Outcome,
    pub status: IdentityStatus,
    pub events: Vec<(ReasonCode, String)>,
}

/// The identity's targets: its own, or `<name>-credentials`.
pub fn targets(spec: &IdentitySpec, name: &str, store: &dyn SecretStore) -> Vec<SecretTarget> {
    emit::targets(&spec.secret_targets, &format!("{name}-credentials"), store)
}

/// What the targets hold.
pub fn target_data(username: &str, password: &str, host: Option<&str>) -> BTreeMap<String, String> {
    let mut data = BTreeMap::from([
        ("username".to_string(), username.to_string()),
        ("password".to_string(), password.to_string()),
    ]);
    if let Some(host) = host {
        let auth =
            base64::engine::general_purpose::STANDARD.encode(format!("{username}:{password}"));
        data.insert(
            ".dockerconfigjson".into(),
            serde_json::json!({"auths": {host: {"username": username, "password": password, "auth": auth}}})
                .to_string(),
        );
    }
    data
}

pub async fn reconcile_identity(
    pass: IdentityPass<'_>,
    ports: &IdentityPorts<'_>,
) -> IdentityReport {
    let mut status = pass.status.clone();
    let mut events = Vec::new();
    let ready = run(&pass, ports, &mut status, &mut events).await;
    let mut outcome = match ready {
        Ok(message) => Outcome::synced(message),
        Err(failure) => Outcome::failed(failure),
    };
    outcome.instance = status.common.instance.clone();
    IdentityReport {
        outcome,
        status,
        events,
    }
}

async fn run(
    pass: &IdentityPass<'_>,
    ports: &IdentityPorts<'_>,
    status: &mut IdentityStatus,
    events: &mut Vec<(ReasonCode, String)>,
) -> Result<String, Failure> {
    let owner = pass.owner;
    let username = pass
        .spec
        .username
        .clone()
        .unwrap_or_else(|| owner.name.clone());
    status.username = Some(username.clone());
    let bound = ports
        .instances
        .resolve_attached(
            &owner.namespace,
            pass.spec.instance_ref.as_ref(),
            status.common.instance.as_ref(),
        )
        .await
        .map_err(instance_failure)?;
    status.common.instance = Some(bound.clone());
    let instance = ports
        .instances
        .instance(&bound)
        .await
        .map_err(|e| Failure::new(ReasonCode::KubernetesApiError, e))?
        .ok_or_else(|| Failure::new(ReasonCode::InstanceNotFound, format!("{bound} is gone")))?;

    // The password, hashed, in the identity's own Secret.
    let secret = password_secret(&owner.name);
    let read = |key: &'static str| {
        let secret = secret.clone();
        async move {
            ports
                .objects
                .secret_text(owner, &secret, key)
                .await
                .map_err(workload_failure)
        }
    };
    let stored = read(keys::PASSWORD).await?;
    let version: u64 = read(keys::VERSION)
        .await?
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    let nonce = pass
        .annotations
        .get(annotations::ROTATE)
        .cloned()
        .unwrap_or_default();
    let rotated_at: Option<Timestamp> = read(keys::ROTATED_AT).await?.and_then(|t| t.parse().ok());
    let due = pass.spec.rotation_days.is_some_and(|days| {
        rotated_at.is_some_and(|at| {
            pass.now.duration_since(at) >= SignedDuration::from_hours(24 * i64::from(days))
        })
    });
    let rotate =
        stored.is_some() && (read(keys::ROTATION).await?.unwrap_or_default() != nonce || due);
    let (password, version) = if stored.is_none() || rotate {
        let password = match (&stored, &pass.spec.adopt_secret_ref) {
            (None, Some(r)) => ports
                .credentials
                .read(&owner.namespace, &r.name, &r.key)
                .await
                .map_err(|e| Failure::new(e.reason(), e.to_string()))?
                .trim()
                .to_string(),
            _ => ports.keys.password(),
        };
        let version = version + 1;
        let mut labels = BTreeMap::from([
            (
                "app.kubernetes.io/managed-by".to_string(),
                api::MANAGED_BY.to_string(),
            ),
            (
                "app.kubernetes.io/component".to_string(),
                "identity".to_string(),
            ),
            (api::CREDENTIAL_LABEL.to_string(), "true".to_string()),
        ]);
        labels.insert("app.kubernetes.io/instance".into(), owner.name.clone());
        let data = [
            (keys::PASSWORD, password.clone()),
            (keys::HASH, ports.hasher.hash(&password)),
            (keys::VERSION, version.to_string()),
            (keys::ROTATION, nonce.clone()),
            (keys::ROTATED_AT, pass.now.to_string()),
        ];
        let object = Secret {
            metadata: ObjectMeta {
                name: Some(secret.clone()),
                namespace: Some(owner.namespace.clone()),
                labels: Some(labels),
                ..Default::default()
            },
            data: Some(
                data.into_iter()
                    .map(|(k, v)| (k.to_string(), ByteString(v.into_bytes())))
                    .collect(),
            ),
            ..Default::default()
        };
        ports
            .objects
            .apply(owner, &[OwnedObject::of(&object, false)])
            .await
            .map_err(workload_failure)?;
        (password, version)
    } else {
        (stored.unwrap_or_default(), version.max(1))
    };
    status.pending_password_version = (status.password_version != Some(version)).then_some(version);

    // What the instance made of it.
    let instance_status = instance.status.clone().unwrap_or_default();
    let key = format!("{}/{}", owner.namespace, owner.name);
    let render = instance_status.identities.get(&key);
    status.conflict_with = render.and_then(|r| r.conflict_with.clone());
    let waiting = |what: String| Failure::new(ReasonCode::ConfigPropagating, what);
    let Some(render) = render else {
        return Err(waiting(format!("waiting for {bound} to render it")));
    };
    match render.state {
        RenderState::Conflict => {
            return Err(Failure::new(
                ReasonCode::UsernameConflict,
                render.message.clone().unwrap_or_default(),
            ));
        }
        RenderState::SecretNotAllowed => {
            return Err(Failure::new(
                ReasonCode::SecretNotAllowed,
                render.message.clone().unwrap_or_default(),
            ));
        }
        RenderState::SecretMissing | RenderState::Invalid => {
            return Err(waiting(format!("waiting for {bound} to render it")));
        }
        RenderState::Rendered => {}
    }
    let confirmed = instance_status.config_hash.is_some()
        && instance_status.applied_config_hash == instance_status.config_hash;
    if render.version != Some(version) || !confirmed {
        return Err(waiting(format!(
            "password version {version} waits for every pod of {bound} to accept it; the targets \
             keep the previous one"
        )));
    }

    // Confirmed: the targets get it.
    let secret_owner = SecretOwner::namespaced(KIND, &owner.namespace, &owner.name);
    let targets = targets(pass.spec, &owner.name, ports.store);
    emit::check_targets(&targets, &secret_owner)
        .map_err(|e| Failure::new(ReasonCode::InvalidSpec, e.to_string()))?;
    let store_failure = |e: weebo_kit_application::ports::SecretStoreError| {
        Failure::new(ReasonCode::SecretMissing, e.to_string())
    };
    let missing = emit::any_missing(&targets, &secret_owner, ports.store)
        .await
        .map_err(store_failure)?;
    if status.password_version != Some(version) || missing {
        let data = target_data(&username, &password, pass.spec.registry_host.as_deref());
        emit::write_all(&targets, &secret_owner, &data, ports.store)
            .await
            .map_err(store_failure)?;
        if status.password_version.is_some_and(|v| v < version) {
            events.push((
                ReasonCode::PasswordRotated,
                format!("password version {version} written to the targets"),
            ));
            status.last_rotation_time = Some(pass.now.to_string());
        }
        status.password_version = Some(version);
        status.pending_password_version = None;
    }
    Ok(format!("{username} signs in to {bound}"))
}

/// The identity's targets removed (its finalizer).
pub async fn release(
    spec: &IdentitySpec,
    owner: &Owner,
    store: &dyn SecretStore,
) -> Result<(), Failure> {
    let secret_owner = SecretOwner::namespaced(KIND, &owner.namespace, &owner.name);
    emit::delete_all(&targets(spec, &owner.name, store), &secret_owner, store)
        .await
        .map_err(|e| Failure::new(ReasonCode::KubernetesApiError, e.to_string()))
}

#[cfg(test)]
mod tests;
