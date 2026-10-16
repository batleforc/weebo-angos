//! Integration tests: the `AngosLocalInstance` controller on a real
//! control plane (envtest: no kubelet, so no pod ever runs) with the kit's
//! `KubeOwnedObjects` — the objects of an instance, its two layers, a hot
//! change rewritten in place, a startup change under a new Secret, an
//! object never taken over, a Secret that doesn't opt in, the finalizer.

use std::time::Duration;

use adapters_inbound::controller;
use api::LocalInstance;
use api::local_instance::LocalInstanceSpec;
use k8s_openapi::ByteString;
use k8s_openapi::api::apps::v1::Deployment;
use k8s_openapi::api::core::v1::{Namespace, Secret, Service};
use kube::api::{Api, ObjectMeta, Patch, PatchParams, PostParams};
use kube::{Client, ResourceExt};
use weebo_kit_runtime::WatchScope;

mod common;

const TIMEOUT: Duration = Duration::from_secs(20);
const POLL: Duration = Duration::from_millis(200);
const NS: &str = "team";

fn ready(instance: &LocalInstance) -> Option<(String, String)> {
    let c = weebo_kit_api::status::ready(&instance.status.as_ref()?.conditions)?;
    Some((c.status.clone(), c.reason.clone()))
}

fn instance(name: &str, spec: serde_json::Value) -> LocalInstance {
    let spec: LocalInstanceSpec = serde_json::from_value(spec).unwrap();
    LocalInstance::new(name, spec)
}

async fn file(client: &Client, name: &str) -> String {
    let secret = Api::<Secret>::namespaced(client.clone(), NS)
        .get(name)
        .await
        .unwrap();
    String::from_utf8(secret.data.unwrap()["config.toml"].0.clone()).unwrap()
}

async fn reason_is(api: &Api<LocalInstance>, name: &str, reason: &str) {
    let wanted = reason.to_string();
    testkit::polling::wait_for(api, name, TIMEOUT, POLL, |i| {
        ready(i).filter(|(_, r)| *r == wanted)
    })
    .await;
}

