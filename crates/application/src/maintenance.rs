//! What an `AngosMaintenance` runs (spec 4.3, 6.6): the `Job` of a task —
//! Angos's image and both layers of its instance mounted, its store's
//! volume too — and the rules deciding whether it may run now: the write
//! lock (one `scrub` or `prune` at a time per instance) and the **prune
//! lock** (a prune deletes every namespace no repository declares).

use std::collections::BTreeMap;

use api::LocalInstance;
use api::config::AccessMode;
use api::local_instance::Names;
use api::maintenance::{MaintenanceSpec, RunState, Task};
use api::status::RenderState;
use k8s_openapi::api::batch::v1::{Job, JobSpec};
use k8s_openapi::api::core::v1::{
    Affinity, Container, EmptyDirVolumeSource, EnvVar, PersistentVolumeClaimVolumeSource,
    PodAffinity, PodAffinityTerm, PodSpec, PodTemplateSpec, SecretVolumeSource, Volume,
    VolumeMount,
};
use k8s_openapi::apimachinery::pkg::apis::meta::v1::{LabelSelector, ObjectMeta};
use k8s_openapi::jiff::{SignedDuration, Timestamp};
use weebo_kit_application::workload::{
    OwnedObject, restricted_container_security, restricted_pod_security,
};

use crate::render::paths;
use crate::workload::{ANGOS_UID, boot_secret, selector};

/// The command line of a task (after `-c boot -c hot`).
pub fn args(spec: &MaintenanceSpec) -> Vec<String> {
    let p = &spec.params;
    let mut args: Vec<String> = match spec.task {
        Task::Scrub => vec!["scrub".into()],
        Task::Prune => vec!["prune".into(), "-u".into(), p.upload_timeout.clone()],
        Task::ReconcileReplication => vec!["reconcile".into(), "replication".into()],
        Task::ReconcileScan => vec!["reconcile".into(), "scan".into()],
        Task::ReconcileIndex => vec!["reconcile".into(), "index".into()],
    };
    if p.dry_run {
        args.push("-d".into());
    }
    if spec.task == Task::Scrub && p.delete_unknown {
        args.push("--delete-unknown".into());
    }
    if matches!(spec.task, Task::ReconcileScan | Task::ReconcileIndex) && p.force {
        args.push("--force".into());
    }
    if let (Some(n), Task::Scrub | Task::Prune | Task::ReconcileScan) = (p.concurrency, spec.task) {
        args.push("--concurrency".into());
        args.push(n.to_string());
    }
    args
}

/// Why the instance's store can't be reached by a `Job`: an FS store on an
/// `emptyDir` lives in the server's pod only.
pub fn unreachable_store(instance: &LocalInstance) -> Option<String> {
    let storage = &instance.spec.storage;
    let fs = [
        storage.fs.as_ref(),
        storage.metadata.as_ref().and_then(|m| m.fs.as_ref()),
    ];
    fs.into_iter()
        .flatten()
        .any(|fs| fs.volume.is_none())
        .then(|| {
            "the instance's filesystem store is an emptyDir of its pod: a maintenance Job can't \
             reach it (give it a volume, or use S3)"
                .to_string()
        })
}

