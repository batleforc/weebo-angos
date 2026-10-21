//! `AngosLocalInstance` (namespaced): the instance's workload and
//! configuration (`reconcile_instance`). Watched besides the CR: its
//! `Deployment` (a rollout's progress) and its pods (one that restarts
//! during a confirmation), both by metadata. No watch of the Secrets it
//! references: they are read again on every pass (300 s, 15 s while a
//! reload is being confirmed).
//!
//! A finalizer holds its deletion; the objects it owns go with it (owner
//! references).

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Instant;

use api::LocalInstance;
use application::ports::{Owner, StatusLedger};
use application::use_cases::reconcile_instance::{
    InstancePass, InstancePorts, reconcile_instance, status_fields,
};
use k8s_openapi::api::apps::v1::Deployment;
use k8s_openapi::api::core::v1::Pod;
use kube::api::Api;
use kube::core::PartialObjectMeta;
use kube::runtime::controller::Action;
use kube::runtime::finalizer::{Event as Finalizer, finalizer};
use kube::runtime::reflector::ObjectRef;
use kube::runtime::{WatchStreamExt, watcher};
use kube::{Client, Resource, ResourceExt};
use weebo_kit_runtime::events;
use weebo_kit_runtime::shared::WatchesIn;
use weebo_kit_runtime::status::StatusUpdate;

use super::{Ctx, Error, error_policy};

const KIND: &str = "AngosLocalInstance";

/// The instance an object of it belongs to (`app.kubernetes.io/instance`
/// with `managed-by: weebo-angos`).
fn instance_of<K: Resource>(object: &K) -> Option<ObjectRef<LocalInstance>> {
    let labels = object.meta().labels.as_ref()?;
    (labels
        .get("app.kubernetes.io/managed-by")
        .map(String::as_str)
        == Some(api::MANAGED_BY)
        || labels.get("app.kubernetes.io/name").map(String::as_str) == Some("angos"))
    .then_some(())?;
    let name = labels.get("app.kubernetes.io/instance")?;
    Some(ObjectRef::new(name).within(object.meta().namespace.as_deref()?))
}

/// What of a repository or an identity the instance renders: its spec
/// (generation), its annotations, its deletion and `status` — not its
/// conditions, which it writes on every pass.
fn bound_change<K: Resource, S: serde::Serialize>(object: &K, status: S) -> Option<u64> {
    let meta = object.meta();
    super::hash_of(&(
        meta.generation,
        &meta.annotations,
        meta.deletion_timestamp.is_some(),
        status,
    ))
}

pub async fn run(client: Client, ctx: Arc<Ctx>) {
    let health = ctx.health.clone();
    let owned = watcher::Config::default()
        .labels(&format!("app.kubernetes.io/managed-by={}", api::MANAGED_BY));
    let pods = watcher::Config::default().labels("app.kubernetes.io/name=angos");
    // Metadata only: what changed, not the objects.
    let deployments: Api<PartialObjectMeta<Deployment>> = ctx.scope.api(client.clone());
    let pod_api: Api<PartialObjectMeta<Pod>> = ctx.scope.api(client.clone());
    let results = ctx
        .watches
        .controller(ctx.scope.api::<LocalInstance>(client.clone()), &ctx.scope)
        .watches_stream(
            watcher(deployments, owned)
                .default_backoff()
                .touched_objects(),
            |d| instance_of(&d),
        )
        .watches_stream(
            watcher(pod_api, pods).default_backoff().touched_objects(),
            |p| instance_of(&p),
        )
        // A repository bound to an instance (or no longer) re-renders it.
        .watches_in_filtered(
            &ctx.watches,
            &ctx.scope,
            &client,
            |repo: &api::Repository| {
                // `rendered` and `conflictWith` come from the instance.
                bound_change(repo, repo.status.as_ref().map(|s| &s.common.instance))
            },
            |repo| {
                let bound = repo.status.as_ref()?.common.instance.clone()?;
                Some(ObjectRef::new(&bound.name).within(bound.namespace.as_deref()?))
            },
        )
        .watches_in_filtered(
            &ctx.watches,
            &ctx.scope,
            &client,
            |identity: &api::Identity| {
                // Its status without the conditions: the password version
                // it holds is what the instance renders.
                let status = identity.status.as_ref().map(|s| {
                    let mut s = serde_json::to_value(s).unwrap_or_default();
                    if let Some(m) = s.as_object_mut() {
                        m.remove("conditions");
                        m.remove("observedGeneration");
                    }
                    s
                });
                bound_change(identity, status)
            },
            |identity| {
                let bound = identity.status.as_ref()?.common.instance.clone()?;
                Some(ObjectRef::new(&bound.name).within(bound.namespace.as_deref()?))
            },
        )
        .graceful_shutdown_on(ctx.shutdown.wait())
        .run(reconcile, error_policy, ctx);
    super::drive(KIND, health, results).await;
}

