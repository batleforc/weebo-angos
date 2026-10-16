use std::collections::BTreeMap;

use k8s_openapi::jiff::{SignedDuration, Timestamp};
use serde_json::json;
use weebo_kit_application::workload::{OwnedObjects, PodState};

use super::*;
use crate::test_support::{
    CountingKeys, FakeCredentials, FakeIssuer, FakeLedger, FakeOwnedObjects, FakePodLogs,
};
use api::status::PodReloadState;

const IMAGE: &str = "ghcr.io/project-angos/angos:1.12.3";
const W: &str = crate::reload::WATCHER_TARGET;

/// An instance, the cluster around it, and the status passes carry.
struct World {
    owner: Owner,
    spec: LocalInstanceSpec,
    annotations: BTreeMap<String, String>,
    status: InstanceStatus,
    now: Timestamp,
    objects: FakeOwnedObjects,
    logs: FakePodLogs,
    credentials: FakeCredentials,
    issuer: FakeIssuer,
    keys: CountingKeys,
    ledger: FakeLedger,
}

impl World {
    fn new(spec: serde_json::Value) -> Self {
        Self {
            owner: Owner {
                namespace: "team".into(),
                name: "reg".into(),
                uid: "u-1".into(),
            },
            spec: serde_json::from_value(spec).unwrap(),
            annotations: BTreeMap::new(),
            status: InstanceStatus::default(),
            now: "2026-10-16T10:00:00Z".parse().unwrap(),
            objects: FakeOwnedObjects::default(),
            logs: FakePodLogs::default(),
            credentials: FakeCredentials::default(),
            issuer: FakeIssuer(Some("https://kubernetes.default.svc.cluster.local".into())),
            keys: CountingKeys::default(),
            ledger: FakeLedger::default(),
        }
    }

    fn fs() -> Self {
        Self::new(json!({"storage": {"fs": {}}}))
    }

    /// One pass, its status kept for the next; the clock moves a minute.
    async fn pass(&mut self) -> InstanceReport {
        let report = reconcile_instance(
            InstancePass {
                owner: &self.owner,
                spec: &self.spec,
                annotations: &self.annotations,
                status: &self.status,
                now: self.now,
                default_image: IMAGE,
                log_limit: 1 << 20,
            },
            &InstancePorts {
                objects: &self.objects,
                logs: &self.logs,
                credentials: &self.credentials,
                issuer: &self.issuer,
                keys: &self.keys,
                ledger: &self.ledger,
            },
        )
        .await;
        self.status = report.status.clone();
        self.now = self.now.checked_add(SignedDuration::from_mins(1)).unwrap();
        report
    }

    /// A ready pod on the current startup layer, started at `started`.
    fn pod(&self, name: &str, started: &str) {
        self.objects.set_pods(vec![PodState {
            name: name.into(),
            ready: true,
            started: Some(started.parse().unwrap()),
            annotations: BTreeMap::from([(
                annotations::BOOT_HASH.to_string(),
                self.status.boot_hash.clone().unwrap_or_default(),
            )]),
            ..Default::default()
        }]);
    }

    fn log(&self, pod: &str, at: Timestamp, level: &str, target: &str, message: &str) {
        self.logs.push(
            "team",
            pod,
            at,
            &json!({"level": level, "target": target, "fields": {"message": message}}).to_string(),
        );
    }

    fn hot(&self) -> String {
        self.objects.secret("reg-config", paths::FILE).unwrap()
    }

    fn written(&self) -> Timestamp {
        self.status
            .config_written_time
            .as_deref()
            .unwrap()
            .parse()
            .unwrap()
    }

    /// The template's startup hash.
    async fn template_boot_hash(&self) -> String {
        let rollout = self
            .objects
            .rollout(&self.owner, "reg")
            .await
            .unwrap()
            .unwrap();
        rollout
            .annotation(annotations::BOOT_HASH)
            .unwrap()
            .to_string()
    }
}

fn reason(report: &InstanceReport) -> ReasonCode {
    report.outcome.reason()
}

fn advisories(report: &InstanceReport) -> Vec<ReasonCode> {
    report.outcome.advisories.iter().map(|a| a.reason).collect()
}

