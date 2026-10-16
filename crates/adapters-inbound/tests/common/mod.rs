//! The operator's context as `operator::main` builds it, on a client of the
//! test's choosing: the real adapters, but the cluster issuer's (envtest
//! has none) and the logs (no kubelet) are in memory.

#![allow(dead_code)]

use std::sync::Arc;

use adapters_inbound::controller::{self, Ctx, Settings, Shutdown};
use adapters_outbound::{KubeCredentials, LocalInstanceDirectory, RandomKeys};
use application::test_support::{FakeIssuer, FakePodLogs};
use kube::Client;
use kube::runtime::events::{Recorder, Reporter};
use weebo_kit_adapters::KubeOwnedObjects;
use weebo_kit_runtime::{Health, WatchScope};

pub fn ctx(client: &Client, scope: WatchScope, logs: Arc<FakePodLogs>) -> (Arc<Ctx>, Arc<Health>) {
    let health = Arc::new(Health::default());
    let instances = Arc::new(LocalInstanceDirectory::new(client.clone()).with_scope(
        scope.watched_namespace().map(String::from),
        scope.selector().cloned(),
    ));
    let ctx = Arc::new(Ctx {
        client: client.clone(),
        instances,
        objects: Arc::new(KubeOwnedObjects::new(
            client.clone(),
            adapters_outbound::workload_settings(vec![api::angos::IMAGE_REPOSITORY.into()]),
        )),
        logs,
        credentials: Arc::new(KubeCredentials::new(client.clone())),
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
    });
    (ctx, health)
}
