//! What the admission webhook refuses of an `AngosLocalInstance` before it
//! exists (spec 2.2, 2.8): a configuration Angos would refuse, pods the
//! operator doesn't run, a Secret that doesn't opt in. The reconcile checks
//! the same again (a Secret may change after admission).
//!
//! The Secrets' values aren't read here: placeholders stand in for them,
//! so what depends on a value (a token key's length…) is the reconcile's.

use std::collections::BTreeSet;

use api::local_instance::LocalInstanceSpec;
use domain::Failure;
use domain::error::ReasonCode;
use weebo_kit_application::pod_safety;

use crate::ports::CelChecker;
use crate::render::{self, Resolved};
use crate::rules;
use crate::workload::{self, sketch};

/// A stand-in for every value the spec takes from a Secret.
pub fn placeholders(spec: &LocalInstanceSpec) -> Resolved {
    let pair = || Some(("placeholder".to_string(), "placeholder".to_string()));
    Resolved {
        s3: spec.storage.s3.as_ref().and_then(|_| pair()),
        metadata_s3: spec
            .storage
            .metadata
            .as_ref()
            .and_then(|m| m.s3.as_ref())
            .and_then(|_| pair()),
        redis_url: spec
            .cache
            .as_ref()
            .map(|_| "redis://placeholder:6379".into()),
        // 33 bytes once decoded: long enough.
        token_key: spec.auth.token_service.as_ref().map(|_| "A".repeat(44)),
        oidc: spec
            .auth
            .oidc
            .iter()
            .map(|p| {
                (
                    p.name.clone(),
                    (
                        "https://issuer.placeholder/".to_string(),
                        "placeholder".to_string(),
                    ),
                )
            })
            .collect(),
        ui_client_id: Some("placeholder".into()),
        cluster_issuer: Some("https://kubernetes.default.svc".into()),
        scan_token: spec
            .scan
            .as_ref()
            .and_then(|s| s.token_secret_ref.as_ref())
            .map(|_| "placeholder".into()),
        event_tokens: spec
            .event_webhooks
            .iter()
            .map(|w| (w.name.clone(), "placeholder".to_string()))
            .collect(),
        webhook_auth: spec
            .auth
            .webhooks
            .iter()
            .map(|w| {
                (
                    w.name.clone(),
                    render::WebhookAuth {
                        bearer_token: w
                            .bearer_token_secret_ref
                            .as_ref()
                            .map(|_| "placeholder".into()),
                        basic: w.basic_auth_secret_ref.as_ref().and_then(|_| pair()),
                    },
                )
            })
            .collect(),
        ..Default::default()
    }
}

/// The Secrets the spec names: what it reads and what its pods mount.
pub fn referenced_secrets(
    name: &str,
    namespace: &str,
    spec: &LocalInstanceSpec,
    image: &str,
) -> BTreeSet<String> {
    let mut secrets = pod_safety::secrets_to_check(&sketch(name, namespace, spec, image));
    let mut add = |n: &str| {
        secrets.insert(n.to_string());
    };
    for s3 in [
        spec.storage.s3.as_ref(),
        spec.storage.metadata.as_ref().and_then(|m| m.s3.as_ref()),
    ]
    .into_iter()
    .flatten()
    {
        add(&s3.credentials_secret_ref.name);
    }
    if let Some(cache) = &spec.cache {
        add(&cache.redis.url_secret_ref.name);
    }
    for provider in &spec.auth.oidc {
        for r in [&provider.issuer_from, &provider.audience_from]
            .into_iter()
            .flatten()
        {
            add(&r.name);
        }
    }
    if let Some(r) = spec
        .ui
        .as_ref()
        .and_then(|ui| ui.oidc.as_ref())
        .and_then(|o| o.client_id_from.as_ref())
    {
        add(&r.name);
    }
    if let Some(r) = spec
        .auth
        .token_service
        .as_ref()
        .and_then(|t| t.key_secret_ref.as_ref())
    {
        add(&r.name);
    }
    if let Some(r) = spec.scan.as_ref().and_then(|s| s.token_secret_ref.as_ref()) {
        add(&r.name);
    }
    for webhook in &spec.event_webhooks {
        add(&webhook.token_secret_ref.name);
    }
    for webhook in &spec.auth.webhooks {
        if let Some(r) = &webhook.bearer_token_secret_ref {
            add(&r.name);
        }
        if let Some(r) = &webhook.basic_auth_secret_ref {
            add(&r.name);
        }
    }
    secrets
}

