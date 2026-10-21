//! The Kubernetes objects of an `AngosLocalInstance` (spec 2.3, 4.1, 6.5),
//! built from its spec and its two rendered files — pure, no client:
//!
//! - `<name>-boot-<hash8>`: the startup layer, an immutable `Secret` named
//!   by its hash (a pod never sees it change); the pod template carries
//!   the hash (`angos.weebo.io/boot-hash`): it, and it alone, rolls the
//!   pods;
//! - `<name>-config`: the hot layer, rewritten in place, reloaded by Angos;
//! - the `Deployment` (`angos -c <boot> -c <hot> server`), its
//!   `ServiceAccount`, `Service`, claims, `NetworkPolicy` and — from two
//!   pods — `PodDisruptionBudget`;
//! - with `worker.enabled`, `<name>-worker` (`angos … worker`): the same
//!   files and mounts, no port nor probe (spec 6.3).

use std::collections::BTreeMap;

use api::config::{AccessMode, FsStorage};
use api::local_instance::{LocalInstanceSpec, Names, Worker};
use k8s_openapi::ByteString;
use k8s_openapi::api::apps::v1::{
    Deployment, DeploymentSpec, DeploymentStrategy, RollingUpdateDeployment,
};
use k8s_openapi::api::core::v1::{
    ConfigMapProjection, Container, ContainerPort, EmptyDirVolumeSource, EnvVar, HTTPGetAction,
    KeyToPath, Lifecycle, LifecycleHandler, PersistentVolumeClaim, PersistentVolumeClaimSpec,
    PersistentVolumeClaimVolumeSource, PodAffinity, PodAffinityTerm, PodSpec, PodTemplateSpec,
    Probe, ProjectedVolumeSource, ResourceRequirements, Secret, SecretProjection,
    SecretVolumeSource, Service, ServiceAccount, ServiceAccountTokenProjection, ServicePort,
    ServiceSpec, SleepAction, Volume, VolumeMount, VolumeProjection, VolumeResourceRequirements,
};
use k8s_openapi::api::policy::v1::{PodDisruptionBudget, PodDisruptionBudgetSpec};
use k8s_openapi::apimachinery::pkg::api::resource::Quantity;
use k8s_openapi::apimachinery::pkg::apis::meta::v1::{LabelSelector, ObjectMeta};
use k8s_openapi::apimachinery::pkg::util::intstr::IntOrString;
use weebo_kit_application::workload::{
    OwnedObject, PolicyRules, network_policy, restricted_container_security,
    restricted_pod_security,
};

use crate::render::paths;

/// The image's user (`USER 65534:65534`).
pub const ANGOS_UID: i64 = 65534;

/// The template annotation of the image tag: an image change rolls like a
/// startup layer.
pub const IMAGE_ANNOTATION: &str = "angos.weebo.io/image";

/// The template annotation of the hot layer, in `reload.mode: Rollout`.
pub const CONFIG_HASH_ANNOTATION: &str = "angos.weebo.io/config-hash";

/// What the objects are built from.
#[derive(Debug, Clone)]
pub struct WorkloadInput<'a> {
    pub name: &'a str,
    pub namespace: &'a str,
    pub spec: &'a LocalInstanceSpec,
    /// The image, `repository:tag`.
    pub image: &'a str,
    /// The startup layer to run, and its hash.
    pub boot_file: &'a str,
    pub boot_hash: &'a str,
    /// The hot layer to write in `<name>-config`.
    pub hot_file: &'a str,
    /// Set in `reload.mode: Rollout`: the hot layer's hash on the template.
    pub rollout_hash: Option<&'a str>,
    /// The server's startup layer and image when they wait for the
    /// worker's (spec 6.3); unset, `boot_hash` and `image`.
    pub server_hold: Option<(&'a str, &'a str)>,
    /// The server's pods: `spec.replicas`, or one until the store's
    /// conditional writes are known atomic.
    pub replicas: i32,
    /// The worker to run (`worker.enabled`).
    pub worker: Option<&'a Worker>,
    /// The worker is removed (disabled, and the server took the jobs
    /// back); otherwise a disabled worker is left as it is.
    pub remove_worker: bool,
}

impl WorkloadInput<'_> {
    /// The server's startup layer hash and image.
    pub fn server_boot(&self) -> (&str, &str) {
        self.server_hold.unwrap_or((self.boot_hash, self.image))
    }
}

/// The labels of everything an instance owns.
pub fn labels(name: &str, component: &str) -> BTreeMap<String, String> {
    BTreeMap::from([
        ("app.kubernetes.io/name".into(), "angos".into()),
        ("app.kubernetes.io/instance".into(), name.into()),
        ("app.kubernetes.io/component".into(), component.into()),
        (
            "app.kubernetes.io/managed-by".into(),
            api::MANAGED_BY.into(),
        ),
    ])
}

