//! The validating webhook (`POST /validate`, the kit's router):
//!
//! - an `AngosLocalInstance` is refused when Angos would refuse its
//!   configuration (its rules checked on Angos's CEL engine), when its pods
//!   break the operator's rules, or when a Secret it names exists without
//!   `angos.weebo.io/credential: "true"` (spec 2.2, 2.6, 2.8) — a Secret
//!   that doesn't exist yet is let through, the reconcile waits for it;
//! - an `AngosRepository` is refused when its vocabulary or its rules
//!   don't hold, or when its prefix overlaps a repository already bound to
//!   its instance (spec 4.2), or when its instance can't be read;
//! - an `AngosMaintenance` whose schedule doesn't read.

use std::sync::Arc;

use api::{LocalInstance, Repository};
use application::admission::{check_instance, referenced_secrets};
use application::ports::{
    AttachedInstances, CelChecker, Credentials, InstanceError, Owner, RepositoryDirectory,
    ResolveError, instance_reason,
};
use domain::error::ReasonCode;
use domain::validation::belongs_to;
use kube::core::DynamicObject;
use weebo_kit_runtime::admission::{Admission, Denial, typed};

pub struct AngosAdmission {
    pub credentials: Arc<dyn Credentials>,
    pub cel: Arc<dyn CelChecker>,
    pub attached: Arc<dyn AttachedInstances>,
    pub repositories: Arc<dyn RepositoryDirectory>,
    pub allowed_images: Vec<String>,
    pub default_image: String,
}

#[async_trait::async_trait]
impl Admission for AngosAdmission {
    type Reason = ReasonCode;

    async fn admit(
        &self,
        kind: &str,
        namespace: &str,
        object: &DynamicObject,
    ) -> Result<(), Denial<ReasonCode>> {
        match kind {
            "AngosLocalInstance" => {}
            "AngosRepository" => return self.admit_repository(namespace, object).await,
            "AngosMaintenance" => {
                let maintenance: api::Maintenance = typed(object)
                    .map_err(|e| Denial::new(ReasonCode::InvalidSpec, e.to_string()))?;
                if let Some(schedule) = &maintenance.spec.schedule {
                    application::cron::Schedule::parse(schedule).map_err(|e| {
                        Denial::new(ReasonCode::InvalidSpec, format!("spec.schedule: {e}"))
                    })?;
                }
                return Ok(());
            }
            _ => {
                return Err(Denial::new(
                    ReasonCode::InvalidSpec,
                    format!("kind {kind} is not one the operator admits"),
                ));
            }
        }
        let instance: LocalInstance =
            typed(object).map_err(|e| Denial::new(ReasonCode::InvalidSpec, e.to_string()))?;
        let name = object.metadata.name.clone().unwrap_or_default();
        check_instance(
            &name,
            namespace,
            &instance.spec,
            &self.allowed_images,
            &self.default_image,
            self.cel.as_ref(),
        )
        .map_err(|f| Denial::new(f.reason, f.message))?;
        let image = application::workload::image(&instance.spec, &self.default_image);
        // On an update, the object carries the status in place: the
        // version the pods run.
        application::upgrade::check(
            instance.status.as_ref().and_then(|s| s.version.as_deref()),
            application::workload::tag(&image),
            instance
                .metadata
                .annotations
                .as_ref()
                .and_then(|a| a.get(api::annotations::APPROVED_UPGRADE))
                .map(String::as_str),
        )
        .map_err(|m| Denial::new(ReasonCode::UpgradeNotAllowed, m.to_string()))?;
        for secret in referenced_secrets(&name, namespace, &instance.spec, &image) {
            match self.credentials.consents(namespace, &secret).await {
                Ok(Some(false)) => {
                    return Err(Denial::new(
                        ReasonCode::SecretNotAllowed,
                        weebo_kit_application::pod_safety::secret_not_labelled(
                            namespace,
                            &secret,
                            api::CREDENTIAL_LABEL,
                        ),
                    ));
                }
                Ok(_) => {}
                Err(e) => return Err(Denial::new(e.reason(), e.to_string())),
            }
        }
        Ok(())
    }

    fn undecidable(&self) -> ReasonCode {
        ReasonCode::InvalidSpec
    }
}