/// A first pass: everything created, the pods started on the files.
async fn running(world: &mut World) -> InstanceReport {
    let report = world.pass().await;
    assert_eq!(
        reason(&report),
        ReasonCode::Reconciled,
        "{:?}",
        report.outcome
    );
    report
}

#[tokio::test]
async fn a_new_instance_runs_its_first_configuration() {
    let mut world = World::fs();
    let report = running(&mut world).await;
    let status = &report.status;
    assert_eq!(status.version.as_deref(), Some("1.12.3"));
    assert_eq!(status.endpoint.as_deref(), Some("reg.team.svc:8000"));
    assert_eq!(status.applied_config_hash, status.config_hash);
    assert!(status.boot_hash.is_some());
    // Both layers, and the copy of the accepted one.
    assert!(
        world
            .objects
            .secret("reg-config-applied", paths::FILE)
            .is_some()
    );
    let boot = boot_secret("reg", status.boot_hash.as_deref().unwrap());
    assert!(
        world
            .objects
            .secret(&boot, paths::FILE)
            .unwrap()
            .contains("[blob_store.fs]")
    );
    // The write was recorded right after it happened.
    assert_eq!(world.ledger.0.lock().unwrap().len(), 1);
    assert!(report.events.is_empty());
}

#[tokio::test]
async fn a_hot_change_is_confirmed_by_each_pod_s_log() {
    let mut world = World::fs();
    running(&mut world).await;
    world.pod("reg-1", "2026-10-16T09:00:00Z");
    world.spec.global.gc_grace_secs = Some(600);
    let written = world.pass().await;
    assert_eq!(reason(&written), ReasonCode::ConfigPropagating);
    assert!(world.hot().contains("gc_grace_secs = 600"));
    let at = world.written();
    // No answer yet.
    let waiting = world.pass().await;
    assert_eq!(reason(&waiting), ReasonCode::ConfigPropagating);
    assert_eq!(waiting.status.pods[0].state, PodReloadState::Pending);
    world.log(
        "reg-1",
        at + SignedDuration::from_secs(40),
        "INFO",
        W,
        reload::RELOADED,
    );
    let applied = world.pass().await;
    assert_eq!(
        reason(&applied),
        ReasonCode::Reconciled,
        "{:?}",
        applied.outcome
    );
    assert_eq!(
        applied.status.applied_config_hash,
        applied.status.config_hash
    );
    assert!(
        applied
            .events
            .iter()
            .any(|(r, _)| *r == ReasonCode::ConfigReloaded)
    );
    assert!(
        world
            .objects
            .secret("reg-config-applied", paths::FILE)
            .unwrap()
            .contains("gc_grace_secs = 600")
    );
    // The startup layer didn't move: no roll.
    assert!(
        world
            .objects
            .log()
            .iter()
            .all(|l| !l.starts_with("restarted"))
    );
}

#[tokio::test]
async fn a_refused_configuration_is_replaced_by_the_accepted_one() {
    let mut world = World::fs();
    running(&mut world).await;
    let accepted = world.hot();
    world.pod("reg-1", "2026-10-16T09:00:00Z");
    world.spec.global.gc_grace_secs = Some(600);
    world.pass().await;
    let at = world.written();
    world.log(
        "reg-1",
        at + SignedDuration::from_secs(30),
        "WARN",
        W,
        "Failed to reload configuration: invalid configuration format: something \"secret\"",
    );
    let refused = world.pass().await;
    assert_eq!(reason(&refused), ReasonCode::ConfigRejected);
    let message = refused.status.rejection_message.clone().unwrap();
    assert!(!message.contains("secret\""), "{message}");
    // D12: the accepted layer is back.
    assert_eq!(world.hot(), accepted);
    // Held: not written again while nothing changes.
    let held = world.pass().await;
    assert_eq!(reason(&held), ReasonCode::ConfigRejected);
    assert_eq!(world.hot(), accepted);
    // A change of the spec is tried.
    world.spec.global.gc_grace_secs = Some(700);
    let retried = world.pass().await;
    assert_eq!(reason(&retried), ReasonCode::ConfigPropagating);
    assert!(world.hot().contains("gc_grace_secs = 700"));
}