/// The `Job` of a run.
pub fn job(
    job_name: &str,
    maintenance: &str,
    namespace: &str,
    spec: &MaintenanceSpec,
    instance: &LocalInstance,
    image: &str,
    boot_hash: &str,
) -> OwnedObject {
    let instance_name = instance.metadata.name.clone().unwrap_or_default();
    let names = Names(&instance_name);
    let ispec = &instance.spec;
    let mut volumes = vec![
        Volume {
            name: "boot".into(),
            secret: Some(SecretVolumeSource {
                secret_name: Some(boot_secret(&instance_name, boot_hash)),
                ..Default::default()
            }),
            ..Default::default()
        },
        Volume {
            name: "hot".into(),
            secret: Some(SecretVolumeSource {
                secret_name: Some(names.config()),
                ..Default::default()
            }),
            ..Default::default()
        },
        Volume {
            name: "tmp".into(),
            empty_dir: Some(EmptyDirVolumeSource::default()),
            ..Default::default()
        },
    ];
    let mount = |name: &str, path: &str, read_only: bool| VolumeMount {
        name: name.into(),
        mount_path: path.into(),
        read_only: Some(read_only),
        ..Default::default()
    };
    let mut mounts = vec![
        mount("boot", paths::BOOT_DIR, true),
        mount("hot", paths::HOT_DIR, true),
        mount("tmp", "/tmp", false),
    ];
    if let Some((volume, mount)) = crate::workload::client_tls_volume(ispec) {
        volumes.push(volume);
        mounts.push(mount);
    }
    let mut beside = false;
    for (volume, fs, created) in [
        ("data", ispec.storage.fs.as_ref(), names.data()),
        (
            "metadata",
            ispec.storage.metadata.as_ref().and_then(|m| m.fs.as_ref()),
            names.metadata(),
        ),
    ] {
        let Some((fs, v)) = fs.and_then(|fs| fs.volume.as_ref().map(|v| (fs, v))) else {
            continue;
        };
        beside |= v.access_mode != AccessMode::ReadWriteMany;
        volumes.push(Volume {
            name: volume.into(),
            persistent_volume_claim: Some(PersistentVolumeClaimVolumeSource {
                claim_name: v.claim_name.clone().unwrap_or(created),
                ..Default::default()
            }),
            ..Default::default()
        });
        mounts.push(mount(volume, &fs.root_dir, false));
    }
    let labels = BTreeMap::from([
        (
            "app.kubernetes.io/name".to_string(),
            "angos-maintenance".to_string(),
        ),
        (
            "app.kubernetes.io/instance".to_string(),
            maintenance.to_string(),
        ),
        (
            "app.kubernetes.io/managed-by".to_string(),
            api::MANAGED_BY.to_string(),
        ),
    ]);
    let job = Job {
        metadata: ObjectMeta {
            name: Some(job_name.into()),
            namespace: Some(namespace.into()),
            labels: Some(labels.clone()),
            ..Default::default()
        },
        spec: Some(JobSpec {
            backoff_limit: Some(0),
            active_deadline_seconds: Some(spec.timeout_seconds),
            ttl_seconds_after_finished: Some(spec.ttl_seconds_after_finished),
            template: PodTemplateSpec {
                metadata: Some(ObjectMeta {
                    labels: Some(labels),
                    ..Default::default()
                }),
                spec: Some(PodSpec {
                    restart_policy: Some("Never".into()),
                    automount_service_account_token: Some(false),
                    image_pull_secrets: (!ispec.image_pull_secrets.is_empty())
                        .then(|| ispec.image_pull_secrets.clone()),
                    security_context: Some(
                        ispec
                            .pod_security_context
                            .clone()
                            .unwrap_or_else(|| restricted_pod_security(ANGOS_UID)),
                    ),
                    // A single-writer claim is mounted by one node: the Job
                    // goes where the instance runs.
                    affinity: beside.then(|| Affinity {
                        pod_affinity: Some(PodAffinity {
                            required_during_scheduling_ignored_during_execution: Some(vec![
                                PodAffinityTerm {
                                    label_selector: Some(LabelSelector {
                                        match_labels: Some(selector(&instance_name)),
                                        ..Default::default()
                                    }),
                                    topology_key: "kubernetes.io/hostname".into(),
                                    ..Default::default()
                                },
                            ]),
                            ..Default::default()
                        }),
                        ..Default::default()
                    }),
                    node_selector: (!ispec.node_selector.is_empty())
                        .then(|| ispec.node_selector.clone()),
                    tolerations: (!ispec.tolerations.is_empty()).then(|| ispec.tolerations.clone()),
                    volumes: Some(volumes),
                    containers: vec![Container {
                        name: "angos".into(),
                        image: Some(image.into()),
                        args: Some(
                            [
                                "-c".to_string(),
                                format!("{}/{}", paths::BOOT_DIR, paths::FILE),
                                "-c".to_string(),
                                format!("{}/{}", paths::HOT_DIR, paths::FILE),
                            ]
                            .into_iter()
                            .chain(args(spec))
                            .collect(),
                        ),
                        env: Some(vec![EnvVar {
                            name: "RUST_LOG".into(),
                            value: Some("info".into()),
                            ..Default::default()
                        }]),
                        resources: spec.resources.clone(),
                        security_context: Some(
                            ispec
                                .security_context
                                .clone()
                                .unwrap_or_else(restricted_container_security),
                        ),
                        volume_mounts: Some(mounts),
                        ..Default::default()
                    }],
                    ..Default::default()
                }),
            },
            ..Default::default()
        }),
        ..Default::default()
    };
    OwnedObject::of(&job, true)
}

/// Why a prune can't run now (spec 6.6), `None` when it may. `aiming` are
/// the repositories bound to the instance or about to be: each must be in
/// its configuration.
pub fn prune_lock(instance: &LocalInstance, aiming: &[String], now: Timestamp) -> Option<String> {
    let status = instance.status.clone().unwrap_or_default();
    if let Some(key) = aiming
        .iter()
        .find(|key| !status.repositories.contains_key(*key))
    {
        return Some(format!(
            "the repository {key} isn't rendered by the instance yet: its data would go"
        ));
    }
    if let Some((key, r)) = status
        .repositories
        .iter()
        .find(|(_, r)| r.state != RenderState::Rendered)
    {
        return Some(format!(
            "the repository {key} isn't in the configuration ({:?}): its data would go",
            r.state
        ));
    }
    let window = SignedDuration::from_hours(i64::from(instance.spec.prune_lock.window_hours));
    if let Some((key, _)) = status.released_repositories.iter().find(|(_, at)| {
        at.parse::<Timestamp>()
            .is_ok_and(|at| now.duration_since(at) < window)
    }) {
        return Some(format!(
            "the repository {key} was released less than {}h ago (pruneLock.windowHours)",
            instance.spec.prune_lock.window_hours
        ));
    }
    if status.config_hash.is_none() || status.applied_config_hash != status.config_hash {
        return Some("the instance's pods don't all run its configuration yet".into());
    }
    None
}

