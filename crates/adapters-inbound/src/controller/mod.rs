//! One `kube::runtime::Controller` per CRD, on the kit's skeleton
//! (triggers, concurrency, requeue, finalizer, Events, metrics). The
//! reconcile bodies delegate to `application::use_cases`.

use std::fmt::Debug;
use std::sync::{Arc, OnceLock};
use std::time::Instant;

use api::Status;
use api::status::Condition;
use application::ports::{
    BoundInstance, ClusterIssuer, Credentials, InstanceDirectory, KeyGenerator, OwnedObjects,
    PodLogs,
};
use application::use_cases::Outcome;
use domain::Failure;
use domain::error::ReasonCode;
use kube::api::Api;
use kube::runtime::controller::Action;
use kube::runtime::events::Recorder;
use kube::{Client, Resource};
use serde::Serialize;
use serde::de::DeserializeOwned;
use weebo_kit_runtime::events;
use weebo_kit_runtime::metrics::ReconcileMetrics;
use weebo_kit_runtime::status::StatusUpdate;
use weebo_kit_runtime::{Health, WatchScope};

/// Resolves once the process is asked to stop: every controller then stops
/// taking new work, lets its in-flight reconciles finish, and its `run`
/// returns.
pub use weebo_kit_runtime::Shutdown;
/// The kit's controller skeleton: a watch filtered to what reconciling
/// acts on (never the CR's own status patches), bounded concurrency
/// (`WEEBO_RECONCILE_CONCURRENCY`), the result stream drained into the
/// liveness.
pub use weebo_kit_runtime::controller::{controller, drive};

pub mod identity;
pub mod local_instance;
pub mod maintenance;
pub mod metrics;
pub mod repository;

/// Field manager of the status patches, meter and Event reporter name.
pub const FIELD_MANAGER: &str = "weebo-angos-operator";
pub const FINALIZER: &str = "angos.weebo.io/finalizer";
/// Prefix of every metric: `<prefix>_reconcile_total`, `<prefix>_leader`…
pub const METRIC_PREFIX: &str = "weebo_angos";

/// Shared across every controller. Concrete adapters are built and
/// injected once, in `operator::main` (tests inject fakes).
pub struct Ctx {
    pub client: Client,
    /// Which instance a CR lives on (the kit's `LocalInstances`, on the
    /// watch scope).
    pub instances: Arc<dyn InstanceDirectory>,
    /// The objects of the instances (the kit's `KubeOwnedObjects`).
    pub objects: Arc<dyn OwnedObjects>,
    /// The instances' pods' logs (`pods/log`).
    pub logs: Arc<dyn PodLogs>,
    /// The Secrets the specs reference (credential label required).
    pub credentials: Arc<dyn Credentials>,
    pub issuer: Arc<dyn ClusterIssuer>,
    pub keys: Arc<dyn KeyGenerator>,
    /// The repositories bound to an instance.
    pub repositories: Arc<dyn application::ports::RepositoryDirectory>,
    /// The identities bound to an instance.
    pub identities: Arc<dyn application::ports::IdentityDirectory>,
    /// An identity's own Secret (owner kind `AngosIdentity`).
    pub identity_objects: Arc<dyn OwnedObjects>,
    /// Argon2id.
    pub hasher: Arc<dyn application::ports::PasswordHasher>,
    /// Where identities write their credentials.
    pub secret_store: Arc<dyn weebo_kit_application::ports::SecretStore>,
    /// A maintenance's Jobs (owner kind `AngosMaintenance`).
    pub maintenance_objects: Arc<dyn OwnedObjects>,
    pub jobs: Arc<dyn application::ports::Jobs>,
    pub maintenances: Arc<dyn application::ports::MaintenanceDirectory>,
    /// The instances a repository may bind to.
    pub attached: Arc<dyn application::ports::AttachedInstances>,
    /// Angos's CEL engine.
    pub cel: Arc<dyn application::ports::CelChecker>,
    /// The operator's own settings.
    pub settings: Settings,
    /// One recorder for the whole process: the kit folds repeated Events
    /// into a series only within a recorder.
    pub recorder: Recorder,
    pub shutdown: Shutdown,
    /// Liveness: controllers report their watch errors here.
    pub health: Arc<Health>,
    /// What the controllers watch (`WEEBO_WATCH_NAMESPACE`,
    /// `WEEBO_WATCH_LABEL_SELECTOR`): every watch goes through it.
    pub scope: WatchScope,
}