#[tokio::test]
async fn a_refused_configuration_is_tried_again_after_ten_minutes() {
    let mut world = World::fs();
    running(&mut world).await;
    world.pod("reg-1", "2026-10-16T09:00:00Z");
    world.spec.global.gc_grace_secs = Some(600);
    world.pass().await;
    let at = world.written();
    world.log(
        "reg-1",
        at + SignedDuration::from_secs(1),
        "ERROR",
        "angos::command::server::command::notifier",
        "Failed to apply configuration: JWKS unreachable",
    );
    world.log(
        "reg-1",
        at + SignedDuration::from_secs(1),
        "WARN",
        W,
        reload::NOT_APPLIED,
    );
    let refused = world.pass().await;
    assert_eq!(
        refused.status.rejection_message.as_deref(),
        Some("JWKS unreachable")
    );
    world.now += SignedDuration::from_mins(11);
    let retried = world.pass().await;
    assert_eq!(reason(&retried), ReasonCode::ConfigPropagating);
    assert!(world.hot().contains("gc_grace_secs = 600"));
}

#[tokio::test]
async fn an_unconfirmed_reload_waits_or_rolls() {
    for (on_unconfirmed, rolls) in [("Wait", false), ("Rollout", true)] {
        let mut world = World::new(json!({
            "storage": {"fs": {}},
            "reload": {"onUnconfirmed": on_unconfirmed, "timeoutSeconds": 60},
        }));
        running(&mut world).await;
        world.pod("reg-1", "2026-10-16T09:00:00Z");
        world.spec.global.gc_grace_secs = Some(600);
        world.pass().await;
        world.now += SignedDuration::from_mins(5);
        let late = world.pass().await;
        assert_eq!(reason(&late), ReasonCode::ConfigPropagating);
        assert!(advisories(&late).contains(&ReasonCode::ReloadUnconfirmed));
        let rollout = world
            .objects
            .rollout(&world.owner, "reg")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            rollout.annotation(annotations::RESTARTED_AT).is_some(),
            rolls,
            "{on_unconfirmed}"
        );
        if rolls {
            // The new pod read the file at start.
            world.pod("reg-2", "2026-10-16T11:00:00Z");
            let applied = world.pass().await;
            assert_eq!(
                reason(&applied),
                ReasonCode::Reconciled,
                "{:?}",
                applied.outcome
            );
        }
    }
}

#[tokio::test]
async fn a_pod_whose_watcher_died_is_rolled_at_once() {
    let mut world = World::fs();
    running(&mut world).await;
    world.pod("reg-1", "2026-10-16T09:00:00Z");
    world.log(
        "reg-1",
        "2026-10-16T09:00:01Z".parse().unwrap(),
        "ERROR",
        W,
        "Config watcher failed: unable to read configuration: Watcher error: No file descriptors available (os error 24)",
    );
    world.spec.global.gc_grace_secs = Some(600);
    world.pass().await;
    let report = world.pass().await;
    assert_eq!(reason(&report), ReasonCode::ConfigPropagating);
    let message = &report.outcome.advisories[0].message;
    assert!(message.contains("os error 24"), "{message}");
    let rollout = world
        .objects
        .rollout(&world.owner, "reg")
        .await
        .unwrap()
        .unwrap();
    assert!(rollout.annotation(annotations::RESTARTED_AT).is_some());
}

#[tokio::test]
async fn unreadable_logs_roll_the_pods() {
    let mut world = World::fs();
    running(&mut world).await;
    world.pod("reg-1", "2026-10-16T09:00:00Z");
    world.logs.unreadable("team", "reg-1");
    world.spec.global.gc_grace_secs = Some(600);
    world.pass().await;
    let report = world.pass().await;
    assert!(advisories(&report).contains(&ReasonCode::ReloadUnconfirmed));
    let rollout = world
        .objects
        .rollout(&world.owner, "reg")
        .await
        .unwrap()
        .unwrap();
    assert!(rollout.annotation(annotations::RESTARTED_AT).is_some());
}