#[tokio::test]
async fn an_instance_is_deployed_from_its_two_layers() {
    let _ = tracing_subscriber::fmt().with_test_writer().try_init();
    let cluster = testkit::envtest::start().await;
    let client = cluster.client();
    Api::<Namespace>::all(client.clone())
        .create(
            &PostParams::default(),
            &Namespace {
                metadata: ObjectMeta {
                    name: Some(NS.into()),
                    ..Default::default()
                },
                ..Default::default()
            },
        )
        .await
        .unwrap();
    let (ctx, health) = common::ctx(&client, WatchScope::cluster(), Default::default());
    tokio::spawn(controller::local_instance::run(client.clone(), ctx));
    let instances: Api<LocalInstance> = Api::namespaced(client.clone(), NS);
    let pp = PostParams::default();

    // ── Deployed: both layers, the Deployment on them ─────────────────────
    instances
        .create(
            &pp,
            &instance("reg", serde_json::json!({"storage": {"fs": {}}})),
        )
        .await
        .unwrap();
    reason_is(&instances, "reg", "WorkloadNotReady").await;
    let reg = instances.get("reg").await.unwrap();
    assert!(
        reg.finalizers()
            .contains(&controller::FINALIZER.to_string())
    );
    let status = reg.status.clone().unwrap();
    let boot_hash = status.boot_history[0].clone();
    let boot_name = format!("reg-boot-{}", &boot_hash[..8]);
    let boot = Api::<Secret>::namespaced(client.clone(), NS)
        .get(&boot_name)
        .await
        .unwrap();
    assert_eq!(boot.immutable, Some(true));
    assert!(file(&client, &boot_name).await.contains("[blob_store.fs]"));
    assert!(
        file(&client, "reg-config")
            .await
            .contains("has_repository_policy()")
    );
    assert!(status.config_written_time.is_some());
    let deployment = Api::<Deployment>::namespaced(client.clone(), NS)
        .get("reg")
        .await
        .unwrap();
    let template = deployment.spec.unwrap().template;
    assert_eq!(
        template.metadata.unwrap().annotations.unwrap()[api::annotations::BOOT_HASH],
        boot_hash
    );
    let container = &template.spec.unwrap().containers[0];
    assert_eq!(
        container.image.as_deref(),
        Some("ghcr.io/project-angos/angos:1.12.3")
    );
    assert_eq!(
        container.args.as_ref().unwrap().last().map(String::as_str),
        Some("server")
    );
    Api::<Service>::namespaced(client.clone(), NS)
        .get("reg")
        .await
        .unwrap();

    // ── A hot key: rewritten in place, no new startup layer ───────────────
    instances
        .patch(
            "reg",
            &PatchParams::default(),
            &Patch::Merge(serde_json::json!({"spec": {"global": {"gcGraceSecs": 900}}})),
        )
        .await
        .unwrap();
    let deadline = tokio::time::Instant::now() + TIMEOUT;
    while !file(&client, "reg-config")
        .await
        .contains("gc_grace_secs = 900")
    {
        assert!(
            tokio::time::Instant::now() < deadline,
            "reg-config never rewritten"
        );
        tokio::time::sleep(POLL).await;
    }
    assert_eq!(
        instances
            .get("reg")
            .await
            .unwrap()
            .status
            .unwrap()
            .boot_history,
        std::slice::from_ref(&boot_hash)
    );

    // ── A startup key: a new Secret, the Deployment on it ─────────────────
    instances
        .patch(
            "reg",
            &PatchParams::default(),
            &Patch::Merge(serde_json::json!({"spec": {"server": {"port": 9000}}})),
        )
        .await
        .unwrap();
    let history = testkit::polling::wait_for(&instances, "reg", TIMEOUT, POLL, |i| {
        let history = &i.status.as_ref()?.boot_history;
        (history.len() == 2).then(|| history.clone())
    })
    .await;
    assert_eq!(history[1], boot_hash);
    assert!(
        file(&client, &format!("reg-boot-{}", &history[0][..8]))
            .await
            .contains("port = 9000")
    );

    // ── An object that isn't the instance's is never taken over ───────────
    Api::<Service>::namespaced(client.clone(), NS)
        .create(
            &pp,
            &Service {
                metadata: ObjectMeta {
                    name: Some("taken".into()),
                    ..Default::default()
                },
                spec: Some(k8s_openapi::api::core::v1::ServiceSpec {
                    ports: Some(vec![k8s_openapi::api::core::v1::ServicePort {
                        port: 80,
                        ..Default::default()
                    }]),
                    ..Default::default()
                }),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    instances
        .create(
            &pp,
            &instance("taken", serde_json::json!({"storage": {"fs": {}}})),
        )
        .await
        .unwrap();
    reason_is(&instances, "taken", "OwnedObjectConflict").await;

    // ── A Secret that doesn't opt in is never read ────────────────────────
    Api::<Secret>::namespaced(client.clone(), NS)
        .create(
            &pp,
            &Secret {
                metadata: ObjectMeta {
                    name: Some("s3".into()),
                    ..Default::default()
                },
                data: Some(
                    [
                        ("access-key-id".to_string(), ByteString(b"AK".to_vec())),
                        ("secret-key".to_string(), ByteString(b"SK".to_vec())),
                    ]
                    .into(),
                ),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    instances
        .create(
            &pp,
            &instance(
                "s3",
                serde_json::json!({"storage": {"s3": {
                    "endpoint": "http://s3", "bucket": "b", "region": "us-east-1",
                    "credentialsSecretRef": {"name": "s3"},
                }}}),
            ),
        )
        .await
        .unwrap();
    reason_is(&instances, "s3", "SecretNotAllowed").await;

    // ── Deleted: the finalizer lets go ────────────────────────────────────
    instances.delete("reg", &Default::default()).await.unwrap();
    testkit::polling::wait_for_absence(&instances, "reg", TIMEOUT, POLL).await;
    assert_eq!(health.live(), Ok(()));
}