/// The labels selecting the instance's pods, server and worker: those
/// that load the configuration. The server's `Deployment` selects with
/// them too (a selector is immutable; the ReplicaSets' owner references
/// keep the two Deployments apart).
pub fn selector(name: &str) -> BTreeMap<String, String> {
    BTreeMap::from([
        ("app.kubernetes.io/name".into(), "angos".into()),
        ("app.kubernetes.io/instance".into(), name.into()),
    ])
}

/// The labels of one role's pods: what the `Service`, the
/// `PodDisruptionBudget` and the worker's `Deployment` select.
pub fn role_selector(name: &str, role: Role) -> BTreeMap<String, String> {
    let mut labels = selector(name);
    labels.insert("app.kubernetes.io/component".into(), role.as_str().into());
    labels
}

/// The instance's two kinds of pods.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    Server,
    Worker,
}

impl Role {
    pub fn as_str(self) -> &'static str {
        match self {
            Role::Server => "server",
            Role::Worker => "worker",
        }
    }
}

/// The name of the startup `Secret` of `boot_hash`.
pub fn boot_secret(name: &str, boot_hash: &str) -> String {
    format!(
        "{}-{}",
        Names(name).boot_prefix(),
        &boot_hash[..8.min(boot_hash.len())]
    )
}

/// The image of `spec`: its repository and tag, defaults filled.
pub fn image(spec: &LocalInstanceSpec, default_image: &str) -> String {
    let (default_repository, default_tag) = default_image
        .rsplit_once(':')
        .unwrap_or((default_image, api::angos::VERSION));
    format!(
        "{}:{}",
        spec.image
            .repository
            .as_deref()
            .unwrap_or(default_repository),
        spec.image.tag.as_deref().unwrap_or(default_tag)
    )
}

/// The tag of an image reference.
pub fn tag(image: &str) -> &str {
    let name = image.split('@').next().unwrap_or(image);
    match name.rsplit_once(':') {
        Some((_, tag)) if !tag.contains('/') => tag,
        _ => "latest",
    }
}

fn meta(input: &WorkloadInput<'_>, name: String, component: &str) -> ObjectMeta {
    ObjectMeta {
        name: Some(name),
        namespace: Some(input.namespace.into()),
        labels: Some(labels(input.name, component)),
        ..Default::default()
    }
}

fn some_map(map: &BTreeMap<String, String>) -> Option<BTreeMap<String, String>> {
    (!map.is_empty()).then(|| map.clone())
}

fn file_secret(input: &WorkloadInput<'_>, name: String, component: &str, file: &str) -> Secret {
    let mut metadata = meta(input, name, component);
    // Read back by the operator: it opts in like any Secret it reads.
    metadata
        .labels
        .get_or_insert_default()
        .insert(api::CREDENTIAL_LABEL.into(), "true".into());
    Secret {
        metadata,
        data: Some(BTreeMap::from([(
            paths::FILE.into(),
            ByteString(file.as_bytes().to_vec()),
        )])),
        ..Default::default()
    }
}

fn default_resources() -> ResourceRequirements {
    ResourceRequirements {
        requests: Some(BTreeMap::from([
            ("cpu".into(), Quantity("100m".into())),
            ("memory".into(), Quantity("256Mi".into())),
        ])),
        limits: Some(BTreeMap::from([("memory".into(), Quantity("1Gi".into()))])),
        ..Default::default()
    }
}

fn probe(path: &str, scheme: &str, period: i32, failures: i32) -> Probe {
    Probe {
        http_get: Some(HTTPGetAction {
            path: Some(path.into()),
            port: IntOrString::String("http".into()),
            scheme: Some(scheme.into()),
            ..Default::default()
        }),
        period_seconds: Some(period),
        failure_threshold: Some(failures),
        ..Default::default()
    }
}

/// The claim of an FS store: the spec's, or the operator's (`create`).
fn claim(fs: &FsStorage, created: String) -> Option<(String, bool)> {
    let volume = fs.volume.as_ref()?;
    Some(match &volume.claim_name {
        Some(name) => (name.clone(), false),
        None => (created, true),
    })
}

