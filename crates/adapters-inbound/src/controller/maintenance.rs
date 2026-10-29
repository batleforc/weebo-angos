//! `AngosMaintenance` (namespaced): the operator schedules its runs, each a
//! `Job` it owns (`reconcile_maintenance`). Triggered by the maintenance,
//! by its Jobs, and by its time: the next run, or 15 s while one runs.

use std::sync::Arc;
use std::time::{Duration, Instant};

use api::Maintenance;
use application::ports::{ApiError, Owner, StatusLedger};
use application::use_cases::reconcile_maintenance::{
    MaintenancePass, MaintenancePorts, reconcile_maintenance, status_fields,
};
use k8s_openapi::api::batch::v1::Job;
use kube::api::Api;
use kube::core::PartialObjectMeta;
use kube::runtime::controller::Action;
use kube::runtime::reflector::ObjectRef;
use kube::runtime::{WatchStreamExt, watcher};
use kube::{Client, Resource, ResourceExt};
use weebo_kit_runtime::events;
use weebo_kit_runtime::status::StatusUpdate;

use super::{Ctx, Error, error_policy};

const KIND: &str = "AngosMaintenance";

pub async fn run(client: Client, ctx: Arc<Ctx>) {
    let health = ctx.health.clone();
    let jobs: Api<PartialObjectMeta<Job>> = ctx.scope.api(client.clone());
    let results = ctx
        .watches
        .controller(ctx.scope.api::<Maintenance>(client), &ctx.scope)
        .watches_stream(
            watcher(
                jobs,
                watcher::Config::default().labels("app.kubernetes.io/name=angos-maintenance"),
            )
            .default_backoff()
            .touched_objects(),
            |job| {
                let name = job.labels().get("app.kubernetes.io/instance")?.clone();
                Some(ObjectRef::<Maintenance>::new(&name).within(job.namespace().as_deref()?))
            },
        )
        .graceful_shutdown_on(ctx.shutdown.wait())
        .run(reconcile, error_policy, ctx);
    super::drive(KIND, health, results).await;
}

struct Ledger {
    client: Client,
}

#[async_trait::async_trait]
impl StatusLedger for Ledger {
    async fn record(
        &self,
        namespace: &str,
        name: &str,
        fields: serde_json::Map<String, serde_json::Value>,
    ) -> Result<(), ApiError> {
        let api: Api<Maintenance> = Api::namespaced(self.client.clone(), namespace);
        weebo_kit_runtime::status::record(&api, super::FIELD_MANAGER, name, &fields)
            .await
            .map(|_| ())
            .map_err(super::api_error(format_args!(
                "recording the status of {namespace}/{name}"
            )))
    }
}

async fn reconcile(obj: Arc<Maintenance>, ctx: Arc<Ctx>) -> Result<Action, Error> {
    if obj.meta().deletion_timestamp.is_some() {
        return Ok(Action::await_change());
    }
    let namespace = obj.namespace().unwrap_or_default();
    let api: Api<Maintenance> = Api::namespaced(ctx.client.clone(), &namespace);
    let started = Instant::now();
    let owner = Owner {
        namespace: namespace.clone(),
        name: obj.name_any(),
        uid: obj.uid().unwrap_or_default(),
    };
    let previous = obj.status.clone().unwrap_or_default();
    let ledger = Ledger {
        client: ctx.client.clone(),
    };
    let report = reconcile_maintenance(
        MaintenancePass {
            owner: &owner,
            created: obj.metadata.creation_timestamp.as_ref().map(|t| t.0),
            spec: &obj.spec,
            annotations: obj.annotations(),
            status: &previous,
            now: k8s_openapi::jiff::Timestamp::now(),
            default_image: &ctx.settings.default_image,
        },
        &MaintenancePorts {
            instances: ctx.attached.as_ref(),
            objects: ctx.maintenance_objects.as_ref(),
            jobs: ctx.jobs.as_ref(),
            logs: ctx.logs.as_ref(),
            maintenances: ctx.maintenances.as_ref(),
            repositories: ctx.repositories.as_ref(),
            ledger: &ledger,
        },
    )
    .await;
    super::record_reconcile(KIND, started, &report.outcome);
    super::metrics::record_maintenance(obj.spec.task, &previous, &report.status);
    let fields = status_fields(&previous, &report.status);
    weebo_kit_runtime::status::publish(
        &api,
        &ctx.recorder,
        super::FIELD_MANAGER,
        obj.as_ref(),
        &previous.common.conditions,
        StatusUpdate {
            ready: &report.outcome.ready,
            advisories: &report.outcome.advisories,
            fields,
        },
    )
    .await?;
    for (reason, message) in &report.events {
        events::announce_change(
            &ctx.recorder,
            &obj.object_ref(&()),
            *reason,
            events::RECONCILE,
            message,
        )
        .await;
    }
    Ok(match report.due_in {
        Some(due) => Action::requeue(Duration::from_secs(due.clamp(1, 300))),
        None => super::requeue_after(obj.as_ref(), &report.outcome),
    })
}