/// What the operator's environment sets (spec 6.11).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Settings {
    /// `WEEBO_ANGOS_DEFAULT_IMAGE`: the image of an instance without one.
    pub default_image: String,
    /// `WEEBO_ANGOS_RELOAD_LOG_BYTES`: at most this many bytes of a pod's
    /// log are read.
    pub reload_log_bytes: i64,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            default_image: format!("{}:{}", api::angos::IMAGE_REPOSITORY, api::angos::VERSION),
            reload_log_bytes: 1 << 20,
        }
    }
}

/// What stopped a reconcile before (or instead of) its status.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("Kubernetes API error: {0}")]
    Kube(#[from] kube::Error),
    /// Anything the finalizer helper reports: the reconcile's own error
    /// (`ApplyFailed`/`CleanupFailed`) or adding/removing the finalizer.
    #[error("{0}")]
    Finalizer(Box<kube::runtime::finalizer::Error<Error>>),
    /// A cleanup that couldn't delete the remote object: the finalizer
    /// stays until it can (or the CR is force-orphaned).
    #[error("{0}")]
    Cleanup(Failure),
}

impl Error {
    /// The reason code of the Event announcing this error, and whether it
    /// happened reconciling or cleaning up (the Event's `action`).
    pub fn reason(&self) -> (ReasonCode, &'static str) {
        use kube::runtime::finalizer::Error as Finalizer;
        match self {
            Error::Kube(_) => (ReasonCode::KubernetesApiError, events::RECONCILE),
            Error::Cleanup(failure) => (failure.reason, events::CLEANUP),
            Error::Finalizer(err) => match err.as_ref() {
                Finalizer::ApplyFailed(inner) => inner.reason(),
                Finalizer::CleanupFailed(inner) => (inner.reason().0, events::CLEANUP),
                _ => (ReasonCode::KubernetesApiError, events::RECONCILE),
            },
        }
    }
}

/// Every reconcile that errors out is announced by a `Warning` Event on
/// its CR, then retried in ~30s (the kit's).
pub fn error_policy<K>(obj: Arc<K>, err: &Error, ctx: Arc<Ctx>) -> Action
where
    K: Resource<DynamicType = ()>,
{
    let (reason, action) = err.reason();
    weebo_kit_runtime::controller::retry_after_error(
        &ctx.recorder,
        obj.object_ref(&()),
        reason,
        action,
        err,
    )
}

/// Steady-state drift check after a success, a quick retry after a
/// failure (both jittered — the kit's).
pub fn requeue_after(outcome: &Outcome) -> Action {
    match &outcome.ready {
        // A reload being confirmed (its logs), an upgrade's Job (not
        // watched): read again soon. A rollout's progress comes through the
        // Deployment and pod watches: the steady interval is the fallback.
        Err(f)
            if matches!(
                f.reason,
                ReasonCode::ConfigPropagating | ReasonCode::UpgradeInProgress
            ) =>
        {
            weebo_kit_runtime::controller::requeue_after(false, Some(15))
        }
        Err(f) if f.reason == ReasonCode::WorkloadNotReady => {
            weebo_kit_runtime::controller::requeue_after(true, None)
        }
        Err(f) => weebo_kit_runtime::controller::requeue_after(f.reason.needs_a_change(), None),
        Ok(_)
            if outcome
                .advisories
                .iter()
                .any(|a| a.reason == ReasonCode::StorageChangePending) =>
        {
            weebo_kit_runtime::controller::requeue_after(false, None)
        }
        Ok(_) => weebo_kit_runtime::controller::requeue_after(true, None),
    }
}