/// The client TLS files of the services the spec calls (scanner,
/// webhooks, OIDC providers), one directory per slot: a projected volume
/// and its mount, or none.
pub fn client_tls_volume(spec: &LocalInstanceSpec) -> Option<(Volume, VolumeMount)> {
    use crate::render::tls_files;
    let slots = crate::render::client_tls_slots(spec);
    if slots.is_empty() {
        return None;
    }
    let item = |key: &str, slot: &str, file: &str| KeyToPath {
        key: key.into(),
        path: format!("{slot}/{file}"),
        ..Default::default()
    };
    let mut sources = Vec::new();
    for (slot, tls) in &slots {
        if let Some(ca) = &tls.ca {
            match (&ca.config_map_key_ref, &ca.secret_key_ref) {
                (Some(r), _) => sources.push(VolumeProjection {
                    config_map: Some(ConfigMapProjection {
                        name: r.name.clone(),
                        items: Some(vec![item(&r.key, slot, tls_files::CA)]),
                        ..Default::default()
                    }),
                    ..Default::default()
                }),
                (None, Some(r)) => sources.push(VolumeProjection {
                    secret: Some(SecretProjection {
                        name: r.name.clone(),
                        items: Some(vec![item(&r.key, slot, tls_files::CA)]),
                        ..Default::default()
                    }),
                    ..Default::default()
                }),
                (None, None) => {}
            }
        }
        if let Some(name) = &tls.client_certificate_secret_name {
            sources.push(VolumeProjection {
                secret: Some(SecretProjection {
                    name: name.clone(),
                    items: Some(vec![
                        item("tls.crt", slot, tls_files::CERTIFICATE),
                        item("tls.key", slot, tls_files::KEY),
                    ]),
                    ..Default::default()
                }),
                ..Default::default()
            });
        }
    }
    Some((
        Volume {
            name: "client-tls".into(),
            projected: Some(ProjectedVolumeSource {
                sources: Some(sources),
                ..Default::default()
            }),
            ..Default::default()
        },
        VolumeMount {
            name: "client-tls".into(),
            mount_path: paths::CLIENT_TLS_DIR.into(),
            read_only: Some(true),
            ..Default::default()
        },
    ))
}

/// Whether the pods share a single-writer volume: a surge pod would wait
/// for it, they are recreated instead.
pub fn single_writer(spec: &LocalInstanceSpec) -> bool {
    [
        spec.storage.fs.as_ref(),
        spec.storage.metadata.as_ref().and_then(|m| m.fs.as_ref()),
    ]
    .into_iter()
    .flatten()
    .filter_map(|fs| fs.volume.as_ref())
    .any(|v| v.access_mode != AccessMode::ReadWriteMany)
}

fn rust_log(spec: &LocalInstanceSpec) -> String {
    format!(
        "{},{}=info,{}=warn",
        spec.log_level,
        crate::reload::WATCHER_TARGET,
        crate::reload::JOB_STORE_TARGET
    )
}

fn service_account_name(input: &WorkloadInput<'_>) -> String {
    input
        .spec
        .service_account
        .name
        .clone()
        .unwrap_or_else(|| input.name.to_string())
}

