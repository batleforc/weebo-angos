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
