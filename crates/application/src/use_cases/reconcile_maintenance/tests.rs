use std::sync::Mutex;

use api::LocalInstance;
use api::status::{InstanceStatus, RenderState, RepositoryRender};
use k8s_openapi::jiff::SignedDuration;
use serde_json::json;
use weebo_kit_api::Condition;

use super::*;
use crate::ports::{BoundInstance, InstanceError, JobState};
use crate::test_support::{FakeLedger, FakeOwnedObjects, FakePodLogs};

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

#[derive(Default)]
struct FakeJobs(Mutex<BTreeMap<String, JobState>>);

#[async_trait::async_trait]
impl Jobs for FakeJobs {
    async fn job(&self, _: &str, name: &str) -> Result<Option<JobState>, String> {
        Ok(self.0.lock().unwrap().get(name).cloned())
    }
}

#[derive(Default)]
struct Others(Mutex<Vec<api::Maintenance>>);

#[async_trait::async_trait]
impl MaintenanceDirectory for Others {
    async fn in_namespace(&self, _: &str) -> Result<Vec<api::Maintenance>, String> {
        Ok(self.0.lock().unwrap().clone())
    }
}

struct World {
    owner: Owner,
    spec: MaintenanceSpec,
    annotations: BTreeMap<String, String>,
    status: MaintenanceStatus,
    now: Timestamp,
    instances: Instances,
    objects: FakeOwnedObjects,
    jobs: FakeJobs,
    logs: FakePodLogs,
    others: Others,
    ledger: FakeLedger,
    repositories: Aiming,
}

#[derive(Default)]
struct Aiming(Mutex<Vec<String>>);

#[async_trait::async_trait]
impl crate::ports::RepositoryDirectory for Aiming {
    async fn bound_to(
        &self,
        _: &Owner,
        _: &api::local_instance::RepositorySelector,
    ) -> Result<Vec<crate::repositories::RepositoryView>, String> {
        Ok(Vec::new())
    }

    async fn aiming_at(&self, _: &Owner) -> Result<Vec<String>, String> {
        Ok(self.0.lock().unwrap().clone())
    }
}

fn ready_instance(storage: serde_json::Value) -> LocalInstance {
    let mut instance = LocalInstance::new(
        "reg",
        serde_json::from_value(json!({"storage": storage})).unwrap(),
    );
    instance.status = Some(InstanceStatus {
        boot_hash: Some("0123456789".into()),
        config_hash: Some("h".into()),
        applied_config_hash: Some("h".into()),
        conditions: vec![Condition {
            type_: "Ready".into(),
            status: "True".into(),
            reason: "Reconciled".into(),
            message: String::new(),
            last_transition_time: None,
            observed_generation: None,
        }],
        ..Default::default()
    });
    instance
}

impl World {
    fn new(spec: serde_json::Value) -> Self {
        Self {
            owner: Owner {
                namespace: "team".into(),
                name: "prune".into(),
                uid: "u-1".into(),
            },
            spec: serde_json::from_value(spec).unwrap(),
            annotations: BTreeMap::new(),
            status: MaintenanceStatus::default(),
            now: "2026-10-16T10:00:00Z".parse().unwrap(),
            instances: Instances(Mutex::new(ready_instance(
                json!({"fs": {"volume": {"size": "1Gi"}}}),
            ))),
            objects: FakeOwnedObjects::default(),
            jobs: FakeJobs::default(),
            logs: FakePodLogs::default(),
            others: Others::default(),
            ledger: FakeLedger::default(),
            repositories: Aiming::default(),
        }
    }

    async fn pass(&mut self) -> MaintenanceReport {
        let report = reconcile_maintenance(
            MaintenancePass {
                owner: &self.owner,
                created: Some("2026-10-16T09:00:00Z".parse().unwrap()),
                spec: &self.spec,
                annotations: &self.annotations,
                status: &self.status,
                now: self.now,
                default_image: "ghcr.io/project-angos/angos:1.12.3",
            },
            &MaintenancePorts {
                instances: &self.instances,
                objects: &self.objects,
                jobs: &self.jobs,
                logs: &self.logs,
                maintenances: &self.others,
                repositories: &self.repositories,
                ledger: &self.ledger,
            },
        )
        .await;
        self.status = report.status.clone();
        report
    }

    fn job_name(&self) -> String {
        self.status.last_run.as_ref().unwrap().job_name.clone()
    }

    fn job(&self, state: JobState) {
        self.jobs.0.lock().unwrap().insert(self.job_name(), state);
    }
}

fn advisories(report: &MaintenanceReport) -> Vec<ReasonCode> {
    report.outcome.advisories.iter().map(|a| a.reason).collect()
}

