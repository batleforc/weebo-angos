use std::sync::Mutex;

use api::LocalInstance;
use api::status::{IdentityRender, InstanceStatus};
use serde_json::json;
use weebo_kit_application::test_support::FakeSecretStore;

use super::*;
use crate::ports::{BoundInstance, InstanceError};
use crate::test_support::{CountingKeys, FakeCredentials, FakeHasher, FakeOwnedObjects};

struct Instances(Mutex<LocalInstance>);

#[async_trait::async_trait]
impl AttachedInstances for Instances {
    async fn resolve_attached(
        &self,
        namespace: &str,
        _: Option<&weebo_kit_api::InstanceRef>,
        _: Option<&BoundInstance>,
    ) -> Result<BoundInstance, InstanceError> {
        Ok(BoundInstance::local(namespace, "reg"))
    }

    async fn instance(&self, _: &BoundInstance) -> Result<Option<LocalInstance>, String> {
        Ok(Some(self.0.lock().unwrap().clone()))
    }
}

struct World {
    owner: Owner,
    spec: IdentitySpec,
    annotations: BTreeMap<String, String>,
    status: IdentityStatus,
    now: Timestamp,
    instances: Instances,
    objects: FakeOwnedObjects,
    credentials: FakeCredentials,
    keys: CountingKeys,
    store: FakeSecretStore,
}

impl World {
    fn new(spec: serde_json::Value) -> Self {
        Self {
            owner: Owner {
                namespace: "team".into(),
                name: "ci".into(),
                uid: "u-1".into(),
            },
            spec: serde_json::from_value(spec).unwrap(),
            annotations: BTreeMap::new(),
            status: IdentityStatus::default(),
            now: "2026-10-16T10:00:00Z".parse().unwrap(),
            instances: Instances(Mutex::new(LocalInstance::new("reg", Default::default()))),
            objects: FakeOwnedObjects::default(),
            credentials: FakeCredentials::default(),
            keys: CountingKeys::default(),
            store: FakeSecretStore::default(),
        }
    }

    async fn pass(&mut self) -> IdentityReport {
        let report = reconcile_identity(
            IdentityPass {
                owner: &self.owner,
                spec: &self.spec,
                annotations: &self.annotations,
                status: &self.status,
                now: self.now,
            },
            &IdentityPorts {
                instances: &self.instances,
                objects: &self.objects,
                credentials: &self.credentials,
                keys: &self.keys,
                hasher: &FakeHasher,
                store: &self.store,
            },
        )
        .await;
        self.status = report.status.clone();
        report
    }

    /// The instance renders `version` of `team/ci`, confirmed or not.
    fn instance_renders(&self, version: u64, confirmed: bool, state: RenderState) {
        let mut status = InstanceStatus {
            config_hash: Some("h".into()),
            applied_config_hash: Some(if confirmed { "h" } else { "old" }.into()),
            ..Default::default()
        };
        status.identities.insert(
            "team/ci".into(),
            IdentityRender {
                username: "ci".into(),
                state,
                version: Some(version),
                applied_version: confirmed.then_some(version),
                message: Some("why".into()),
                conflict_with: (state == RenderState::Conflict).then(|| "other/ci".into()),
            },
        );
        self.instances.0.lock().unwrap().status = Some(status);
    }

    fn stored(&self, key: &str) -> Option<String> {
        self.objects.secret("ci-angos-password", key)
    }

    fn target(&self) -> Option<BTreeMap<String, String>> {
        self.store.get("team/ci-credentials")
    }
}

#[tokio::test]
async fn a_password_reaches_the_targets_once_the_instance_runs_it() {
    let mut world = World::new(json!({"registryHost": "registry.example"}));
    let created = world.pass().await;
    assert_eq!(created.outcome.reason(), ReasonCode::ConfigPropagating);
    assert_eq!(world.stored(keys::VERSION).as_deref(), Some("1"));
    let password = world.stored(keys::PASSWORD).unwrap();
    assert_eq!(
        world.stored(keys::HASH),
        Some(format!("$argon2id$fake${password}"))
    );
    assert_eq!(created.status.pending_password_version, Some(1));
    assert!(world.target().is_none());
    // Rendered but not confirmed yet: still nothing.
    world.instance_renders(1, false, RenderState::Rendered);
    assert_eq!(
        world.pass().await.outcome.reason(),
        ReasonCode::ConfigPropagating
    );
    assert!(world.target().is_none());
    world.instance_renders(1, true, RenderState::Rendered);
    let ready = world.pass().await;
    assert_eq!(
        ready.outcome.reason(),
        ReasonCode::Reconciled,
        "{:?}",
        ready.outcome
    );
    let target = world.target().unwrap();
    assert_eq!(target["username"], "ci");
    assert_eq!(target["password"], password);
    assert!(target[".dockerconfigjson"].contains("registry.example"));
    assert_eq!(ready.status.password_version, Some(1));
    assert_eq!(ready.status.pending_password_version, None);
}

#[tokio::test]
async fn a_rotation_keeps_the_previous_password_until_confirmed() {
    let mut world = World::new(json!({}));
    world.pass().await;
    world.instance_renders(1, true, RenderState::Rendered);
    world.pass().await;
    let first = world.target().unwrap()["password"].clone();
    world
        .annotations
        .insert(annotations::ROTATE.into(), "now".into());
    let rotating = world.pass().await;
    assert_eq!(rotating.outcome.reason(), ReasonCode::ConfigPropagating);
    assert_eq!(world.stored(keys::VERSION).as_deref(), Some("2"));
    assert_eq!(world.target().unwrap()["password"], first);
    // The same nonce doesn't rotate again.
    world.pass().await;
    assert_eq!(world.stored(keys::VERSION).as_deref(), Some("2"));
    world.instance_renders(2, true, RenderState::Rendered);
    let rotated = world.pass().await;
    assert_eq!(rotated.outcome.reason(), ReasonCode::Reconciled);
    assert_ne!(world.target().unwrap()["password"], first);
    assert!(
        rotated
            .events
            .iter()
            .any(|(r, _)| *r == ReasonCode::PasswordRotated)
    );
}

#[tokio::test]
async fn a_periodic_rotation_comes_due() {
    let mut world = World::new(json!({"rotationDays": 30}));
    world.pass().await;
    world.now += SignedDuration::from_hours(24 * 31);
    world.pass().await;
    assert_eq!(world.stored(keys::VERSION).as_deref(), Some("2"));
}

#[tokio::test]
async fn an_adopted_password_is_kept() {
    let mut world = World::new(json!({"adoptSecretRef": {"name": "legacy", "key": "password"}}));
    world
        .credentials
        .insert("team", "legacy", &[("password", "s3cr3t\n")]);
    world.pass().await;
    assert_eq!(world.stored(keys::PASSWORD).as_deref(), Some("s3cr3t"));
}

#[tokio::test]
async fn a_username_in_use_is_refused() {
    let mut world = World::new(json!({}));
    world.instance_renders(1, true, RenderState::Conflict);
    let report = world.pass().await;
    assert_eq!(report.outcome.reason(), ReasonCode::UsernameConflict);
    assert_eq!(report.status.conflict_with.as_deref(), Some("other/ci"));
    assert!(world.target().is_none());
}