#[tokio::test]
async fn a_storage_change_waits_for_its_confirmation() {
    let mut world = World::fs();
    running(&mut world).await;
    let first = world.template_boot_hash().await;
    world.spec.storage.fs.as_mut().unwrap().root_dir = "/elsewhere".into();
    let pending = world.pass().await;
    assert_eq!(reason(&pending), ReasonCode::Reconciled);
    assert!(advisories(&pending).contains(&ReasonCode::StorageChangePending));
    let wanted = pending.status.pending_boot_hash.clone().unwrap();
    let message = &pending.outcome.advisories[0].message;
    assert!(message.contains("fs /data → fs /elsewhere"), "{message}");
    // The pods keep the running layer.
    assert_eq!(world.template_boot_hash().await, first);
    world
        .annotations
        .insert(annotations::CONFIRM_STORAGE.into(), wanted.clone());
    let confirmed = world.pass().await;
    assert_eq!(reason(&confirmed), ReasonCode::Reconciled);
    assert_eq!(world.template_boot_hash().await, wanted);
    assert_eq!(confirmed.status.pending_boot_hash, None);
    assert!(
        confirmed
            .events
            .iter()
            .any(|(r, _)| *r == ReasonCode::ConfigRolledOut)
    );
}

#[tokio::test]
async fn a_startup_key_rolls_without_a_confirmation() {
    let mut world = World::fs();
    running(&mut world).await;
    let first = world.template_boot_hash().await;
    world.spec.server.port = 9000;
    let report = world.pass().await;
    assert_eq!(reason(&report), ReasonCode::Reconciled);
    assert_ne!(world.template_boot_hash().await, first);
    // The previous startup Secret is kept for a rollback.
    assert_eq!(
        world
            .objects
            .names("Secret")
            .iter()
            .filter(|n| n.starts_with("reg-boot-"))
            .count(),
        2
    );
}

#[tokio::test]
async fn only_the_newest_startup_secrets_are_kept() {
    let mut world = World::fs();
    running(&mut world).await;
    for port in [9001, 9002, 9003, 9004, 9005] {
        world.spec.server.port = port;
        world.pass().await;
    }
    let boots = world
        .objects
        .names("Secret")
        .into_iter()
        .filter(|n| n.starts_with("reg-boot-"))
        .count();
    // The running one and two before it.
    assert_eq!(boots, 3);
    assert_eq!(world.status.boot_history.len(), 3);
}

#[tokio::test]
async fn s3_credentials_rotate_without_a_confirmation() {
    let mut world = World::new(json!({"storage": {"s3": {
        "endpoint": "http://s3", "bucket": "b", "region": "us-east-1",
        "credentialsSecretRef": {"name": "s3"},
    }}}));
    world.credentials.insert(
        "team",
        "s3",
        &[("access-key-id", "AK"), ("secret-key", "SK")],
    );
    running(&mut world).await;
    let first = world.template_boot_hash().await;
    world.credentials.insert(
        "team",
        "s3",
        &[("access-key-id", "AK"), ("secret-key", "SK2")],
    );
    let rotated = world.pass().await;
    assert!(advisories(&rotated).contains(&ReasonCode::StorageCredentialsRotated));
    assert!(!advisories(&rotated).contains(&ReasonCode::StorageChangePending));
    assert_ne!(world.template_boot_hash().await, first);
}

#[tokio::test]
async fn secrets_must_exist_and_opt_in() {
    let mut world = World::new(json!({"storage": {"s3": {
        "endpoint": "http://s3", "bucket": "b", "region": "us-east-1",
        "credentialsSecretRef": {"name": "s3"},
    }}}));
    assert_eq!(reason(&world.pass().await), ReasonCode::SecretMissing);
    world.credentials.insert_unlabelled(
        "team",
        "s3",
        &[("access-key-id", "AK"), ("secret-key", "SK")],
    );
    assert_eq!(reason(&world.pass().await), ReasonCode::SecretNotAllowed);
    // Nothing was written.
    assert!(world.objects.names("Deployment").is_empty());
}

#[tokio::test]
async fn an_invalid_spec_writes_nothing() {
    let mut world = World::new(json!({"storage": {"fs": {}},
        "global": {"limits": {"maxBlobSize": "0GiB"}}}));
    let report = world.pass().await;
    assert_eq!(reason(&report), ReasonCode::InvalidSpec);
    assert!(
        report
            .outcome
            .ready
            .unwrap_err()
            .message
            .contains("max_blob_size")
    );
    assert!(world.objects.names("Secret").is_empty());
}

