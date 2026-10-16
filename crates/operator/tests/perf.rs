//! What the operator costs a cluster (weebo-kit-perf): the release binary
//! against envtest. No kubelet: the harness stands in for one — the
//! instance's Deployment reported rolled out, one server pod Ready, started
//! after any write (so it read every configuration at start, as a pod that
//! restarts does) — and the repositories go `Ready` as on a cluster. A
//! warm-up cycle, an idle baseline, then a measured cycle — a burst of
//! `AngosRepository`s on one instance, an idle window with all of them in
//! place, a spec churn on all of them, their deletion — and a final idle
//! window, checked against the kit's small-cluster budget.
//!
//! `task perf` runs it. Tunable: `PERF_OBJECTS` (default 200),
//! `PERF_IDLE_SECS` (default 30); `WEEBO_PERF_REPORT_ONLY=1` reports
//! without failing; `WEEBO_PERF_BASELINE=<previous json>` adds the change
//! from a previous run. The report lands in `target/perf/`.

use std::time::{Duration, Instant};

use api::{LocalInstance, Repository};
use kube::api::Api;
use serde_json::json;
use testkit::envtest::EnvTestCluster;
use weebo_kit_perf::{BASELINE, Budget, FINAL, OperatorMetrics, PerfRun, Workload, names};

