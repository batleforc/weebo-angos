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
