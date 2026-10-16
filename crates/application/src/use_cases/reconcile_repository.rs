//! An `AngosRepository` (spec 4.2): bound to its instance (through the
//! instance's consent for another namespace), its own spec checked — the
//! vocabulary, the CEL rules on Angos's engine —, then what the instance
//! made of it reflected: rendered and confirmed by every pod, or why not
//! (`PrefixConflict`, `SecretMissing`, `SecretNotAllowed`, waiting).
//! The instance renders it; this pass writes nothing but the status.

use std::collections::BTreeMap;

use api::repository::RepositorySpec;
use api::status::RenderState;
use domain::error::ReasonCode;
use domain::{Advisory, Failure};

use super::{Outcome, instance_failure};
use crate::ports::{AttachedInstances, BoundInstance, CelChecker};

/// What a pass is about.
pub struct RepositoryPass<'a> {
    pub namespace: &'a str,
    pub name: &'a str,
    pub generation: Option<i64>,
    pub spec: &'a RepositorySpec,
    pub annotations: &'a BTreeMap<String, String>,
    pub recorded: Option<&'a BoundInstance>,
}

/// The outcome, and the status fields of a repository.
#[derive(Debug, Clone)]
pub struct RepositoryReport {
    pub outcome: Outcome,
    pub rendered: bool,
    pub conflict_with: Option<String>,
}

pub async fn reconcile_repository(
    pass: RepositoryPass<'_>,
    instances: &dyn AttachedInstances,
    cel: &dyn CelChecker,
) -> RepositoryReport {
    let mut report = RepositoryReport {
        outcome: Outcome::synced(""),
        rendered: false,
        conflict_with: None,
    };
    let bound = match instances
        .resolve_attached(
            pass.namespace,
            pass.spec.instance_ref.as_ref(),
            pass.recorded,
        )
        .await
    {
        Ok(bound) => bound,
        Err(e) => {
            report.outcome = Outcome::failed(instance_failure(e));
            return report;
        }
    };
    let ready = run(&pass, &bound, instances, cel, &mut report).await;
    let advisories = std::mem::take(&mut report.outcome.advisories);
    report.outcome = match ready {
        Ok(message) => Outcome::synced(message),
        Err(failure) => Outcome::failed(failure),
    }
    .on(bound);
    report.outcome.advisories = advisories;
    report
}

async fn run(
    pass: &RepositoryPass<'_>,
    bound: &BoundInstance,
    instances: &dyn AttachedInstances,
    cel: &dyn CelChecker,
    report: &mut RepositoryReport,
) -> Result<String, Failure> {
    let instance = instances
        .instance(bound)
        .await
        .map_err(|e| Failure::new(ReasonCode::KubernetesApiError, e))?
        .ok_or_else(|| Failure::new(ReasonCode::InstanceNotFound, format!("{bound} is gone")))?;
    let kube = instance
        .spec
        .auth
        .kubernetes
        .as_ref()
        .map(|k| k.name.as_str());
    let (_, warnings) = crate::repositories::validate(pass.spec, pass.annotations, kube, cel)
        .map_err(|problems| Failure::new(ReasonCode::InvalidSpec, problems.join("; ")))?;
    if !warnings.is_empty() {
        report.outcome.advisories.push(Advisory {
            reason: ReasonCode::RuleMayFail,
            message: warnings.join("; "),
        });
    }
    let status = instance.status.clone().unwrap_or_default();
    let key = format!("{}/{}", pass.namespace, pass.name);
    let Some(render) = status.repositories.get(&key) else {
        return Err(Failure::new(
            ReasonCode::ConfigPropagating,
            format!("waiting for {bound} to render it"),
        ));
    };
    let message = render.message.clone().unwrap_or_default();
    match render.state {
        RenderState::Conflict => {
            report.conflict_with = render.conflict_with.clone();
            Err(Failure::new(ReasonCode::PrefixConflict, message))
        }
        RenderState::SecretMissing => Err(Failure::new(ReasonCode::SecretMissing, message)),
        RenderState::SecretNotAllowed => Err(Failure::new(ReasonCode::SecretNotAllowed, message)),
        RenderState::Invalid => Err(Failure::new(ReasonCode::InvalidSpec, message)),
        RenderState::Rendered => {
            report.rendered = true;
            // Confirmed by the instance itself, for this generation: never
            // inferred from its hashes, which move in separate writes.
            if render.applied_generation.is_some() && render.applied_generation == pass.generation {
                Ok(format!("served by {bound} under {}/…", pass.spec.prefix))
            } else {
                Err(Failure::new(
                    ReasonCode::ConfigPropagating,
                    format!("rendered into {bound}, waiting for its pods to load it"),
                ))
            }
        }
    }
}