#[tokio::test]
async fn the_token_key_is_generated_once_and_rotated_on_demand() {
    let mut world = World::new(json!({"storage": {"fs": {}},
        "auth": {"tokenService": {"realm": "https://registry.example/token"}}}));
    running(&mut world).await;
    let key = world.objects.secret("reg-token-key", TOKEN_KEY).unwrap();
    assert!(world.hot().contains(&key));
    running(&mut world).await;
    assert_eq!(
        world.objects.secret("reg-token-key", TOKEN_KEY).unwrap(),
        key
    );
    world
        .annotations
        .insert(annotations::ROTATE.into(), "2026-10-16".into());
    let rotated = world.pass().await;
    assert!(
        rotated
            .events
            .iter()
            .any(|(r, _)| *r == ReasonCode::TokenKeyRotated)
    );
    assert_ne!(
        world.objects.secret("reg-token-key", TOKEN_KEY).unwrap(),
        key
    );
    assert_eq!(reason(&rotated), ReasonCode::ConfigPropagating);
}

#[tokio::test]
async fn a_crashing_pod_is_a_refused_startup_layer() {
    let mut world = World::fs();
    world.objects.hold_rollout(true);
    world.pass().await;
    let boot = world.template_boot_hash().await;
    world.objects.set_pods(vec![PodState {
        name: "reg-1".into(),
        restarts: 3,
        waiting: Some("CrashLoopBackOff".into()),
        annotations: BTreeMap::from([(annotations::BOOT_HASH.to_string(), boot)]),
        ..Default::default()
    }]);
    world.log(
        "reg-1",
        "2026-10-16T10:00:05Z".parse().unwrap(),
        "ERROR",
        "angos",
        "Failed to bind to 0.0.0.0:8000: in use",
    );
    let report = world.pass().await;
    assert_eq!(reason(&report), ReasonCode::BootRejected);
    assert!(
        report
            .outcome
            .ready
            .unwrap_err()
            .message
            .contains("Failed to bind")
    );
    world.objects.set_pods(vec![]);
    assert_eq!(reason(&world.pass().await), ReasonCode::WorkloadNotReady);
}

#[tokio::test]
async fn the_cluster_issuer_is_read_for_kubernetes_tokens() {
    let mut world = World::new(json!({"storage": {"fs": {}},
        "auth": {"kubernetes": {"audience": "angos"}}}));
    let report = running(&mut world).await;
    assert!(advisories(&report).contains(&ReasonCode::IssuerDiscoveryNotBound));
    assert!(
        world
            .hot()
            .contains("issuer = \"https://kubernetes.default.svc.cluster.local\"")
    );
    world.issuer = FakeIssuer(None);
    world.status = InstanceStatus::default();
    assert_eq!(reason(&world.pass().await), ReasonCode::KubernetesApiError);
}

#[tokio::test]
async fn status_fields_clear_what_a_pass_unset() {
    let fields = status_fields(&InstanceStatus::default());
    assert_eq!(fields["pendingBootHash"], serde_json::Value::Null);
    assert_eq!(fields["pods"], json!([]));
    assert!(!fields.contains_key("conditions"));
}

#[tokio::test]
async fn pods_on_a_single_writer_volume_roll_only_with_consent() {
    let mut world = World::new(json!({"storage": {"fs": {"volume": {"size": "1Gi"}}}}));
    running(&mut world).await;
    let first = world.template_boot_hash().await;
    world.spec.server.port = 9000;
    let held = world.pass().await;
    assert_eq!(reason(&held), ReasonCode::Reconciled);
    assert!(advisories(&held).contains(&ReasonCode::DisruptiveUpdatePending));
    assert_eq!(world.template_boot_hash().await, first);
    // A hot change still goes through.
    world.spec.global.gc_grace_secs = Some(600);
    world.pass().await;
    assert!(world.hot().contains("gc_grace_secs = 600"));
    world.annotations.insert(
        api::ANNOTATIONS.key(weebo_kit_api_annotations::ALLOW_DISRUPTIVE_UPDATE),
        "true".into(),
    );
    world.pass().await;
    assert_ne!(world.template_boot_hash().await, first);
}