impl AngosAdmission {
    async fn admit_repository(
        &self,
        namespace: &str,
        object: &DynamicObject,
    ) -> Result<(), Denial<ReasonCode>> {
        let repo: Repository =
            typed(object).map_err(|e| Denial::new(ReasonCode::InvalidSpec, e.to_string()))?;
        let name = object.metadata.name.clone().unwrap_or_default();
        let recorded = repo.status.as_ref().and_then(|s| s.common.instance.clone());
        // Not resolvable yet (no instance): the spec alone is checked, the
        // reconcile says why it waits. Any other failure is refused: letting
        // it through would skip the prefix check below.
        let bound = match self
            .attached
            .resolve_attached(
                namespace,
                repo.spec.instance_ref.as_ref(),
                recorded.as_ref(),
            )
            .await
        {
            Ok(bound) => Some(bound),
            Err(InstanceError::Resolve(
                ResolveError::NoInstance | ResolveError::NotFound { .. },
            )) => None,
            Err(e) => {
                tracing::warn!(namespace, %e, "repository refused: its instance can't be resolved");
                return Err(Denial::new(instance_reason(&e), e.to_string()));
            }
        };
        let instance = match &bound {
            Some(bound) => self
                .attached
                .instance(bound)
                .await
                .map_err(|e| Denial::new(e.reason(), e.to_string()))?,
            None => None,
        };
        let kube = instance
            .as_ref()
            .and_then(|i| i.spec.auth.kubernetes.as_ref())
            .map(|k| k.name.as_str())
            // Unknown yet: the kubernetes subjects are checked later.
            .or(if instance.is_none() {
                Some("kube")
            } else {
                None
            });
        let annotations = object.metadata.annotations.clone().unwrap_or_default();
        application::repositories::validate(&repo.spec, &annotations, kube, self.cel.as_ref())
            .map_err(|problems| Denial::new(ReasonCode::InvalidSpec, problems.join("; ")))?;
        let Some(instance) = instance else {
            return Ok(());
        };
        let owner = Owner {
            namespace: instance.metadata.namespace.clone().unwrap_or_default(),
            name: instance.metadata.name.clone().unwrap_or_default(),
            uid: instance.metadata.uid.clone().unwrap_or_default(),
        };
        let bound_repos = self
            .repositories
            .bound_to(&owner, &instance.spec.repository_selector)
            .await
            .map_err(|e| Denial::new(e.reason(), e.to_string()))?;
        let key = format!("{namespace}/{name}");
        let prefix = &repo.spec.prefix;
        if let Some(other) = bound_repos.iter().find(|r| {
            r.key() != key
                && (belongs_to(prefix, &r.spec.prefix) || belongs_to(&r.spec.prefix, prefix))
        }) {
            return Err(Denial::new(
                ReasonCode::PrefixConflict,
                format!(
                    "prefix {prefix:?} overlaps {:?} of {} on {}/{}",
                    other.spec.prefix,
                    other.key(),
                    owner.namespace,
                    owner.name
                ),
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use application::test_support::FakeCredentials;
    use serde_json::json;

    use super::*;

    struct NoInstance;

    #[async_trait::async_trait]
    impl AttachedInstances for NoInstance {
        async fn resolve_attached(
            &self,
            _: &str,
            _: Option<&weebo_kit_api::InstanceRef>,
            _: Option<&application::ports::BoundInstance>,
        ) -> Result<application::ports::BoundInstance, application::ports::InstanceError> {
            Err(ResolveError::NoInstance.into())
        }

        async fn instance(
            &self,
            _: &application::ports::BoundInstance,
        ) -> Result<Option<LocalInstance>, application::ports::ApiError> {
            Ok(None)
        }
    }

    fn admission(credentials: FakeCredentials) -> AngosAdmission {
        AngosAdmission {
            credentials: Arc::new(credentials),
            cel: Arc::new(application::test_support::FakeCel::default()),
            attached: Arc::new(NoInstance),
            repositories: Arc::new(application::test_support::FakeRepositories::default()),
            allowed_images: vec![api::angos::IMAGE_REPOSITORY.into()],
            default_image: format!("{}:{}", api::angos::IMAGE_REPOSITORY, api::angos::VERSION),
        }
    }

    fn object(spec: serde_json::Value) -> DynamicObject {
        serde_json::from_value(json!({
            "apiVersion": "angos.weebo.io/v1alpha1", "kind": "AngosLocalInstance",
            "metadata": {"name": "reg", "namespace": "team"}, "spec": spec,
        }))
        .unwrap()
    }

    async fn admit(
        credentials: FakeCredentials,
        spec: serde_json::Value,
    ) -> Result<(), ReasonCode> {
        admission(credentials)
            .admit("AngosLocalInstance", "team", &object(spec))
            .await
            .map_err(|d| d.reason)
    }

    #[tokio::test]
    async fn secrets_that_exist_must_opt_in() {
        let s3 = json!({"storage": {"s3": {"endpoint": "http://s3", "bucket": "b", "region": "r",
            "credentialsSecretRef": {"name": "s3"}}}});
        // Not there yet: the reconcile waits for it.
        assert_eq!(admit(FakeCredentials::default(), s3.clone()).await, Ok(()));
        let unlabelled = FakeCredentials::default();
        unlabelled.insert_unlabelled("team", "s3", &[]);
        assert_eq!(
            admit(unlabelled, s3.clone()).await,
            Err(ReasonCode::SecretNotAllowed)
        );
        let labelled = FakeCredentials::default();
        labelled.insert("team", "s3", &[]);
        assert_eq!(admit(labelled, s3).await, Ok(()));
    }

    #[tokio::test]
    async fn an_update_moves_to_a_tested_version_forward_unless_approved() {
        let updated = |tag: &str, approved: Option<&str>| -> DynamicObject {
            let mut annotations = serde_json::Map::new();
            if let Some(a) = approved {
                annotations.insert("angos.weebo.io/approved-upgrade".into(), a.into());
            }
            serde_json::from_value(json!({
                "apiVersion": "angos.weebo.io/v1alpha1", "kind": "AngosLocalInstance",
                "metadata": {"name": "reg", "namespace": "team", "annotations": annotations},
                "spec": {"storage": {"fs": {}}, "image": {"tag": tag}},
                "status": {"version": "1.12.3"},
            }))
            .unwrap()
        };
        let admission = admission(FakeCredentials::default());
        let admit = |o: DynamicObject| {
            let admission = &admission;
            async move {
                admission
                    .admit("AngosLocalInstance", "team", &o)
                    .await
                    .map_err(|d| d.reason)
            }
        };
        assert_eq!(admit(updated("1.12.3", None)).await, Ok(()));
        assert_eq!(
            admit(updated("1.12.2", None)).await,
            Err(ReasonCode::UpgradeNotAllowed)
        );
        assert_eq!(admit(updated("1.12.2", Some("1.12.2"))).await, Ok(()));
    }

    #[tokio::test]
    async fn invalid_specs_and_other_kinds_are_refused() {
        assert_eq!(
            admit(
                FakeCredentials::default(),
                json!({"storage": {"fs": {}}, "image": {"repository": "evil/x"}})
            )
            .await,
            Err(ReasonCode::InvalidSpec)
        );
        assert!(
            admission(FakeCredentials::default())
                .admit("Other", "team", &object(json!({})))
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn a_repository_s_spec_is_checked_without_its_instance() {
        let admission = admission(FakeCredentials::default());
        let repo = |spec: serde_json::Value| -> DynamicObject {
            serde_json::from_value(json!({
                "apiVersion": "angos.weebo.io/v1alpha1", "kind": "AngosRepository",
                "metadata": {"name": "app", "namespace": "team"}, "spec": spec,
            }))
            .unwrap()
        };
        assert!(
            admission
                .admit(
                    "AngosRepository",
                    "team",
                    &repo(json!({"prefix": "team", "access": {"pull": ["anonymous"]}}))
                )
                .await
                .is_ok()
        );
        let denied = admission
            .admit(
                "AngosRepository",
                "team",
                &repo(json!({"prefix": "team", "access": {"admin": [], "pull": ["nobody"]}})),
            )
            .await
            .unwrap_err();
        assert_eq!(denied.reason, ReasonCode::InvalidSpec);
    }

    struct Unreachable;

    #[async_trait::async_trait]
    impl AttachedInstances for Unreachable {
        async fn resolve_attached(
            &self,
            _: &str,
            _: Option<&weebo_kit_api::InstanceRef>,
            _: Option<&application::ports::BoundInstance>,
        ) -> Result<application::ports::BoundInstance, InstanceError> {
            Err(InstanceError::Kubernetes("timeout".into()))
        }

        async fn instance(
            &self,
            _: &application::ports::BoundInstance,
        ) -> Result<Option<LocalInstance>, application::ports::ApiError> {
            Ok(None)
        }
    }

    #[tokio::test]
    async fn a_repository_whose_instance_can_t_be_read_is_refused() {
        let admission = AngosAdmission {
            attached: Arc::new(Unreachable),
            ..admission(FakeCredentials::default())
        };
        let repo: DynamicObject = serde_json::from_value(json!({
            "apiVersion": "angos.weebo.io/v1alpha1", "kind": "AngosRepository",
            "metadata": {"name": "app", "namespace": "team"},
            "spec": {"prefix": "team", "access": {"pull": ["anonymous"]}},
        }))
        .unwrap();
        let denied = admission
            .admit("AngosRepository", "team", &repo)
            .await
            .unwrap_err();
        assert_eq!(denied.reason, ReasonCode::KubernetesApiError);
    }
}
