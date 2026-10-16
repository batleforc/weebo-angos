//! Single source of truth for every reason the operator surfaces. Status
//! conditions, Kubernetes Events and metrics all carry one of these
//! variants — never an ad-hoc string at the call site. Add a variant here
//! before reporting a new cause anywhere.

pub use weebo_kit_domain::{Reason, Severity};

weebo_kit_domain::reason_codes! {
    pub enum ReasonCode {
        /// `Ready: True` — the one success reason every reconciler uses.
        Reconciled,
        /// The spec breaks a rule Angos would refuse at load (a missing
        /// field, a value out of range, a duplicate, a reference to
        /// nothing), a CEL rule doesn't compile, `extraConfig` sets a path
        /// a typed field renders, or an access subject is too broad for
        /// `jobs`/`admin`. Only a change of the spec clears it.
        InvalidSpec,
        /// `spec.instanceRef` names no `AngosLocalInstance` the operator
        /// sees (or, unset, there is none in the namespace to default to).
        InstanceNotFound,
        /// `spec.instanceRef` is unset and several instances could serve
        /// the object.
        InstanceAmbiguous,
        /// The instance named is out of reach: one of another namespace
        /// that doesn't accept objects of this one, or outside the
        /// namespace the operator is limited to.
        InstanceNotAllowed,
        /// `spec.instanceRef` by name now designates another instance than
        /// the one recorded in `status.instance`: not followed until
        /// `spec.instanceRef.namespace` says which.
        InstanceBindingChanged,
        /// The instance is being deleted while repositories, identities or
        /// maintenances are still bound to it: it waits for them.
        InstanceInUse,
        /// A `Secret` the operator would read, or the instance's pods
        /// would mount, lacks the label `angos.weebo.io/credential: "true"`.
        SecretNotAllowed,
        /// A referenced `Secret`, or its key, is missing.
        SecretMissing,
        /// An object the operator would create exists and isn't the
        /// instance's (no owner reference, no `managed-by` label): never
        /// taken over.
        OwnedObjectConflict,
        /// The storage changed (type, endpoint, bucket, region, prefix):
        /// the new startup layer waits for the annotation
        /// `angos.weebo.io/confirm-storage` set to `status.pendingBootHash`.
        StorageChangePending,
        /// The instance's pods aren't ready on the desired startup layer
        /// yet (rolling out, or not scheduled).
        WorkloadNotReady,
        /// The new pods don't start (crash loop, `/readyz` never ready): the
        /// message quotes the last error they logged. The previous pods
        /// keep serving.
        BootRejected,
        /// The hot layer was written and the pods haven't all confirmed
        /// loading it yet.
        ConfigPropagating,
        /// A pod refused the hot layer: the last accepted one was put back.
        /// Only a change (of the spec, a repository, a Secret) is tried
        /// again.
        ConfigRejected,
        /// No pod said whether it loaded the hot layer within
        /// `reload.timeoutSeconds` (`onUnconfirmed: Wait`).
        ReloadUnconfirmed,
        /// Another repository of the instance has the same prefix or one
        /// that overlaps it (`team` and `team/app`): the older one is
        /// rendered.
        PrefixConflict,
        /// Another identity of the instance has the same username, or it
        /// equals the name of an OIDC provider.
        UsernameConflict,
        /// Deleting the repository (or pruning a downstream) would erase
        /// its data at the next prune: it needs the annotation
        /// `angos.weebo.io/allow-data-deletion: "true"`.
        DeletionNotAllowed,
        /// The maintenance run failed (its Job's exit code and last log
        /// lines in `status.lastRun`).
        MaintenanceFailed,
        /// The maintenance run outlived `timeoutSeconds`.
        MaintenanceTimeout,
        /// A run recorded as running has no Job any more (the operator or
        /// the Job was interrupted): not retried.
        MaintenanceInterrupted,
        /// A prune run was skipped by the prune lock (a repository not
        /// rendered or released recently, the instance not confirmed): the
        /// message says why.
        MaintenanceSkipped,
        /// Another writing task runs on the instance: this one waits for
        /// the next pass.
        MaintenanceDeferred,
        /// A run is in progress.
        MaintenanceRunning,
        /// The schedule is suspended (`suspend`, or the instance rolls out
        /// or upgrades).
        MaintenanceSuspended,
        /// The image tag goes back below `status.version` or jumps more
        /// than one minor: it needs `angos.weebo.io/approved-upgrade` set
        /// to that tag.
        UpgradeNotAllowed,
        /// An update would take a CR still carrying the finalizer out of
        /// the operator's label selector (`watch.labelSelector`): refused
        /// at admission.
        LeavesWatchScope,
        /// A Kubernetes API call of the operator itself failed (reading a
        /// CR or a Secret, applying an object, patching a status) —
        /// retried.
        KubernetesApiError,
        /// `extraConfig` is rendered: what it sets isn't checked by the
        /// operator, only by Angos at load.
        ExtraConfigUsed,
        /// A CEL rule failed to evaluate on a plausible request: in an
        /// access policy, an error is a denial.
        RuleMayFail,
        /// The index jobs may outgrow the container's memory limit
        /// (`global.jobs.index × global.limits.maxIndexEntries`).
        IndexMemoryRisk,
        /// `global.gcGraceSecs` was lowered on an instance in service.
        GcGraceLowered,
        /// The storage volume asks for another size than its existing
        /// claim: a claim is never resized by the operator.
        VolumeNotResized,
        /// The S3 credentials changed alone: the pods rolled without a
        /// confirmation.
        StorageCredentialsRotated,
        /// `auth.kubernetes` is used: the instance's account must be bound
        /// to `system:service-account-issuer-discovery` (gitops).
        IssuerDiscoveryNotBound,
        /// Every pod loaded the new hot layer (Event only).
        ConfigReloaded,
        /// The pods rolled onto a new startup layer or image (Event only).
        ConfigRolledOut,
        /// The token service's key was rotated: every token is invalid
        /// (Event only).
        TokenKeyRotated,
        /// An identity's password was rotated (Event only).
        PasswordRotated,
        /// A maintenance run started (Event only).
        MaintenanceStarted,
        /// The finalizer finished — cleanup done, or the CR force-orphaned
        /// (Event only).
        Released,
    }
}