/// Another maintenance of the same instance running a writing task, if any.
pub fn writer_running(
    others: &[api::Maintenance],
    me: &str,
    instance: &weebo_kit_api::BoundInstance,
) -> Option<String> {
    others.iter().find_map(|m| {
        let name = m.metadata.name.clone().unwrap_or_default();
        let status = m.status.as_ref()?;
        let run = status.last_run.as_ref()?;
        (name != me
            && m.spec.task.writes()
            && !run.dry_run
            && run.state == RunState::Running
            && status.common.instance.as_ref() == Some(instance))
        .then_some(name)
    })
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn spec(value: serde_json::Value) -> MaintenanceSpec {
        serde_json::from_value(value).unwrap()
    }

    #[test]
    fn tasks_run_dry_unless_told() {
        assert_eq!(
            args(&spec(json!({"task": "Prune"}))),
            ["prune", "-u", "24h", "-d"]
        );
        assert_eq!(
            args(&spec(
                json!({"task": "Scrub", "params": {"dryRun": false, "deleteUnknown": true, "concurrency": 4}})
            )),
            ["scrub", "--delete-unknown", "--concurrency", "4"]
        );
        assert_eq!(
            args(&spec(
                json!({"task": "ReconcileIndex", "params": {"force": true}})
            )),
            ["reconcile", "index", "-d", "--force"]
        );
    }

    #[test]
    fn the_job_mounts_the_instance_s_layers_and_volume() {
        let instance = LocalInstance::new(
            "reg",
            serde_json::from_value(json!({"storage": {"fs": {"volume": {"size": "1Gi"}}}}))
                .unwrap(),
        );
        let job = job(
            "prune-1",
            "prune",
            "team",
            &spec(json!({"task": "Prune"})),
            &instance,
            "ghcr.io/project-angos/angos:1.12.3",
            "0123456789",
        );
        assert!(job.create_only);
        let pod = &job.manifest["spec"]["template"]["spec"];
        assert_eq!(job.manifest["spec"]["backoffLimit"], 0);
        assert_eq!(pod["restartPolicy"], "Never");
        assert_eq!(
            pod["containers"][0]["args"],
            json!([
                "-c",
                "/etc/angos/boot/config.toml",
                "-c",
                "/etc/angos/hot/config.toml",
                "prune",
                "-u",
                "24h",
                "-d"
            ])
        );
        let volumes = pod["volumes"].as_array().unwrap();
        assert!(
            volumes
                .iter()
                .any(|v| v["secret"]["secretName"] == "reg-boot-01234567")
        );
        assert!(
            volumes
                .iter()
                .any(|v| v["persistentVolumeClaim"]["claimName"] == "reg-data")
        );
        assert!(pod["affinity"]["podAffinity"].is_object());
        let without_volume = LocalInstance::new(
            "reg",
            serde_json::from_value(json!({"storage": {"fs": {}}})).unwrap(),
        );
        assert!(unreachable_store(&without_volume).is_some());
        assert!(unreachable_store(&instance).is_none());
    }

    #[test]
    fn a_prune_waits_for_every_repository_and_the_window() {
        let now: Timestamp = "2026-10-16T12:00:00Z".parse().unwrap();
        let mut instance = LocalInstance::new("reg", Default::default());
        let mut status = api::status::InstanceStatus {
            config_hash: Some("h".into()),
            applied_config_hash: Some("h".into()),
            ..Default::default()
        };
        instance.status = Some(status.clone());
        assert_eq!(prune_lock(&instance, &[], now), None);
        assert!(
            prune_lock(&instance, &["team/new".to_string()], now)
                .unwrap()
                .contains("team/new")
        );
        status
            .released_repositories
            .insert("team/old".into(), "2026-10-16T02:00:00Z".into());
        instance.status = Some(status.clone());
        assert!(
            prune_lock(&instance, &[], now)
                .unwrap()
                .contains("released")
        );
        status.released_repositories.clear();
        status.repositories.insert(
            "team/app".into(),
            api::status::RepositoryRender {
                prefix: "app".into(),
                state: RenderState::SecretMissing,
                message: None,
                conflict_with: None,
                generation: None,
                applied_generation: None,
            },
        );
        instance.status = Some(status.clone());
        assert!(
            prune_lock(&instance, &[], now)
                .unwrap()
                .contains("team/app")
        );
        status.repositories.clear();
        status.applied_config_hash = Some("old".into());
        instance.status = Some(status);
        assert!(prune_lock(&instance, &[], now).is_some());
    }
}
