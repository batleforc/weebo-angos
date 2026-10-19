//! `AngosRepository` (namespaced): bound to its instance, checked, and what
//! the instance made of it reflected (`reconcile_repository`). Triggered
//! by the repository and by its instance (its `status.repositories` names
//! the repositories it rendered or refused). It writes nothing but its
//! status: the instance renders it.

use std::sync::Arc;
use std::time::Instant;

use api::{LocalInstance, Repository};
use application::use_cases::reconcile_repository::{
    RepositoryPass, reconcile_repository, release_repository,
};
use kube::api::Api;
use kube::runtime::WatchStreamExt;
use kube::runtime::controller::Action;
use kube::runtime::finalizer::{Event as Finalizer, finalizer};
use kube::runtime::reflector::ObjectRef;
use kube::{Client, Resource, ResourceExt};
use weebo_kit_runtime::status::StatusUpdate;

use super::{Ctx, Error, error_policy};

const KIND: &str = "AngosRepository";

pub async fn run(client: Client, ctx: Arc<Ctx>) {
    let health = ctx.health.clone();
    let results = ctx
        .watches
        .controller(ctx.scope.api::<Repository>(client.clone()), &ctx.scope)
        // Only when what the instance made of its repositories changes: its
        // other status writes (a reload being confirmed…) would wake them
        // all for nothing.
        .watches_stream(
            super::touched::<LocalInstance>(&ctx, &client).predicate_filter(
                |instance: &LocalInstance| {
                    super::hash_of(&instance.status.as_ref().map(|s| &s.repositories))
                },
                Default::default(),
            ),
            |instance| {
                instance
                    .status
                    .iter()
                    .flat_map(|s| s.repositories.keys())
                    .filter_map(|key| {
                        let (ns, name) = key.split_once('/')?;
                        Some(ObjectRef::<Repository>::new(name).within(ns))
                    })
                    .collect::<Vec<_>>()
            },
        )
        .graceful_shutdown_on(ctx.shutdown.wait())
        .run(reconcile, error_policy, ctx);
    super::drive(KIND, health, results).await;
}

async fn reconcile(obj: Arc<Repository>, ctx: Arc<Ctx>) -> Result<Action, Error> {
    let namespace = obj.namespace().unwrap_or_default();
    let api: Api<Repository> = Api::namespaced(ctx.client.clone(), &namespace);
    finalizer(&api, super::FINALIZER, obj, |event| async {
        match event {
            Finalizer::Apply(repo) => apply(repo, ctx.clone()).await,
            Finalizer::Cleanup(repo) => {
                let status = repo.status.clone().unwrap_or_default();
                let released = release_repository(
                    &namespace,
                    &repo.name_any(),
                    &repo.spec.prefix,
                    repo.annotations(),
                    status.common.instance.as_ref(),
                    ctx.attached.as_ref(),
                )
                .await;
                if let Err(failure) = &released {
                    // Said on the repository too, not only in an Event.
                    weebo_kit_runtime::status::publish(
                        &api,
                        &ctx.recorder,
                        super::FIELD_MANAGER,
                        repo.as_ref(),
                        &status.common.conditions,
                        StatusUpdate {
                            ready: &Err(failure.clone()),
                            advisories: &[],
                            fields: serde_json::Map::new(),
                        },
                    )
                    .await?;
                }
                super::finalize(repo.as_ref(), &ctx, async {
                    released.map_err(Error::Cleanup)
                })
                .await
            }
        }
    })
    .await
    .map_err(|e| Error::Finalizer(Box::new(e)))
}

async fn apply(obj: Arc<Repository>, ctx: Arc<Ctx>) -> Result<Action, Error> {
    let namespace = obj.namespace().unwrap_or_default();
    let api: Api<Repository> = Api::namespaced(ctx.client.clone(), &namespace);
    let started = Instant::now();
    let status = obj.status.clone().unwrap_or_default();
    let report = reconcile_repository(
        RepositoryPass {
            namespace: &namespace,
            name: &obj.name_any(),
            generation: obj.meta().generation,
            spec: &obj.spec,
            annotations: obj.annotations(),
            recorded: status.common.instance.as_ref(),
        },
        ctx.attached.as_ref(),
        ctx.cel.as_ref(),
    )
    .await;
    super::record_reconcile(KIND, started, &report.outcome);
    let mut fields = serde_json::Map::new();
    if let Some(instance) = &report.outcome.instance {
        fields.insert(
            "instance".into(),
            serde_json::to_value(instance).expect("a BoundInstance serializes"),
        );
    }
    fields.insert("rendered".into(), report.rendered.into());
    fields.insert(
        "conflictWith".into(),
        report
            .conflict_with
            .clone()
            .map_or(serde_json::Value::Null, Into::into),
    );
    weebo_kit_runtime::status::publish(
        &api,
        &ctx.recorder,
        super::FIELD_MANAGER,
        obj.as_ref(),
        &status.common.conditions,
        StatusUpdate {
            ready: &report.outcome.ready,
            advisories: &report.outcome.advisories,
            fields,
        },
    )
    .await?;
    Ok(super::requeue_waiting(&report.outcome))
}