/// A deleted repository's release (its finalizer, spec 6.6): its data goes
/// at the instance's next prune, so it needs the annotation
/// `angos.weebo.io/allow-data-deletion: "true"`; then it waits until every
/// pod of its instance runs without it.
pub async fn release_repository(
    namespace: &str,
    name: &str,
    prefix: &str,
    annotations: &BTreeMap<String, String>,
    recorded: Option<&BoundInstance>,
    instances: &dyn AttachedInstances,
) -> Result<(), Failure> {
    if annotations
        .get(api::annotations::ALLOW_DATA_DELETION)
        .map(String::as_str)
        != Some("true")
    {
        return Err(Failure::new(
            ReasonCode::DeletionNotAllowed,
            format!(
                "the data under {prefix}/ would be erased at the instance's next prune: annotate \
                 the repository {}=\"true\" to confirm, or recreate it",
                api::annotations::ALLOW_DATA_DELETION
            ),
        ));
    }
    let Some(bound) = recorded else {
        return Ok(());
    };
    let Some(instance) = instances
        .instance(bound)
        .await
        .map_err(|e| Failure::new(ReasonCode::KubernetesApiError, e))?
    else {
        return Ok(());
    };
    let status = instance.status.unwrap_or_default();
    let key = format!("{namespace}/{name}");
    let gone = !status.repositories.contains_key(&key);
    let confirmed = status.applied_config_hash == status.config_hash;
    if gone && confirmed {
        Ok(())
    } else {
        Err(Failure::new(
            ReasonCode::ConfigPropagating,
            format!("waiting for {bound} to run without it"),
        ))
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use api::LocalInstance;
    use api::status::{InstanceStatus, RepositoryRender};
    use serde_json::json;

    use super::*;
    use crate::ports::InstanceError;
    use crate::test_support::FakeCel;

    struct Instances(Mutex<Option<LocalInstance>>);

    #[async_trait::async_trait]
    impl AttachedInstances for Instances {
        async fn resolve_attached(
            &self,
            namespace: &str,
            _: Option<&weebo_kit_api::InstanceRef>,
            _: Option<&BoundInstance>,
        ) -> Result<BoundInstance, InstanceError> {
            Ok(BoundInstance::local(namespace, "reg"))
        }

        async fn instance(&self, _: &BoundInstance) -> Result<Option<LocalInstance>, String> {
            Ok(self.0.lock().unwrap().clone())
        }
    }

    fn instance(render: Option<RepositoryRender>, applied: bool) -> Instances {
        let mut instance = LocalInstance::new("reg", Default::default());
        let mut status = InstanceStatus {
            config_hash: Some("h".into()),
            applied_config_hash: Some(if applied { "h" } else { "old" }.into()),
            ..Default::default()
        };
        if let Some(mut render) = render {
            if !applied {
                render.applied_generation = None;
            }
            status.repositories.insert("team/app".into(), render);
        }
        instance.status = Some(status);
        Instances(Mutex::new(Some(instance)))
    }

    fn render(state: RenderState) -> RepositoryRender {
        RepositoryRender {
            prefix: "team/app".into(),
            state,
            message: Some("why".into()),
            conflict_with: (state == RenderState::Conflict).then(|| "team/team".into()),
            generation: Some(1),
            applied_generation: Some(1),
        }
    }

    async fn pass(instances: &Instances, spec: serde_json::Value) -> RepositoryReport {
        let spec: RepositorySpec = serde_json::from_value(spec).unwrap();
        reconcile_repository(
            RepositoryPass {
                namespace: "team",
                name: "app",
                generation: Some(1),
                spec: &spec,
                annotations: &BTreeMap::new(),
                recorded: None,
            },
            instances,
            &FakeCel::default(),
        )
        .await
    }

    fn spec() -> serde_json::Value {
        json!({"prefix": "team/app", "access": {"pull": ["anonymous"]}})
    }

    #[tokio::test]
    async fn a_rendered_and_confirmed_repository_is_ready() {
        let report = pass(&instance(Some(render(RenderState::Rendered)), true), spec()).await;
        assert_eq!(report.outcome.reason(), ReasonCode::Reconciled);
        assert!(report.rendered);
        assert_eq!(
            report.outcome.instance,
            Some(BoundInstance::local("team", "reg"))
        );
        let waiting = pass(
            &instance(Some(render(RenderState::Rendered)), false),
            spec(),
        )
        .await;
        assert_eq!(waiting.outcome.reason(), ReasonCode::ConfigPropagating);
        let unseen = pass(&instance(None, true), spec()).await;
        assert_eq!(unseen.outcome.reason(), ReasonCode::ConfigPropagating);
    }

    #[tokio::test]
    async fn what_the_instance_refused_is_reflected() {
        for (state, reason) in [
            (RenderState::Conflict, ReasonCode::PrefixConflict),
            (RenderState::SecretMissing, ReasonCode::SecretMissing),
            (RenderState::SecretNotAllowed, ReasonCode::SecretNotAllowed),
            (RenderState::Invalid, ReasonCode::InvalidSpec),
        ] {
            let report = pass(&instance(Some(render(state)), true), spec()).await;
            assert_eq!(report.outcome.reason(), reason, "{state:?}");
        }
        let conflict = pass(&instance(Some(render(RenderState::Conflict)), true), spec()).await;
        assert_eq!(conflict.conflict_with.as_deref(), Some("team/team"));
    }

    #[tokio::test]
    async fn a_deleted_repository_waits_for_consent_then_for_its_instance() {
        let annotations = |allowed: bool| {
            let mut a = BTreeMap::new();
            if allowed {
                a.insert(
                    api::annotations::ALLOW_DATA_DELETION.to_string(),
                    "true".to_string(),
                );
            }
            a
        };
        let bound = BoundInstance::local("team", "reg");
        let still = instance(Some(render(RenderState::Rendered)), true);
        let refused = release_repository(
            "team",
            "app",
            "team/app",
            &annotations(false),
            Some(&bound),
            &still,
        )
        .await
        .unwrap_err();
        assert_eq!(refused.reason, ReasonCode::DeletionNotAllowed);
        let waiting = release_repository(
            "team",
            "app",
            "team/app",
            &annotations(true),
            Some(&bound),
            &still,
        )
        .await
        .unwrap_err();
        assert_eq!(waiting.reason, ReasonCode::ConfigPropagating);
        let gone = instance(None, true);
        assert!(
            release_repository(
                "team",
                "app",
                "team/app",
                &annotations(true),
                Some(&bound),
                &gone
            )
            .await
            .is_ok()
        );
    }

    #[tokio::test]
    async fn its_own_spec_is_checked_first() {
        let report = pass(
            &instance(Some(render(RenderState::Rendered)), true),
            json!({"prefix": "team/app", "access": {"pull": ["namespace:ci"]}}),
        )
        .await;
        assert_eq!(report.outcome.reason(), ReasonCode::InvalidSpec);
        assert!(
            report
                .outcome
                .ready
                .unwrap_err()
                .message
                .contains("auth.kubernetes")
        );
        let gone = Instances(Mutex::new(None));
        assert_eq!(
            pass(&gone, spec()).await.outcome.reason(),
            ReasonCode::InstanceNotFound
        );
    }
}
