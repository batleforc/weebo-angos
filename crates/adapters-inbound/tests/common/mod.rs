//! The operator's context as `operator::main` builds it, on a client of the
//! test's choosing: the real adapters, but the cluster issuer's (envtest
//! has none) and the logs (no kubelet) are in memory.

#![allow(dead_code)]

use std::sync::Arc;

use adapters_inbound::controller::{self, Ctx, Settings, Shutdown};
use adapters_outbound::{
    CelInterpreterChecker, KubeCredentials, KubeRepositories, LocalInstanceDirectory, RandomKeys,
};
use application::test_support::{FakeIssuer, FakePodLogs};
use kube::Client;
use kube::runtime::events::{Recorder, Reporter};
use weebo_kit_adapters::KubeOwnedObjects;
use weebo_kit_runtime::{Health, WatchScope};

pub fn ctx(client: &Client, scope: WatchScope, logs: Arc<FakePodLogs>) -> (Arc<Ctx>, Arc<Health>) {
    let health = Arc::new(Health::default());
    // One shared watch per kind, the stores read from them.
    let watches = controller::spawn_watches(client, &scope, &health);
    let mut instances = LocalInstanceDirectory::new(client.clone()).with_scope(
        scope.watched_namespace().map(String::from),
        scope.selector().cloned(),
    );
    if let Some(shared) = watches.get::<api::LocalInstance>() {
        instances = instances.with_store(shared.store());
    }
    let instances = Arc::new(instances);
    let repositories = Arc::new(
        KubeRepositories::new(client.clone())
            .with_scope(
                scope.watched_namespace().map(String::from),
                scope.selector().cloned(),
            )
            .with_watches(watches.clone()),
    );
    let ctx = Arc::new(Ctx {
        client: client.clone(),
        instances: instances.clone(),
        objects: Arc::new(KubeOwnedObjects::new(
            client.clone(),
            adapters_outbound::workload_settings(vec![api::angos::IMAGE_REPOSITORY.into()]),
        )),
        logs,
        credentials: Arc::new(KubeCredentials::new(client.clone())),
        repositories: repositories.clone(),
        identities: repositories,
        identity_objects: Arc::new(KubeOwnedObjects::new(
            client.clone(),
            adapters_outbound::identity_settings(),
        )),
        hasher: Arc::new(adapters_outbound::Argon2Hasher),
        maintenance_objects: Arc::new(KubeOwnedObjects::new(
            client.clone(),
            adapters_outbound::maintenance_settings(vec![api::angos::IMAGE_REPOSITORY.into()]),
        )),
        jobs: Arc::new(adapters_outbound::KubeJobs::new(client.clone())),
        maintenances: Arc::new(adapters_outbound::KubeMaintenances::new(client.clone())),
        secret_store: Arc::new(weebo_kit_adapters::K8sSecretStore::new(
            client.clone(),
            controller::FIELD_MANAGER,
            api::MANAGED_BY,
            "angos.weebo.io/owner",
        )),
        attached: instances.clone(),
        cel: Arc::new(CelInterpreterChecker),
        issuer: Arc::new(FakeIssuer(Some("https://kubernetes.default.svc".into()))),
        keys: Arc::new(RandomKeys),
        settings: Settings::default(),
        recorder: Recorder::new(
            client.clone(),
            Reporter {
                controller: controller::FIELD_MANAGER.into(),
                instance: None,
            },
        ),
        shutdown: Shutdown::never(),
        health: health.clone(),
        scope,
        watches,
    });
    (ctx, health)
}