/// `Err(InvalidSpec)` when Angos would refuse the configuration, or the
/// pods break the operator's rules.
pub fn check_instance(
    name: &str,
    namespace: &str,
    spec: &LocalInstanceSpec,
    allowed_images: &[String],
    default_image: &str,
    checker: &dyn CelChecker,
) -> Result<(), Failure> {
    let invalid =
        |problems: Vec<String>| Failure::new(ReasonCode::InvalidSpec, problems.join("; "));
    let kube = spec.auth.kubernetes.as_ref().map(|k| k.name.as_str());
    let mut resolved = placeholders(spec);
    resolved.global_rules = rules::global_access(&spec.global.access, kube, checker)
        .map_err(invalid)?
        .rules;
    resolved.admin_rule = rules::admin_rule(&spec.global.access, kube);
    if let Some(retention) = &spec.global.retention {
        rules::image_rules("spec.global.retention.rules", &retention.rules, checker)
            .map_err(invalid)?;
    }
    if let Some(index) = &spec.global.index {
        rules::index_policy("spec.global.index", index, checker).map_err(invalid)?;
    }
    if let Some(scan) = &spec.scan {
        resolved.scan = Some(
            rules::scan_policy("spec.scan", scan.default, &scan.rules, checker)
                .map_err(invalid)?
                .0,
        );
    }
    render::render(spec, &resolved).map_err(|e| {
        Failure::new(
            ReasonCode::InvalidSpec,
            format!("the configuration Angos would refuse: {e}"),
        )
    })?;
    let image = workload::image(spec, default_image);
    let problems = pod_safety::problems(&sketch(name, namespace, spec, &image), allowed_images);
    if !problems.is_empty() {
        return Err(Failure::new(
            ReasonCode::InvalidSpec,
            format!(
                "the instance asks for pods the operator doesn't run: {}. An instance runs Angos's \
                 image (the operator's allow-list), without hostPath, privilege or added capability",
                pod_safety::summary(&problems)
            ),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    const IMAGE: &str = "ghcr.io/project-angos/angos:1.12.3";

    fn allowed() -> Vec<String> {
        vec![api::angos::IMAGE_REPOSITORY.to_string()]
    }

    fn check(spec: serde_json::Value) -> Result<(), ReasonCode> {
        let spec: LocalInstanceSpec = serde_json::from_value(spec).unwrap();
        check_instance(
            "reg",
            "team",
            &spec,
            &allowed(),
            IMAGE,
            &crate::test_support::FakeCel::default(),
        )
        .map_err(|f| f.reason)
    }

    #[test]
    fn a_complete_instance_is_admitted_on_placeholders() {
        assert_eq!(
            check(json!({
                "storage": {"s3": {"endpoint": "http://s3", "bucket": "b", "region": "r",
                    "credentialsSecretRef": {"name": "s3"}}},
                "cache": {"redis": {"urlSecretRef": {"name": "r", "key": "url"}, "keyPrefix": "a:"}},
                "auth": {
                    "oidc": [{"name": "weebo", "issuerFrom": {"name": "a", "key": "i"}, "audience": "angos"}],
                    "kubernetes": {"audience": "angos"},
                    "tokenService": {"realm": "https://r.example/token"},
                },
                "ui": {"enabled": true, "oidc": {"provider": "weebo", "clientIdFrom": {"name": "a", "key": "c"}}},
            })),
            Ok(())
        );
    }

    #[test]
    fn what_angos_or_the_operator_refuse_is_refused() {
        for refused in [
            json!({"storage": {"fs": {}}, "global": {"limits": {"maxManifestSize": "0MiB"}}}),
            json!({"storage": {"fs": {}}, "image": {"repository": "evil/miner"}}),
            json!({"storage": {"fs": {}}, "securityContext": {"privileged": true}}),
            json!({"storage": {"fs": {}}, "extraVolumes": [{"name": "h", "hostPath": {"path": "/"}}]}),
            json!({"storage": {"fs": {}}, "extraConfig": {"server": {"port": 1}}}),
            json!({"storage": {"fs": {}}, "ui": {"oidc": {"provider": "none", "clientId": "x"}}}),
            json!({"storage": {"fs": {}}, "global": {"access": {"admin": ["anonymous"]}}}),
        ] {
            assert_eq!(
                check(refused.clone()),
                Err(ReasonCode::InvalidSpec),
                "{refused}"
            );
        }
    }

    #[test]
    fn every_secret_the_spec_names_is_listed() {
        let spec: LocalInstanceSpec = serde_json::from_value(json!({
            "storage": {"s3": {"endpoint": "http://s3", "bucket": "b", "region": "r",
                "credentialsSecretRef": {"name": "s3"}}},
            "server": {"tls": {"secretName": "tls", "clientCA": {"name": "ca", "key": "ca.crt"}}},
            "cache": {"redis": {"urlSecretRef": {"name": "redis", "key": "url"}, "keyPrefix": "a:"}},
            "auth": {"tokenService": {"realm": "https://r.example/token", "keySecretRef": {"name": "key", "key": "k"}}},
        }))
        .unwrap();
        assert_eq!(
            referenced_secrets("reg", "team", &spec, IMAGE)
                .into_iter()
                .collect::<Vec<_>>(),
            ["ca", "key", "redis", "s3", "tls"]
        );
    }
}
