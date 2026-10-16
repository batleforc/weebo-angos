//! The validating webhook (`POST /validate`, the kit's router): an
//! `AngosLocalInstance` is refused when Angos would refuse its
//! configuration, when its pods break the operator's rules, or when a
//! Secret it names exists without `angos.weebo.io/credential: "true"`
//! (spec 2.2, 2.8). A Secret that doesn't exist yet is let through: the
//! reconcile waits for it (`SecretMissing`).

use std::sync::Arc;

use api::LocalInstance;
use application::admission::{check_instance, referenced_secrets};
use application::ports::Credentials;
use domain::error::ReasonCode;
use kube::core::DynamicObject;
use weebo_kit_runtime::admission::{Admission, Denial, typed};

pub struct AngosAdmission {
    pub credentials: Arc<dyn Credentials>,
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
        if kind != "AngosLocalInstance" {
            return Err(Denial::new(
                ReasonCode::InvalidSpec,
                format!("kind {kind} is not one the operator admits"),
            ));
        }
        let instance: LocalInstance =
            typed(object).map_err(|e| Denial::new(ReasonCode::InvalidSpec, e))?;
        let name = object.metadata.name.clone().unwrap_or_default();
        check_instance(
            &name,
            namespace,
            &instance.spec,
            &self.allowed_images,
            &self.default_image,
        )
        .map_err(|f| Denial::new(f.reason, f.message))?;
        let image = application::workload::image(&instance.spec, &self.default_image);
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

#[cfg(test)]
mod tests {
    use application::test_support::FakeCredentials;
    use serde_json::json;

    use super::*;

    fn admission(credentials: FakeCredentials) -> AngosAdmission {
        AngosAdmission {
            credentials: Arc::new(credentials),
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
}
