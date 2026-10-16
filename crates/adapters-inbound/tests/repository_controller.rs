//! Integration tests: `AngosRepository` and `AngosLocalInstance` together
//! on a real control plane (no kubelet: the instance's pods never run, so
//! nothing is ever confirmed) — a repository bound to the namespace's
//! instance and rendered into its hot layer, an overlapping one refused
//! (`PrefixConflict`), one whose rule Angos's engine refuses
//! (`InvalidSpec`), and a namespace the instance doesn't accept.

use std::time::Duration;

use adapters_inbound::controller;
use api::{LocalInstance, Repository};
use k8s_openapi::api::core::v1::{Namespace, Secret};
use kube::Client;
use kube::api::{Api, ObjectMeta, PostParams};
use weebo_kit_runtime::WatchScope;

mod common;

const TIMEOUT: Duration = Duration::from_secs(30);
const POLL: Duration = Duration::from_millis(200);

fn reason(repo: &Repository) -> Option<String> {
    let c = weebo_kit_api::status::ready(&repo.status.as_ref()?.common.conditions)?;
    Some(c.reason.clone())
}

async fn namespace(client: &Client, name: &str) {
    Api::<Namespace>::all(client.clone())
        .create(
            &PostParams::default(),
            &Namespace {
                metadata: ObjectMeta {
                    name: Some(name.into()),
                    ..Default::default()
                },
                ..Default::default()
            },
        )
        .await
        .unwrap();
}

fn repo(name: &str, spec: serde_json::Value) -> Repository {
    Repository::new(name, serde_json::from_value(spec).unwrap())
}

async fn reason_is(api: &Api<Repository>, name: &str, wanted: &str) {
    let wanted = wanted.to_string();
    testkit::polling::wait_for(api, name, TIMEOUT, POLL, |r| {
        reason(r).filter(|got| *got == wanted)
    })
    .await;
}

#[tokio::test]
async fn repositories_are_rendered_into_their_instance() {
    let _ = tracing_subscriber::fmt().with_test_writer().try_init();
    let cluster = testkit::envtest::start().await;
    let client = cluster.client();
    namespace(&client, "team").await;
    namespace(&client, "other").await;
    let (ctx, health) = common::ctx(&client, WatchScope::cluster(), Default::default());
    tokio::spawn(controller::local_instance::run(client.clone(), ctx.clone()));
    tokio::spawn(controller::repository::run(client.clone(), ctx));
    let pp = PostParams::default();
    Api::<LocalInstance>::namespaced(client.clone(), "team")
        .create(
            &pp,
            &LocalInstance::new(
                "reg",
                serde_json::from_value(serde_json::json!({"storage": {"fs": {}}})).unwrap(),
            ),
        )
        .await
        .unwrap();
    let team: Api<Repository> = Api::namespaced(client.clone(), "team");
    team.create(
        &pp,
        &repo("team", serde_json::json!({"prefix": "team", "access": {"pull": ["anonymous"], "push": ["identity:ci"]}})),
    )
    .await
    .unwrap();
    // Bound and rendered; never confirmed without pods.
    reason_is(&team, "team", "ConfigPropagating").await;
    let bound = team.get("team").await.unwrap().status.unwrap();
    assert!(bound.rendered);
    assert_eq!(bound.common.instance.unwrap().name, "reg");
    let hot = Api::<Secret>::namespaced(client.clone(), "team")
        .get("reg-config")
        .await
        .unwrap();
    let hot = String::from_utf8(hot.data.unwrap()["config.toml"].0.clone()).unwrap();
    assert!(hot.contains("[repository.team.access_policy]"), "{hot}");
    assert!(hot.contains("identity.username == \"ci\""), "{hot}");

    // Overlapping, younger: refused.
    team.create(&pp, &repo("app", serde_json::json!({"prefix": "team/app"})))
        .await
        .unwrap();
    reason_is(&team, "app", "PrefixConflict").await;
    assert_eq!(
        team.get("app")
            .await
            .unwrap()
            .status
            .unwrap()
            .conflict_with
            .as_deref(),
        Some("team/team")
    );

    // A rule Angos's engine refuses (it would crash Angos's parser).
    team.create(
        &pp,
        &repo(
            "broken",
            serde_json::json!({"prefix": "broken", "access": {"rules": ["request.action =="]}}),
        ),
    )
    .await
    .unwrap();
    reason_is(&team, "broken", "InvalidSpec").await;

    // Another namespace, without the instance's consent.
    let other: Api<Repository> = Api::namespaced(client.clone(), "other");
    other
        .create(
            &pp,
            &repo(
                "far",
                serde_json::json!({"prefix": "far", "instanceRef": {"name": "reg", "namespace": "team"}}),
            ),
        )
        .await
        .unwrap();
    reason_is(&other, "far", "InstanceNotAllowed").await;
    assert_eq!(health.live(), Ok(()));
}