fn deployment(input: &WorkloadInput<'_>, role: Role) -> Deployment {
    let spec = input.spec;
    let names = Names(input.name);
    let tls = spec.server.tls.is_some();
    let (boot_hash, image) = match role {
        Role::Server => input.server_boot(),
        Role::Worker => (input.boot_hash, input.image),
    };
    let worker = input.worker.filter(|_| role == Role::Worker);
    let mut volumes = vec![
        Volume {
            name: "boot".into(),
            secret: Some(SecretVolumeSource {
                secret_name: Some(boot_secret(input.name, boot_hash)),
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
    if let Some(server_tls) = &spec.server.tls {
        volumes.push(Volume {
            name: "tls".into(),
            secret: Some(SecretVolumeSource {
                secret_name: Some(server_tls.secret_name.clone()),
                ..Default::default()
            }),
            ..Default::default()
        });
        mounts.push(mount("tls", paths::TLS_DIR, true));
        if let Some(ca) = &server_tls.client_ca {
            volumes.push(Volume {
                name: "client-ca".into(),
                secret: Some(SecretVolumeSource {
                    secret_name: Some(ca.name.clone()),
                    items: Some(vec![KeyToPath {
                        key: ca.key.clone(),
                        path: paths::CLIENT_CA_FILE.into(),
                        ..Default::default()
                    }]),
                    ..Default::default()
                }),
                ..Default::default()
            });
            mounts.push(mount("client-ca", paths::CLIENT_CA_DIR, true));
        }
    }
    if spec.auth.kubernetes.is_some() {
        // The account's own token (API server audience) and the cluster's
        // CA: Angos reads the JWKS with them.
        volumes.push(Volume {
            name: "kube".into(),
            projected: Some(ProjectedVolumeSource {
                sources: Some(vec![
                    VolumeProjection {
                        service_account_token: Some(ServiceAccountTokenProjection {
                            path: "token".into(),
                            expiration_seconds: Some(3600),
                            ..Default::default()
                        }),
                        ..Default::default()
                    },
                    VolumeProjection {
                        config_map: Some(ConfigMapProjection {
                            name: "kube-root-ca.crt".into(),
                            items: Some(vec![KeyToPath {
                                key: "ca.crt".into(),
                                path: "ca.crt".into(),
                                ..Default::default()
                            }]),
                            ..Default::default()
                        }),
                        ..Default::default()
                    },
                ]),
                ..Default::default()
            }),
            ..Default::default()
        });
        mounts.push(mount("kube", paths::KUBE_DIR, true));
    }
    if let Some((volume, mount)) = client_tls_volume(spec) {
        volumes.push(volume);
        mounts.push(mount);
    }
    for (volume, fs, created) in [
        ("data", spec.storage.fs.as_ref(), names.data()),
        (
            "metadata",
            spec.storage.metadata.as_ref().and_then(|m| m.fs.as_ref()),
            names.metadata(),
        ),
    ] {
        let Some(fs) = fs else { continue };
        volumes.push(Volume {
            name: volume.into(),
            persistent_volume_claim: claim(fs, created).map(|(claim_name, _)| {
                PersistentVolumeClaimVolumeSource {
                    claim_name,
                    ..Default::default()
                }
            }),
            empty_dir: fs.volume.is_none().then(EmptyDirVolumeSource::default),
            ..Default::default()
        });
        mounts.push(mount(volume, &fs.root_dir, false));
    }
    volumes.extend(spec.extra_volumes.iter().cloned());
    mounts.extend(spec.extra_volume_mounts.iter().cloned());

    let mut env = vec![EnvVar {
        name: "RUST_LOG".into(),
        value: Some(rust_log(spec)),
        ..Default::default()
    }];
    env.extend(spec.env.iter().filter(|e| e.name != "RUST_LOG").cloned());

    let mut annotations = spec.pod_annotations.clone();
    annotations.insert(api::annotations::BOOT_HASH.into(), boot_hash.into());
    annotations.insert(IMAGE_ANNOTATION.into(), image.into());
    if let Some(hash) = input.rollout_hash {
        annotations.insert(CONFIG_HASH_ANNOTATION.into(), hash.into());
    }
    let mut pod_labels = selector(input.name);
    pod_labels.extend(spec.pod_labels.clone());
    pod_labels.insert("app.kubernetes.io/component".into(), role.as_str().into());

    // The schema caps it; an older object past the cap saturates instead
    // of wrapping into a negative grace period.
    let drain = i64::try_from(spec.global.shutdown_drain_secs.unwrap_or(30)).unwrap_or(i64::MAX);
    let scheme = if tls { "HTTPS" } else { "HTTP" };
    let strategy = if single_writer(spec) && role == Role::Server {
        DeploymentStrategy {
            type_: Some("Recreate".into()),
            ..Default::default()
        }
    } else {
        DeploymentStrategy {
            type_: Some("RollingUpdate".into()),
            rolling_update: Some(RollingUpdateDeployment {
                max_unavailable: Some(IntOrString::Int(0)),
                max_surge: Some(IntOrString::Int(1)),
            }),
        }
    };
    let mut args = vec![
        "-c".into(),
        format!("{}/{}", paths::BOOT_DIR, paths::FILE),
        "-c".into(),
        format!("{}/{}", paths::HOT_DIR, paths::FILE),
        role.as_str().into(),
    ];
    let mut container = Container {
        name: "angos".into(),
        image: Some(image.into()),
        image_pull_policy: Some(spec.image.pull_policy.unwrap_or_default().as_str().into()),
        env: Some(env),
        env_from: (!spec.env_from.is_empty()).then(|| spec.env_from.clone()),
        resources: Some(spec.resources.clone().unwrap_or_else(default_resources)),
        security_context: Some(
            spec.security_context
                .clone()
                .unwrap_or_else(restricted_container_security),
        ),
        volume_mounts: Some(mounts),
        ..Default::default()
    };
    let (name, replicas, selector, affinity) = match worker {
        None => {
            container.ports = Some(vec![ContainerPort {
                name: Some("http".into()),
                container_port: spec.server.port.into(),
                protocol: Some("TCP".into()),
                ..Default::default()
            }]);
            // A slow first S3 listing: up to 5 minutes to start.
            container.startup_probe = Some(probe("/readyz", scheme, 5, 60));
            container.readiness_probe = Some(probe("/readyz", scheme, 10, 3));
            container.liveness_probe = Some(probe("/healthz", scheme, 10, 6));
            // The image has no shell: the kubelet sleeps, the endpoints
            // drain.
            container.lifecycle = Some(Lifecycle {
                pre_stop: Some(LifecycleHandler {
                    sleep: Some(SleepAction { seconds: 5 }),
                    ..Default::default()
                }),
                ..Default::default()
            });
            (
                names.workload(),
                input.replicas,
                selector(input.name),
                spec.affinity.clone(),
            )
        }
        Some(worker) => {
            // No port: the process exiting is its only health signal.
            for queue in &worker.queues {
                args.extend(["--queue".to_string(), queue.as_str().to_string()]);
            }
            if let Some(secs) = worker.poll_interval_seconds {
                args.extend(["--poll-interval".to_string(), format!("{secs}s")]);
            }
            if let Some(resources) = &worker.resources {
                container.resources = Some(resources.clone());
            }
            // A single-writer claim is mounted by one node: the worker goes
            // where the server runs.
            let affinity = if single_writer(spec) {
                let mut affinity = spec.affinity.clone().unwrap_or_default();
                affinity
                    .pod_affinity
                    .get_or_insert_with(PodAffinity::default)
                    .required_during_scheduling_ignored_during_execution
                    .get_or_insert_default()
                    .push(PodAffinityTerm {
                        label_selector: Some(LabelSelector {
                            match_labels: Some(role_selector(input.name, Role::Server)),
                            ..Default::default()
                        }),
                        topology_key: "kubernetes.io/hostname".into(),
                        ..Default::default()
                    });
                Some(affinity)
            } else {
                spec.affinity.clone()
            };
            (
                names.worker(),
                worker.replicas,
                role_selector(input.name, Role::Worker),
                affinity,
            )
        }
    };
    container.args = Some(args);
    Deployment {
        metadata: meta(input, name, role.as_str()),
        spec: Some(DeploymentSpec {
            replicas: Some(replicas),
            selector: LabelSelector {
                match_labels: Some(selector),
                ..Default::default()
            },
            strategy: Some(strategy),
            progress_deadline_seconds: Some(600),
            template: PodTemplateSpec {
                metadata: Some(ObjectMeta {
                    labels: Some(pod_labels),
                    annotations: Some(annotations),
                    ..Default::default()
                }),
                spec: Some(PodSpec {
                    service_account_name: Some(service_account_name(input)),
                    automount_service_account_token: Some(false),
                    image_pull_secrets: (!spec.image_pull_secrets.is_empty())
                        .then(|| spec.image_pull_secrets.clone()),
                    security_context: Some(
                        spec.pod_security_context
                            .clone()
                            .unwrap_or_else(|| restricted_pod_security(ANGOS_UID)),
                    ),
                    node_selector: some_map(&spec.node_selector),
                    tolerations: (!spec.tolerations.is_empty()).then(|| spec.tolerations.clone()),
                    affinity,
                    priority_class_name: spec.priority_class_name.clone(),
                    termination_grace_period_seconds: Some(
                        spec.termination_grace_period_seconds
                            .unwrap_or(drain.saturating_mul(3).saturating_add(10)),
                    ),
                    volumes: Some(volumes),
                    containers: vec![container],
                    ..Default::default()
                }),
            },
            ..Default::default()
        }),
        ..Default::default()
    }
}

/// Every object of the instance, in the order they are applied (what a pod
/// mounts before the pods).
pub fn objects(input: &WorkloadInput<'_>) -> Vec<OwnedObject> {
    let spec = input.spec;
    let names = Names(input.name);
    let mut out = Vec::new();
    // The startup layer: created once under its hash, never rewritten.
    let mut boot = file_secret(
        input,
        boot_secret(input.name, input.boot_hash),
        "boot",
        input.boot_file,
    );
    boot.immutable = Some(true);
    out.push(OwnedObject::of(&boot, true));
    out.push(OwnedObject::of(
        &file_secret(input, names.config(), "config", input.hot_file),
        false,
    ));
    if spec.service_account.create {
        let mut metadata = meta(input, service_account_name(input), "server");
        metadata.annotations = some_map(&spec.service_account.annotations);
        out.push(OwnedObject::of(
            &ServiceAccount {
                metadata,
                automount_service_account_token: Some(false),
                ..Default::default()
            },
            false,
        ));
    }
    for (fs, created) in [
        (spec.storage.fs.as_ref(), names.data()),
        (
            spec.storage.metadata.as_ref().and_then(|m| m.fs.as_ref()),
            names.metadata(),
        ),
    ] {
        let Some(fs) = fs else { continue };
        let (Some((claim_name, true)), Some(volume)) = (claim(fs, created), fs.volume.as_ref())
        else {
            continue;
        };
        out.push(OwnedObject::of(
            &PersistentVolumeClaim {
                metadata: meta(input, claim_name, "data"),
                spec: Some(PersistentVolumeClaimSpec {
                    access_modes: Some(vec![volume.access_mode.as_str().into()]),
                    storage_class_name: volume.storage_class.clone(),
                    resources: Some(VolumeResourceRequirements {
                        requests: Some(BTreeMap::from([(
                            "storage".into(),
                            Quantity(volume.size.clone().unwrap_or_default()),
                        )])),
                        ..Default::default()
                    }),
                    ..Default::default()
                }),
                ..Default::default()
            },
            // Never resized: a claim's spec is the storage class's to grow.
            true,
        ));
    }
    let mut service_meta = meta(input, names.workload(), "server");
    service_meta.annotations = some_map(&spec.service.annotations);
    out.push(OwnedObject::of(
        &Service {
            metadata: service_meta,
            spec: Some(ServiceSpec {
                type_: Some(spec.service.r#type.as_str().into()),
                selector: Some(role_selector(input.name, Role::Server)),
                ports: Some(vec![ServicePort {
                    name: Some("http".into()),
                    port: spec.service.port,
                    target_port: Some(IntOrString::String("http".into())),
                    protocol: Some("TCP".into()),
                    ..Default::default()
                }]),
                ..Default::default()
            }),
            ..Default::default()
        },
        false,
    ));
    if input.worker.is_some() {
        out.push(OwnedObject::of(&deployment(input, Role::Worker), false));
    }
    out.push(OwnedObject::of(&deployment(input, Role::Server), false));
    if input.replicas > 1 {
        out.push(OwnedObject::of(
            &PodDisruptionBudget {
                metadata: meta(input, names.workload(), "server"),
                spec: Some(PodDisruptionBudgetSpec {
                    min_available: Some(IntOrString::Int(1)),
                    selector: Some(LabelSelector {
                        match_labels: Some(role_selector(input.name, Role::Server)),
                        ..Default::default()
                    }),
                    ..Default::default()
                }),
                ..Default::default()
            },
            false,
        ));
    }
    if let Some(policy) = &spec.network_policy {
        out.push(OwnedObject::of(
            &network_policy(
                meta(input, names.workload(), "server"),
                &PolicyRules {
                    pod_labels: selector(input.name),
                    port: "http",
                    ingress_from: &policy.ingress_from,
                    egress_to: &policy.egress_to,
                    // The operator never calls Angos.
                    operator: None,
                },
            ),
            false,
        ));
    }
    out
}

/// The objects a spec no longer renders, which the operator removes:
/// `(apiVersion, kind, name)`. Claims are never removed.
pub fn removed(input: &WorkloadInput<'_>) -> Vec<(&'static str, &'static str, String)> {
    let mut out = Vec::new();
    let names = Names(input.name);
    if input.spec.network_policy.is_none() {
        out.push(("networking.k8s.io/v1", "NetworkPolicy", names.workload()));
    }
    if input.replicas < 2 {
        out.push(("policy/v1", "PodDisruptionBudget", names.workload()));
    }
    if input.worker.is_none() && input.remove_worker {
        out.push(("apps/v1", "Deployment", names.worker()));
    }
    out
}

/// The objects of `spec` as the webhook sees them (no file rendered yet):
/// what the pods may be is decided by the spec alone.
pub fn sketch(
    name: &str,
    namespace: &str,
    spec: &LocalInstanceSpec,
    image: &str,
) -> Vec<OwnedObject> {
    objects(&WorkloadInput {
        name,
        namespace,
        spec,
        image,
        boot_file: "",
        boot_hash: "00000000",
        hot_file: "",
        rollout_hash: None,
        server_hold: None,
        replicas: spec.replicas,
        worker: spec.worker.as_ref().filter(|w| w.enabled),
        remove_worker: false,
    })
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn spec(value: serde_json::Value) -> LocalInstanceSpec {
        serde_json::from_value(value).unwrap()
    }

    fn input<'a>(spec: &'a LocalInstanceSpec) -> WorkloadInput<'a> {
        WorkloadInput {
            name: "reg",
            namespace: "team",
            spec,
            image: "ghcr.io/project-angos/angos:1.12.3",
            boot_file: "[server]\n",
            boot_hash: "0123456789abcdef",
            hot_file: "[global]\n",
            rollout_hash: None,
            server_hold: None,
            replicas: spec.replicas,
            worker: spec.worker.as_ref().filter(|w| w.enabled),
            remove_worker: true,
        }
    }

    fn kinds(objects: &[OwnedObject]) -> Vec<String> {
        objects
            .iter()
            .map(|o| format!("{}/{}", o.kind, o.name))
            .collect()
    }

    #[test]
    fn a_minimal_instance_is_two_layers_an_account_a_service_and_a_deployment() {
        let s = spec(json!({"storage": {"fs": {}}}));
        let objects = objects(&input(&s));
        assert_eq!(
            kinds(&objects),
            [
                "Secret/reg-boot-01234567",
                "Secret/reg-config",
                "ServiceAccount/reg",
                "Service/reg",
                "Deployment/reg",
            ]
        );
        assert!(objects[0].create_only);
        assert_eq!(objects[0].manifest["immutable"], true);
        assert_eq!(
            objects[0].manifest["metadata"]["labels"][api::CREDENTIAL_LABEL],
            "true"
        );
        let deployment = &objects[4].manifest["spec"];
        let pod = &deployment["template"];
        assert_eq!(
            pod["metadata"]["annotations"][api::annotations::BOOT_HASH],
            "0123456789abcdef"
        );
        let container = &pod["spec"]["containers"][0];
        assert_eq!(
            container["args"],
            json!([
                "-c",
                "/etc/angos/boot/config.toml",
                "-c",
                "/etc/angos/hot/config.toml",
                "server"
            ])
        );
        assert_eq!(
            container["env"][0],
            json!({"name": "RUST_LOG", "value": "info,angos::configuration::watcher=info,angos::jobs::store=warn"})
        );
        assert_eq!(container["readinessProbe"]["httpGet"]["path"], "/readyz");
        assert_eq!(container["lifecycle"]["preStop"]["sleep"]["seconds"], 5);
        assert_eq!(pod["spec"]["securityContext"]["runAsUser"], 65534);
        assert_eq!(pod["spec"]["terminationGracePeriodSeconds"], 100);
        assert_eq!(deployment["strategy"]["rollingUpdate"]["maxUnavailable"], 0);
        // The FS store without a volume: an emptyDir at its root.
        let volumes = pod["spec"]["volumes"].as_array().unwrap();
        assert!(
            volumes
                .iter()
                .any(|v| v["name"] == "data" && v.get("emptyDir").is_some())
        );
    }

    #[test]
    fn a_full_instance_mounts_what_its_configuration_names() {
        let s = spec(json!({
            "storage": {"fs": {"volume": {"size": "10Gi"}}},
            "server": {"port": 9000, "tls": {"secretName": "tls", "clientCA": {"name": "ca", "key": "ca.pem"}}},
            "auth": {"kubernetes": {"audience": "angos"}},
            "networkPolicy": {"ingressFrom": [{"podSelector": {}}]},
            "logLevel": "warn",
        }));
        let mut i = input(&s);
        i.rollout_hash = Some("h");
        let objects = objects(&i);
        assert_eq!(
            kinds(&objects),
            [
                "Secret/reg-boot-01234567",
                "Secret/reg-config",
                "ServiceAccount/reg",
                "PersistentVolumeClaim/reg-data",
                "Service/reg",
                "Deployment/reg",
                "NetworkPolicy/reg",
            ]
        );
        assert!(objects[3].create_only);
        assert_eq!(
            removed(&i),
            [
                ("policy/v1", "PodDisruptionBudget", "reg".to_string()),
                ("apps/v1", "Deployment", "reg-worker".to_string()),
            ]
        );
        let pod = &objects[5].manifest["spec"]["template"];
        assert_eq!(pod["metadata"]["annotations"][CONFIG_HASH_ANNOTATION], "h");
        let container = &pod["spec"]["containers"][0];
        assert_eq!(container["ports"][0]["containerPort"], 9000);
        assert_eq!(container["readinessProbe"]["httpGet"]["scheme"], "HTTPS");
        let mounts: Vec<&str> = container["volumeMounts"]
            .as_array()
            .unwrap()
            .iter()
            .map(|m| m["mountPath"].as_str().unwrap())
            .collect();
        for path in [
            "/etc/angos/tls",
            "/etc/angos/client-ca",
            "/var/run/secrets/angos/kube",
            "/data",
        ] {
            assert!(mounts.contains(&path), "{path} in {mounts:?}");
        }
        // A ReadWriteOnce claim: the pods are recreated, not surged.
        assert_eq!(objects[5].manifest["spec"]["strategy"]["type"], "Recreate");
        assert_eq!(
            container["env"][0]["value"],
            "warn,angos::configuration::watcher=info,angos::jobs::store=warn"
        );
    }

    #[test]
    fn a_worker_runs_beside_the_server_and_replicas_get_a_budget() {
        let s = spec(json!({
            "storage": {"s3": {"endpoint": "https://s3", "bucket": "b", "region": "r",
                               "credentialsSecretRef": {"name": "s3"}}},
            "replicas": 3,
            "worker": {"replicas": 2, "queues": ["cache", "index"], "pollIntervalSeconds": 5},
        }));
        let objects = objects(&input(&s));
        assert_eq!(
            kinds(&objects),
            [
                "Secret/reg-boot-01234567",
                "Secret/reg-config",
                "ServiceAccount/reg",
                "Service/reg",
                "Deployment/reg-worker",
                "Deployment/reg",
                "PodDisruptionBudget/reg",
            ]
        );
        // The Service and the budget select the server's pods only.
        let server = json!({"app.kubernetes.io/name": "angos", "app.kubernetes.io/instance": "reg",
                            "app.kubernetes.io/component": "server"});
        assert_eq!(objects[3].manifest["spec"]["selector"], server);
        assert_eq!(
            objects[6].manifest["spec"]["selector"]["matchLabels"],
            server
        );
        assert_eq!(objects[6].manifest["spec"]["minAvailable"], 1);
        let worker = &objects[4].manifest["spec"];
        assert_eq!(worker["replicas"], 2);
        assert_eq!(
            worker["selector"]["matchLabels"]["app.kubernetes.io/component"],
            "worker"
        );
        let container = &worker["template"]["spec"]["containers"][0];
        assert_eq!(
            container["args"],
            json!([
                "-c",
                "/etc/angos/boot/config.toml",
                "-c",
                "/etc/angos/hot/config.toml",
                "worker",
                "--queue",
                "cache",
                "--queue",
                "index",
                "--poll-interval",
                "5s"
            ])
        );
        assert!(container.get("ports").is_none());
        assert!(container.get("readinessProbe").is_none());
        assert_eq!(
            worker["template"]["metadata"]["annotations"][api::annotations::BOOT_HASH],
            "0123456789abcdef"
        );
        assert_eq!(objects[5].manifest["spec"]["replicas"], 3);
    }

    #[test]
    fn a_held_server_keeps_its_startup_layer_while_the_worker_moves() {
        let s = spec(json!({"storage": {"fs": {"volume": {"size": "1Gi"}}}, "worker": {}}));
        let mut i = input(&s);
        i.server_hold = Some(("fedcba9876543210", "ghcr.io/project-angos/angos:1.12.2"));
        let objects = objects(&i);
        let named = |name: &str| {
            &objects
                .iter()
                .find(|o| o.kind == "Deployment" && o.name == name)
                .unwrap()
                .manifest
        };
        let worker = &named("reg-worker")["spec"]["template"];
        let server = &named("reg")["spec"]["template"];
        assert_eq!(
            worker["metadata"]["annotations"][api::annotations::BOOT_HASH],
            "0123456789abcdef"
        );
        assert_eq!(
            server["metadata"]["annotations"][api::annotations::BOOT_HASH],
            "fedcba9876543210"
        );
        assert_eq!(
            server["spec"]["containers"][0]["image"],
            "ghcr.io/project-angos/angos:1.12.2"
        );
        assert!(
            server["spec"]["volumes"]
                .to_string()
                .contains("reg-boot-fedcba98")
        );
        // A single-writer claim: the worker goes where the server is.
        assert_eq!(
            worker["spec"]["affinity"]["podAffinity"]["requiredDuringSchedulingIgnoredDuringExecution"]
                [0]["labelSelector"]["matchLabels"]["app.kubernetes.io/component"],
            "server"
        );
    }

    #[test]
    fn client_tls_files_are_projected_per_slot() {
        let s = spec(json!({
            "storage": {"fs": {}},
            "scan": {"url": "https://scanner", "tls": {
                "ca": {"secretKeyRef": {"name": "scan-ca", "key": "ca"}},
                "clientCertificateSecretName": "scan-client"}},
            "auth": {"oidc": [{"name": "corp", "issuer": "https://idp", "audience": "angos",
                               "tls": {"ca": {"configMapKeyRef": {"name": "corp-ca", "key": "ca.crt"}}}}]},
        }));
        let (volume, mount) = client_tls_volume(&s).unwrap();
        assert_eq!(mount.mount_path, "/etc/angos/client-tls");
        let sources = serde_json::to_value(volume.projected.unwrap().sources).unwrap();
        assert_eq!(
            sources,
            json!([
                {"configMap": {"name": "corp-ca", "items": [{"key": "ca.crt", "path": "oidc-corp/ca.crt"}]}},
                {"secret": {"name": "scan-ca", "items": [{"key": "ca", "path": "scan/ca.crt"}]}},
                {"secret": {"name": "scan-client", "items": [
                    {"key": "tls.crt", "path": "scan/tls.crt"}, {"key": "tls.key", "path": "scan/tls.key"}]}},
            ])
        );
        // The Secrets the pods would read must consent.
        let objects = sketch("reg", "team", &s, "ghcr.io/project-angos/angos:1.12.3");
        assert_eq!(
            weebo_kit_application::pod_safety::secrets_to_check(&objects),
            ["scan-ca".to_string(), "scan-client".to_string()].into()
        );
        assert!(client_tls_volume(&spec(json!({"storage": {"fs": {}}}))).is_none());
    }

    #[test]
    fn images_default_to_the_targeted_version() {
        let s = spec(json!({"storage": {"fs": {}}}));
        assert_eq!(
            image(&s, "ghcr.io/project-angos/angos:1.12.3"),
            "ghcr.io/project-angos/angos:1.12.3"
        );
        let s = spec(json!({"storage": {"fs": {}}, "image": {"tag": "1.12.4"}}));
        assert_eq!(
            image(&s, "mirror.example/angos:1.12.3"),
            "mirror.example/angos:1.12.4"
        );
        assert_eq!(tag("registry:5000/angos:1.12.3"), "1.12.3");
        assert_eq!(tag("registry:5000/angos"), "latest");
    }

    #[test]
    fn the_default_pods_pass_the_kit_s_rules() {
        let s = spec(
            json!({"storage": {"fs": {"volume": {"claimName": "existing"}}},
            "auth": {"kubernetes": {"audience": "angos"}}}),
        );
        let objects = sketch("reg", "team", &s, "ghcr.io/project-angos/angos:1.12.3");
        assert!(
            weebo_kit_application::pod_safety::problems(
                &objects,
                &["ghcr.io/project-angos/angos".to_string()]
            )
            .is_empty()
        );
        // Only what the operator writes is mounted: nothing to consent to.
        assert!(weebo_kit_application::pod_safety::secrets_to_check(&objects).is_empty());
    }
}
