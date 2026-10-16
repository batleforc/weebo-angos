//! Port traits: what the use-cases need from the outside world. Which
//! instance a CR lives on is the kit's [`InstanceDirectory`] (on the local
//! instances only); the objects and pod logs of an instance are the kit's
//! [`OwnedObjects`] and [`PodLogs`].

use domain::error::ReasonCode;
pub use weebo_kit_application::instances::{
    BoundInstance, InstanceDirectory, InstanceError, ResolveError,
};
pub use weebo_kit_application::workload::{
    LogLine, LogQuery, OwnedObject, OwnedObjects, Owner, PodLogs, PodState, Rollout, WorkloadError,
};

/// The reason code of an instance that couldn't be resolved.
pub fn instance_reason(err: &InstanceError) -> ReasonCode {
    match err {
        InstanceError::Kubernetes(_) => ReasonCode::KubernetesApiError,
        InstanceError::Resolve(e) => match e {
            ResolveError::NotFound { .. } | ResolveError::NoInstance => {
                ReasonCode::InstanceNotFound
            }
            ResolveError::Ambiguous { .. } => ReasonCode::InstanceAmbiguous,
            ResolveError::InvalidRef(_) => ReasonCode::InvalidSpec,
            ResolveError::OutsideScope(_) | ResolveError::NotShared { .. } => {
                ReasonCode::InstanceNotAllowed
            }
            ResolveError::BindingChanged { .. } => ReasonCode::InstanceBindingChanged,
        },
    }
}

/// The reason code of a [`WorkloadError`].
pub fn workload_reason(e: &WorkloadError) -> ReasonCode {
    match e {
        WorkloadError::Conflict(_) => ReasonCode::OwnedObjectConflict,
        WorkloadError::Kubernetes(_) => ReasonCode::KubernetesApiError,
        WorkloadError::Refused(_) => ReasonCode::InvalidSpec,
        WorkloadError::SecretNotAllowed(_) => ReasonCode::SecretNotAllowed,
    }
}

/// Why a Secret the spec references couldn't be read.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CredentialError {
    /// The Secret or its key is missing.
    #[error("{0}")]
    Missing(String),
    /// The Secret lacks `angos.weebo.io/credential: "true"`.
    #[error("{0}")]
    NotAllowed(String),
    #[error("Kubernetes API error: {0}")]
    Kubernetes(String),
}

impl CredentialError {
    pub fn reason(&self) -> ReasonCode {
        match self {
            CredentialError::Missing(_) => ReasonCode::SecretMissing,
            CredentialError::NotAllowed(_) => ReasonCode::SecretNotAllowed,
            CredentialError::Kubernetes(_) => ReasonCode::KubernetesApiError,
        }
    }
}

/// The Secrets an instance's spec references, read only when they carry
/// the credential label (spec 2.8).
#[async_trait::async_trait]
pub trait Credentials: Send + Sync {
    /// The UTF-8 value of `key` of the Secret `name` of `namespace`.
    async fn read(&self, namespace: &str, name: &str, key: &str)
    -> Result<String, CredentialError>;
    /// Whether the Secret `name` of `namespace` carries the label (`None`:
    /// it doesn't exist).
    async fn consents(&self, namespace: &str, name: &str) -> Result<Option<bool>, CredentialError>;
}

/// The cluster's ServiceAccount token issuer (the API server's OIDC
/// discovery), for `auth.kubernetes` without an explicit issuer.
#[async_trait::async_trait]
pub trait ClusterIssuer: Send + Sync {
    async fn issuer(&self) -> Result<String, String>;
}

/// Fresh secrets the operator generates.
pub trait KeyGenerator: Send + Sync {
    /// A token-service key: 32 random bytes, base64.
    fn token_key(&self) -> String;
    /// An identity's password: 32 random bytes, base64url.
    fn password(&self) -> String;
}

/// Argon2id as `angos argon` hashes (v0x13, m=19456, t=2, p=1): the PHC
/// string the configuration holds.
pub trait PasswordHasher: Send + Sync {
    fn hash(&self, password: &str) -> String;
}