/// The status fields a pass writes, right after a write: the kit's
/// `status::record`.
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
    ) -> Result<(), String> {
        let api: Api<LocalInstance> = Api::namespaced(self.client.clone(), namespace);
        weebo_kit_runtime::status::record(&api, super::FIELD_MANAGER, name, &fields)
            .await
            .map(|_| ())
            .map_err(|e| e.to_string())
    }
}

async fn reconcile(obj: Arc<LocalInstance>, ctx: Arc<Ctx>) -> Result<Action, Error> {
    let namespace = obj.namespace().unwrap_or_default();
    let api: Api<LocalInstance> = Api::namespaced(ctx.client.clone(), &namespace);
    finalizer(&api, super::FINALIZER, obj, |event| async {
        match event {
            Finalizer::Apply(instance) => apply(&api, &instance, &ctx).await,
            Finalizer::Cleanup(instance) => {
                super::metrics::forget_instance(&namespace, &instance.name_any());
                // Its objects go with it (owner references): nothing to
                // undo yet. Repositories and identities will hold it
                // (InstanceInUse) once they exist.
                super::finalize(instance.as_ref(), &ctx, async { Ok(()) }).await
            }
        }
    })
    .await
    .map_err(|e| Error::Finalizer(Box::new(e)))
}

async fn apply(
    api: &Api<LocalInstance>,
    instance: &LocalInstance,
    ctx: &Ctx,
) -> Result<Action, Error> {
    let started = Instant::now();
    let owner = Owner {
        namespace: instance.namespace().unwrap_or_default(),
        name: instance.name_any(),
        uid: instance.uid().unwrap_or_default(),
    };
    let previous = instance.status.clone().unwrap_or_default();
    let annotations: BTreeMap<String, String> = instance.annotations().clone();
    let ledger = Ledger {
        client: ctx.client.clone(),
    };
    let report = reconcile_instance(
        InstancePass {
            owner: &owner,
            spec: &instance.spec,
            annotations: &annotations,
            status: &previous,
            now: k8s_openapi::jiff::Timestamp::now(),
            default_image: &ctx.settings.default_image,
            log_limit: ctx.settings.reload_log_bytes,
        },
        &InstancePorts {
            objects: ctx.objects.as_ref(),
            jobs: ctx.jobs.as_ref(),
            maintenances: ctx.maintenances.as_ref(),
            logs: ctx.logs.as_ref(),
            credentials: ctx.credentials.as_ref(),
            issuer: ctx.issuer.as_ref(),
            keys: ctx.keys.as_ref(),
            ledger: &ledger,
            repositories: ctx.repositories.as_ref(),
            identities: ctx.identities.as_ref(),
            cel: ctx.cel.as_ref(),
        },
    )
    .await;
    super::record_reconcile(KIND, started, &report.outcome);
    super::metrics::record_instance(
        &owner.namespace,
        &owner.name,
        report.outcome.ready.is_ok(),
        &previous,
        &report.status,
    );
    weebo_kit_runtime::status::publish(
        api,
        &ctx.recorder,
        super::FIELD_MANAGER,
        instance,
        &previous.conditions,
        StatusUpdate {
            ready: &report.outcome.ready,
            advisories: &report.outcome.advisories,
            fields: status_fields(&report.status),
        },
    )
    .await?;
    for (reason, message) in &report.events {
        events::announce_change(
            &ctx.recorder,
            &instance.object_ref(&()),
            *reason,
            events::RECONCILE,
            message,
        )
        .await;
    }
    Ok(super::requeue_after(instance, &report.outcome))
}