#[tokio::test]
async fn a_one_shot_run_is_recorded_started_followed_and_reported() {
    let mut world = World::new(json!({"task": "Prune"}));
    let started = world.pass().await;
    assert!(
        started
            .events
            .iter()
            .any(|(r, _)| *r == ReasonCode::MaintenanceStarted)
    );
    assert_eq!(world.status.run_count, 1);
    // Recorded before the Job was created.
    assert_eq!(world.ledger.0.lock().unwrap().len(), 1);
    let name = world.job_name();
    assert!(world.objects.object("Job", &name).is_some());
    world.job(JobState {
        active: true,
        ..Default::default()
    });
    let running = world.pass().await;
    assert!(advisories(&running).contains(&ReasonCode::MaintenanceRunning));
    world.job(JobState {
        succeeded: true,
        pod: Some("p".into()),
        exit_code: Some(0),
        ..Default::default()
    });
    world.logs.push(
        "team",
        "p",
        world.now,
        &json!({"level": "INFO", "fields": {"message": "would delete namespace orphan/app"}})
            .to_string(),
    );
    let done = world.pass().await;
    assert_eq!(done.outcome.reason(), ReasonCode::Reconciled);
    assert_eq!(
        world.status.last_run.as_ref().unwrap().state,
        RunState::Succeeded
    );
    assert!(
        world
            .status
            .last_dry_run
            .as_ref()
            .unwrap()
            .summary
            .contains("orphan/app")
    );
    // Never twice without a schedule.
    world.pass().await;
    assert_eq!(world.status.run_count, 1);
    // Unless asked again.
    world
        .annotations
        .insert(annotations::RUN_NOW.into(), "1".into());
    world.pass().await;
    assert_eq!(world.status.run_count, 2);
}

#[tokio::test]
async fn a_schedule_runs_at_its_times_only() {
    let mut world = World::new(json!({"task": "Scrub", "schedule": "45 * * * *"}));
    // 10:00, created 09:00: 09:45 is due.
    world.pass().await;
    assert_eq!(world.status.run_count, 1);
    world.job(JobState {
        succeeded: true,
        ..Default::default()
    });
    world.now += SignedDuration::from_mins(10);
    let report = world.pass().await;
    assert_eq!(world.status.run_count, 1);
    assert_eq!(
        world.status.next_run_time.as_deref(),
        Some("2026-10-16T10:45:00Z")
    );
    assert!(report.due_in.unwrap() > 0);
    world.now += SignedDuration::from_mins(40);
    world.pass().await;
    assert_eq!(world.status.run_count, 2);
}

#[tokio::test]
async fn a_prune_is_skipped_while_a_repository_could_lose_its_data() {
    let mut world = World::new(json!({"task": "Prune", "params": {"dryRun": false}}));
    let mut status = world.instances.0.lock().unwrap().status.clone().unwrap();
    status.repositories.insert(
        "team/app".into(),
        RepositoryRender {
            prefix: "app".into(),
            state: RenderState::SecretMissing,
            message: None,
            conflict_with: None,
            generation: None,
            applied_generation: None,
        },
    );
    world.instances.0.lock().unwrap().status = Some(status);
    let report = world.pass().await;
    assert!(advisories(&report).contains(&ReasonCode::MaintenanceSkipped));
    assert!(report.outcome.advisories[0].message.contains("team/app"));
    assert_eq!(world.status.run_count, 0);
    assert!(world.status.last_skip_time.is_some());
    assert!(world.objects.names("Job").is_empty());
}

#[tokio::test]
async fn one_writing_task_at_a_time() {
    let mut world = World::new(json!({"task": "Scrub", "params": {"dryRun": false}}));
    let mut other = api::Maintenance::new(
        "prune",
        serde_json::from_value(json!({"task": "Prune"})).unwrap(),
    );
    other.status = Some(MaintenanceStatus {
        common: api::Status {
            instance: Some(BoundInstance::local("team", "reg")),
            ..Default::default()
        },
        last_run: Some(Run {
            state: RunState::Running,
            start_time: "2026-10-16T09:59:00Z".into(),
            completion_time: None,
            job_name: "prune-1".into(),
            exit_code: None,
            message: None,
            dry_run: false,
        }),
        ..Default::default()
    });
    world.owner.name = "scrub".into();
    *world.others.0.lock().unwrap() = vec![other];
    let report = world.pass().await;
    assert!(advisories(&report).contains(&ReasonCode::MaintenanceDeferred));
    assert_eq!(world.status.run_count, 0);
    // Deferred, not skipped: it starts as soon as the other is done.
    world.others.0.lock().unwrap().clear();
    world.pass().await;
    assert_eq!(world.status.run_count, 1);
}

#[tokio::test]
async fn failures_are_said_and_a_lost_job_is_an_interruption() {
    let mut world = World::new(json!({"task": "Scrub"}));
    world.pass().await;
    world.job(JobState {
        failed: true,
        exit_code: Some(2),
        ..Default::default()
    });
    assert_eq!(
        world.pass().await.outcome.reason(),
        ReasonCode::MaintenanceFailed
    );
    assert_eq!(world.status.failure_count, 1);
    let mut world = World::new(json!({"task": "Scrub"}));
    world.pass().await;
    assert_eq!(
        world.pass().await.outcome.reason(),
        ReasonCode::MaintenanceInterrupted
    );
}

#[tokio::test]
async fn an_instance_without_a_reachable_store_is_refused() {
    let mut world = World::new(json!({"task": "Scrub"}));
    *world.instances.0.lock().unwrap() = ready_instance(json!({"fs": {}}));
    assert_eq!(world.pass().await.outcome.reason(), ReasonCode::InvalidSpec);
    assert!(world.objects.names("Job").is_empty());
}

#[tokio::test]
async fn a_prune_waits_for_a_repository_not_bound_yet() {
    let mut world = World::new(json!({"task": "Prune"}));
    *world.repositories.0.lock().unwrap() = vec!["team/new".into()];
    let report = world.pass().await;
    assert!(report.outcome.advisories[0].message.contains("team/new"));
    assert_eq!(world.status.run_count, 0);
}