/// The `AngosIdentity`s bound to an instance, as [`RepositoryDirectory`].
#[async_trait::async_trait]
pub trait IdentityDirectory: Send + Sync {
    async fn bound_to(
        &self,
        instance: &Owner,
        selector: &api::local_instance::RepositorySelector,
    ) -> Result<Vec<crate::identities::IdentityView>, String>;
}

/// Where a reconcile records status fields right after the write they
/// describe (a status merge patch): a pass that fails half-way must not
/// forget it wrote.
#[async_trait::async_trait]
pub trait StatusLedger: Send + Sync {
    async fn record(
        &self,
        namespace: &str,
        name: &str,
        fields: serde_json::Map<String, serde_json::Value>,
    ) -> Result<(), String>;
}

/// What a CEL rule is evaluated against (spec 2.6): an access policy
/// (`request`, `identity`, `has_repository_policy()`) or an image policy
/// — retention, scan, index (`image`, `now()`, `days()`, `top_pushed()`…).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuleContext {
    Access,
    Image,
}

/// What Angos would make of a rule.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RuleVerdict {
    Ok,
    /// It fails to evaluate on a plausible request (an access rule then
    /// denies, an image rule applies its policy): advisory.
    MayFail(String),
    /// It doesn't compile, names what doesn't exist, or isn't a boolean.
    Refused(String),
}

/// Angos's CEL engine, at Angos's version (spec 2.6).
pub trait CelChecker: Send + Sync {
    fn check(&self, context: RuleContext, rule: &str) -> RuleVerdict;
}

/// The `AngosRepository`s bound to an instance (their `status.instance`),
/// in scope of the operator, from namespaces the instance accepts.
#[async_trait::async_trait]
pub trait RepositoryDirectory: Send + Sync {
    /// Every repository in scope that is, or may become, the instance's:
    /// bound to it, or not bound yet and naming it (or, without
    /// `instanceRef`, in its namespace) — `namespace/name`. What a prune
    /// must not run without (spec 6.6).
    async fn aiming_at(&self, instance: &Owner) -> Result<Vec<String>, String> {
        let _ = instance;
        Ok(Vec::new())
    }

    /// Those recording `instance` in `status.instance`, matching its
    /// `selector`, from its own namespace or one it accepts.
    async fn bound_to(
        &self,
        instance: &Owner,
        selector: &api::local_instance::RepositorySelector,
    ) -> Result<Vec<crate::repositories::RepositoryView>, String>;
}

/// The instances a repository may bind to: resolution through the
/// instance's consent (`repositorySelector.namespaces`), and the instance
/// itself.
#[async_trait::async_trait]
pub trait AttachedInstances: Send + Sync {
    async fn resolve_attached(
        &self,
        namespace: &str,
        instance_ref: Option<&weebo_kit_api::InstanceRef>,
        recorded: Option<&BoundInstance>,
    ) -> Result<BoundInstance, InstanceError>;
    async fn instance(&self, bound: &BoundInstance) -> Result<Option<api::LocalInstance>, String>;
}

/// Where a maintenance's `Job` stands.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct JobState {
    pub active: bool,
    pub succeeded: bool,
    pub failed: bool,
    /// Stopped by its `activeDeadlineSeconds`.
    pub deadline_exceeded: bool,
    /// Its pod, for the log.
    pub pod: Option<String>,
    /// Its container's exit code, once terminated.
    pub exit_code: Option<i32>,
}

/// The maintenances' `Job`s.
#[async_trait::async_trait]
pub trait Jobs: Send + Sync {
    async fn job(&self, namespace: &str, name: &str) -> Result<Option<JobState>, String>;
}

/// The maintenances of a namespace (the write lock looks at the others).
#[async_trait::async_trait]
pub trait MaintenanceDirectory: Send + Sync {
    async fn in_namespace(&self, namespace: &str) -> Result<Vec<api::Maintenance>, String>;
}