/// [`requeue_after`] for what waits on its instance (a repository, an
/// identity): the instance's status changes wake it through the watch, the
/// steady interval is the fallback.
pub fn requeue_waiting(outcome: &Outcome) -> Action {
    match &outcome.ready {
        Err(f)
            if matches!(
                f.reason,
                ReasonCode::ConfigPropagating | ReasonCode::WorkloadNotReady
            ) =>
        {
            weebo_kit_runtime::controller::requeue_after(true, None)
        }
        _ => requeue_after(outcome),
    }
}

/// A hash of `value`'s JSON: what a watch predicate compares.
pub fn hash_of<T: serde::Serialize>(value: &T) -> Option<u64> {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    serde_json::to_string(value).ok()?.hash(&mut hasher);
    Some(hasher.finish())
}

/// `<prefix>_reconcile_total` / `_duration_seconds` (the kit's
/// `ReconcileMetrics`). A no-op until the telemetry installs a real
/// `MeterProvider`.
pub fn record_reconcile(kind: &'static str, started: Instant, outcome: &Outcome) {
    static METRICS: OnceLock<ReconcileMetrics> = OnceLock::new();
    METRICS
        .get_or_init(|| ReconcileMetrics::new(FIELD_MANAGER, METRIC_PREFIX))
        .record(kind, outcome.reason(), started.elapsed());
}

/// The `status` of every CRD here is a [`Status`].
pub trait HasStatus {
    fn status(&self) -> Option<&Status>;

    /// The instance recorded at the last resolution.
    fn bound_instance(&self) -> Option<&BoundInstance> {
        self.status()?.instance.as_ref()
    }
}

/// Publishes `outcome` on `obj` (read fresh from the API server: its
/// conditions are the previous ones). The kit's: one merge patch
/// (conditions, `observedGeneration` on success, the bound instance),
/// retried while the apiserver is unavailable, then a `Warning` on every
/// failed reconcile and a `Normal` for every condition change.
pub async fn publish<K>(
    api: &Api<K>,
    recorder: &Recorder,
    obj: &K,
    outcome: &Outcome,
) -> Result<(), kube::Error>
where
    K: Resource<DynamicType = ()> + HasStatus + Clone + Debug + DeserializeOwned + Serialize,
{
    let previous: &[Condition] = obj
        .status()
        .map(|s| s.conditions.as_slice())
        .unwrap_or_default();
    let mut fields = serde_json::Map::new();
    if let Some(instance) = &outcome.instance {
        let instance = serde_json::to_value(instance).expect("a BoundInstance serializes");
        fields.insert("instance".into(), instance);
    }
    weebo_kit_runtime::status::publish(
        api,
        recorder,
        FIELD_MANAGER,
        obj,
        previous,
        StatusUpdate {
            ready: &outcome.ready,
            advisories: &outcome.advisories,
            fields,
        },
    )
    .await?;
    Ok(())
}

/// A finalizer's cleanup — unless the CR carries
/// `angos.weebo.io/force-orphan: "true"`, the way out for
/// a CR whose cleanup can't succeed any more (its instance gone) — then a
/// `Released` Event (the kit's).
pub async fn finalize<K>(
    obj: &K,
    ctx: &Ctx,
    cleanup: impl Future<Output = Result<(), Error>>,
) -> Result<Action, Error>
where
    K: Resource<DynamicType = ()>,
{
    weebo_kit_runtime::controller::finalize(
        obj,
        &ctx.recorder,
        &api::ANNOTATIONS,
        ReasonCode::Released,
        cleanup,
    )
    .await
}

#[cfg(test)]
mod tests {
    use kube::runtime::finalizer::Error as Finalizer;

    use super::*;

    #[test]
    fn errors_map_to_a_reason_and_an_action() {
        let in_use = || Error::Cleanup(Failure::new(ReasonCode::InstanceInUse, "x"));
        assert_eq!(
            in_use().reason(),
            (ReasonCode::InstanceInUse, events::CLEANUP)
        );
        assert_eq!(
            Error::Finalizer(Box::new(Finalizer::CleanupFailed(in_use()))).reason(),
            (ReasonCode::InstanceInUse, events::CLEANUP)
        );
        assert_eq!(
            Error::Finalizer(Box::new(Finalizer::UnnamedObject)).reason(),
            (ReasonCode::KubernetesApiError, events::RECONCILE)
        );
    }
}