impl Reason for ReasonCode {
    const SUCCESS: Self = ReasonCode::Reconciled;

    fn as_str(self) -> &'static str {
        ReasonCode::as_str(self)
    }

    fn severity(self) -> Severity {
        ReasonCode::severity(self)
    }
}

impl ReasonCode {
    /// Advisory codes don't block `Ready` (and count as `success` in the
    /// reconcile metrics); every failure cause is blocking.
    pub const fn severity(self) -> Severity {
        match self {
            ReasonCode::Reconciled
            | ReasonCode::StorageChangePending
            | ReasonCode::ReloadUnconfirmed
            | ReasonCode::MaintenanceSkipped
            | ReasonCode::MaintenanceDeferred
            | ReasonCode::MaintenanceRunning
            | ReasonCode::MaintenanceSuspended
            | ReasonCode::ExtraConfigUsed
            | ReasonCode::RuleMayFail
            | ReasonCode::IndexMemoryRisk
            | ReasonCode::GcGraceLowered
            | ReasonCode::VolumeNotResized
            | ReasonCode::StorageCredentialsRotated
            | ReasonCode::IssuerDiscoveryNotBound
            | ReasonCode::ConfigReloaded
            | ReasonCode::ConfigRolledOut
            | ReasonCode::TokenKeyRotated
            | ReasonCode::PasswordRotated
            | ReasonCode::MaintenanceStarted
            | ReasonCode::Released => Severity::Advisory,
            _ => Severity::Blocking,
        }
    }

    /// Failures only a change of the spec (or of what it references) can
    /// clear: retried on the steady interval rather than every 30 s. Each
    /// is retried right away anyway by the change that clears it (the
    /// kit's predicates, the watches).
    pub const fn needs_a_change(self) -> bool {
        matches!(
            self,
            ReasonCode::InvalidSpec
                | ReasonCode::InstanceAmbiguous
                | ReasonCode::InstanceNotAllowed
                | ReasonCode::InstanceBindingChanged
                | ReasonCode::SecretNotAllowed
                | ReasonCode::OwnedObjectConflict
                | ReasonCode::ConfigRejected
                | ReasonCode::PrefixConflict
                | ReasonCode::UsernameConflict
                | ReasonCode::DeletionNotAllowed
                | ReasonCode::UpgradeNotAllowed
                // A failed run is never retried: a new one comes from the
                // schedule, `run-now` or a new spec.
                | ReasonCode::MaintenanceFailed
                | ReasonCode::MaintenanceTimeout
                | ReasonCode::MaintenanceInterrupted
        )
    }

    /// The order a `Ready` condition picks its reason in, when several
    /// failures hold at once (spec §5): the first is reported.
    pub const INSTANCE_PRIORITY: &[ReasonCode] = &[
        ReasonCode::InvalidSpec,
        ReasonCode::OwnedObjectConflict,
        ReasonCode::SecretMissing,
        ReasonCode::SecretNotAllowed,
        ReasonCode::UpgradeNotAllowed,
        ReasonCode::BootRejected,
        ReasonCode::WorkloadNotReady,
        ReasonCode::ConfigRejected,
        ReasonCode::ConfigPropagating,
    ];
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_code_is_pascal_case() {
        weebo_kit_domain::reason::assert_pascal_case(ReasonCode::ALL);
    }

    #[test]
    fn only_failures_wait_for_a_change() {
        for code in ReasonCode::ALL {
            if code.needs_a_change() {
                assert_eq!(code.severity(), Severity::Blocking, "{code}");
            }
        }
        assert!(!ReasonCode::KubernetesApiError.needs_a_change());
        assert!(!ReasonCode::ConfigPropagating.needs_a_change());
    }

    #[test]
    fn the_instance_priority_lists_blocking_codes_once() {
        let priority = ReasonCode::INSTANCE_PRIORITY;
        for (i, code) in priority.iter().enumerate() {
            assert_eq!(code.severity(), Severity::Blocking, "{code}");
            assert!(!priority[i + 1..].contains(code), "{code}");
        }
    }
}