fn env_or(name: &str, default: u64) -> u64 {
    std::env::var(name)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

fn repository(name: &str) -> Repository {
    serde_json::from_value(json!({
        "apiVersion": "angos.weebo.io/v1alpha1", "kind": "AngosRepository",
        "metadata": {"name": name},
        "spec": {"prefix": name, "access": {"pull": ["anonymous"], "push": ["identity:ci"]}},
    }))
    .unwrap()
}

/// `Ready: True` for the current generation: served by the instance.
fn converged(r: &Repository) -> bool {
    let Some(status) = &r.status else {
        return false;
    };
    status.common.observed_generation == r.metadata.generation
        && status
            .common
            .conditions
            .iter()
            .any(|c| c.type_ == "Ready" && c.status == "True")
}

/// What a kubelet and a Deployment controller would do for the instance
/// `registry`: its rollout reported done, one server pod Ready on its
/// startup layer, started after any write.
async fn fake_kubelet(client: &kube::Client, timeout: Duration) {
    use k8s_openapi::api::apps::v1::Deployment;
    use k8s_openapi::api::core::v1::Pod;
    use kube::api::{Patch, PatchParams, PostParams};
    let deployments: Api<Deployment> = Api::namespaced(client.clone(), "default");
    let started = Instant::now();
    let deployment = loop {
        if let Some(d) = deployments.get_opt("registry").await.unwrap() {
            break d;
        }
        assert!(started.elapsed() < timeout, "no Deployment registry");
        tokio::time::sleep(Duration::from_millis(200)).await;
    };
    deployments
        .patch_status(
            "registry",
            &PatchParams::default(),
            &Patch::Merge(json!({"status": {
                "observedGeneration": deployment.metadata.generation,
                "replicas": 1, "updatedReplicas": 1, "readyReplicas": 1, "availableReplicas": 1,
            }})),
        )
        .await
        .unwrap();
    let template = deployment.spec.unwrap().template;
    let meta = template.metadata.unwrap_or_default();
    let pods: Api<Pod> = Api::namespaced(client.clone(), "default");
    let pod: Pod = serde_json::from_value(json!({
        "metadata": {"name": "registry-perf", "labels": meta.labels, "annotations": meta.annotations},
        "spec": {"containers": [{"name": "angos", "image": "ghcr.io/project-angos/angos:1.12.3"}]},
    }))
    .unwrap();
    pods.create(&PostParams::default(), &pod).await.unwrap();
    pods.patch_status(
        "registry-perf",
        &PatchParams::default(),
        &Patch::Merge(json!({"status": {
            "phase": "Running",
            "conditions": [{"type": "Ready", "status": "True"}],
            "containerStatuses": [{
                "name": "angos", "ready": true, "restartCount": 0,
                "image": "ghcr.io/project-angos/angos:1.12.3", "imageID": "",
                "state": {"running": {"startedAt": "2099-01-01T00:00:00Z"}},
            }],
        }})),
    )
    .await
    .unwrap();
}

async fn burst(repos: &Workload<Repository>, names: &[String], timeout: Duration) -> Duration {
    let objects: Vec<_> = names.iter().map(|n| repository(n)).collect();
    let started = Instant::now();
    repos.create(&objects).await;
    repos.wait_all(names, timeout, converged).await;
    started.elapsed()
}

async fn churn(repos: &Workload<Repository>, names: &[String], timeout: Duration) -> Duration {
    let started = Instant::now();
    repos
        .patch_all(
            names,
            |_| json!({"spec": {"access": {"pull": ["authenticated"], "push": ["identity:ci"]}}}),
        )
        .await;
    repos.wait_all(names, timeout, converged).await;
    started.elapsed()
}

/// The instance settled: every pod runs the configuration desired.
async fn settled(instances: &Workload<LocalInstance>, timeout: Duration) {
    instances
        .wait_all(&["registry".to_string()], timeout, |i: &LocalInstance| {
            i.status.as_ref().is_some_and(|s| {
                s.config_hash.is_some()
                    && s.config_hash == s.applied_config_hash
                    && s.conditions
                        .iter()
                        .any(|c| c.type_ == "Ready" && c.status == "True")
            })
        })
        .await;
}

async fn delete(
    repos: &Workload<Repository>,
    instances: &Workload<LocalInstance>,
    names: &[String],
    timeout: Duration,
) -> Duration {
    let started = Instant::now();
    // No pod ever confirms the release: the repositories are let go.
    repos
        .patch_all(
            names,
            |_| json!({"metadata": {"annotations": {"angos.weebo.io/force-orphan": "true"}}}),
        )
        .await;
    repos.delete_all(names).await;
    repos.wait_gone(names, timeout).await;
    // Done once the instance runs without them.
    settled(instances, timeout).await;
    started.elapsed()
}

async fn cycle(
    run: &mut PerfRun,
    repos: &Workload<Repository>,
    instances: &Workload<LocalInstance>,
    all: &[String],
    label: &str,
    idle: Option<Duration>,
    timeout: Duration,
) {
    let n = all.len();
    run.load(&format!("{label} burst"), n, burst(repos, all, timeout))
        .await;
    if let Some(idle) = idle {
        run.idle(&format!("{label} loaded"), idle).await;
    }
    run.load(&format!("{label} churn"), n, churn(repos, all, timeout))
        .await;
    run.load(
        &format!("{label} delete"),
        n,
        delete(repos, instances, all, timeout),
    )
    .await;
}

#[tokio::test]
#[ignore = "a perf run (minutes, release build): `task perf`"]
async fn the_operator_fits_a_small_cluster() {
    let objects = env_or("PERF_OBJECTS", 200) as usize;
    let idle = Duration::from_secs(env_or("PERF_IDLE_SECS", 30));
    let timeout = Duration::from_secs(300);

    let cluster: EnvTestCluster = testkit::envtest::start().await;
    let client = cluster.client();
    let mut command = tokio::process::Command::new(env!("CARGO_BIN_EXE_weebo-angos-operator"));
    command.env("RUST_LOG", "warn");
    let mut run = PerfRun::start(
        "weebo-angos",
        &cluster,
        command,
        OperatorMetrics::new("weebo_angos"),
    )
    .await;
    let repos = run.workload(Api::<Repository>::namespaced(client.clone(), "default"));
    let instances = run.workload(Api::<LocalInstance>::namespaced(client.clone(), "default"));

    // The instance's own objects, written once before the measure: a
    // one-off of about 45 requests (two passes: its Secrets, account,
    // Service and Deployment read then applied), not a per-object cost.
    let instance: LocalInstance = serde_json::from_value(json!({
        "apiVersion": "angos.weebo.io/v1alpha1", "kind": "AngosLocalInstance",
        "metadata": {"name": "registry"},
        "spec": {"storage": {"fs": {}}},
    }))
    .unwrap();
    instances.create(&[instance]).await;
    fake_kubelet(&client, timeout).await;
    instances
        .wait_all(&["registry".to_string()], timeout, |i: &LocalInstance| {
            i.status.as_ref().is_some_and(|s| {
                s.conditions
                    .iter()
                    .any(|c| c.type_ == "Ready" && c.status == "True")
            })
        })
        .await;
    run.idle("startup", idle).await;
    // The warm-up is whole cycles at full size: the baseline must follow a
    // process that already filled its caches, pools and allocator arenas.
    let all = names("repo", objects);
    // Two: the allocator's arenas keep growing a little on the second.
    cycle(&mut run, &repos, &instances, &all, "warm-up", None, timeout).await;
    cycle(
        &mut run,
        &repos,
        &instances,
        &all,
        "warm-up 2",
        None,
        timeout,
    )
    .await;
    run.idle(BASELINE, idle).await;
    cycle(
        &mut run,
        &repos,
        &instances,
        &all,
        "cycle",
        Some(idle),
        timeout,
    )
    .await;
    run.idle(FINAL, idle).await;

    run.finish().await.assert_within(&Budget::small_cluster());
}
