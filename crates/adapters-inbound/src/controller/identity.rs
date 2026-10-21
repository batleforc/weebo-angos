//! `AngosIdentity` (namespaced): its password generated, hashed into its
//! own Secret for the instance, and written to its targets once the
//! instance's pods accept it (`reconcile_identity`). Triggered by the
//! identity and by its instance (its `status.identities`). A finalizer
//! removes the targets.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Instant;

use api::{Identity, LocalInstance};
use application::ports::Owner;
use application::use_cases::reconcile_identity::{
    IdentityPass, IdentityPorts, reconcile_identity, release,
};
use kube::api::Api;
use kube::runtime::controller::Action;
use kube::runtime::finalizer::{Event as Finalizer, finalizer};
use kube::runtime::reflector::ObjectRef;
use kube::{Client, Resource, ResourceExt};
use weebo_kit_runtime::events;
use weebo_kit_runtime::shared::WatchesIn;
use weebo_kit_runtime::status::StatusUpdate;

use super::{Ctx, Error, error_policy};

const KIND: &str = "AngosIdentity";

pub async fn run(client: Client, ctx: Arc<Ctx>) {
    let health = ctx.health.clone();
    let results = ctx
        .watches
        .controller(ctx.scope.api::<Identity>(client.clone()), &ctx.scope)
        // Only when what the instance made of its identities changes: its
        // other status writes (a reload being confirmed…) would wake them
        // all for nothing.
        .watches_in_filtered(
            &ctx.watches,
            &ctx.scope,
            &client,
            |instance: &LocalInstance| {
                super::hash_of(&instance.status.as_ref().map(|s| &s.identities))
            },
            |instance| {
                instance
                    .status
                    .iter()
                    .flat_map(|s| s.identities.keys())
                    .filter_map(|key| {
                        let (ns, name) = key.split_once('/')?;
                        Some(ObjectRef::<Identity>::new(name).within(ns))
                    })
                    .collect::<Vec<_>>()
            },
        )
        .graceful_shutdown_on(ctx.shutdown.wait())
        .run(reconcile, error_policy, ctx);
    super::drive(KIND, health, results).await;
}

fn owner(identity: &Identity) -> Owner {
    Owner {
        namespace: identity.namespace().unwrap_or_default(),
        name: identity.name_any(),
        uid: identity.uid().unwrap_or_default(),
    }
}

async fn reconcile(obj: Arc<Identity>, ctx: Arc<Ctx>) -> Result<Action, Error> {
    let namespace = obj.namespace().unwrap_or_default();
    let api: Api<Identity> = Api::namespaced(ctx.client.clone(), &namespace);
    finalizer(&api, super::FINALIZER, obj, |event| async {
        match event {
            Finalizer::Apply(identity) => apply(&api, &identity, &ctx).await,
            Finalizer::Cleanup(identity) => {
                let owner = owner(&identity);
                let spec = identity.spec.clone();
                let store = ctx.secret_store.clone();
                super::finalize(identity.as_ref(), &ctx, async move {
                    release(&spec, &owner, store.as_ref())
                        .await
                        .map_err(Error::Cleanup)
                })
                .await
            }
        }
    })
    .await
    .map_err(|e| Error::Finalizer(Box::new(e)))
}

async fn apply(api: &Api<Identity>, identity: &Identity, ctx: &Ctx) -> Result<Action, Error> {
    let started = Instant::now();
    let owner = owner(identity);
    let previous = identity.status.clone().unwrap_or_default();
    let annotations: BTreeMap<String, String> = identity.annotations().clone();
    let report = reconcile_identity(
        IdentityPass {
            owner: &owner,
            spec: &identity.spec,
            annotations: &annotations,
            status: &previous,
            now: k8s_openapi::jiff::Timestamp::now(),
        },
        &IdentityPorts {
            instances: ctx.attached.as_ref(),
            objects: ctx.identity_objects.as_ref(),
            credentials: ctx.credentials.as_ref(),
            keys: ctx.keys.as_ref(),
            hasher: ctx.hasher.as_ref(),
            store: ctx.secret_store.as_ref(),
        },
    )
    .await;
    super::record_reconcile(KIND, started, &report.outcome);
    let mut fields = serde_json::to_value(&report.status)
        .expect("a status serializes")
        .as_object()
        .cloned()
        .unwrap_or_default();
    fields.remove("conditions");
    fields.remove("observedGeneration");
    for key in [
        "passwordVersion",
        "pendingPasswordVersion",
        "lastRotationTime",
        "conflictWith",
        "username",
    ] {
        fields.entry(key).or_insert(serde_json::Value::Null);
    }
    weebo_kit_runtime::status::publish(
        api,
        &ctx.recorder,
        super::FIELD_MANAGER,
        identity,
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
            &identity.object_ref(&()),
            *reason,
            events::RECONCILE,
            message,
        )
        .await;
    }
    Ok(super::requeue_waiting(identity, &report.outcome))
}
