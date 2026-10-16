//! The `AngosRepository`s bound to an instance, as its configuration's
//! `[repository."<prefix>"]` tables (spec 4.2). Rendered by the instance's
//! pass; a repository's own controller only checks, binds and reflects.
//!
//! - Two repositories whose prefixes overlap (`team` and `team/app`, the
//!   same one included): the older (`creationTimestamp`, then
//!   `namespace/name`) is rendered, the other is `PrefixConflict`.
//! - Their Secrets are read in their own namespace, with the credential
//!   label; one missing leaves the repository out (and, beware, exposed to
//!   a prune: the prune lock covers it, spec 6.6).

use std::collections::BTreeMap;

use api::annotations;
use api::repository::{Downstream, RepositorySpec, Upstream};
use api::status::RenderState;
use domain::config::{self, Policy, RetentionPolicy};
use domain::validation::{belongs_to, is_namespace};
use k8s_openapi::jiff::Timestamp;

use crate::ports::{CelChecker, CredentialError, Credentials};
use crate::rules::{self, Kube, Problems};

/// A repository as the instance's pass sees it.
#[derive(Debug, Clone)]
pub struct RepositoryView {
    pub namespace: String,
    pub name: String,
    pub created: Option<Timestamp>,
    pub generation: Option<i64>,
    pub annotations: BTreeMap<String, String>,
    pub spec: RepositorySpec,
    /// Being deleted (its finalizer holds it).
    pub deleting: bool,
}

impl RepositoryView {
    /// Deleted with `allow-data-deletion: "true"`: out of the configuration.
    pub fn released(&self) -> bool {
        self.deleting
            && self
                .annotations
                .get(annotations::ALLOW_DATA_DELETION)
                .map(String::as_str)
                == Some("true")
    }

    /// `namespace/name`.
    pub fn key(&self) -> String {
        format!("{}/{}", self.namespace, self.name)
    }
}

/// Which repositories keep their prefix: one the instance already renders
/// first (a newcomer never takes a prefix in service, timestamps being to
/// the second), then the older of two that overlap. Returns, for each
/// loser, the winner it conflicts with.
pub fn conflicts(
    views: &[RepositoryView],
    rendered: &std::collections::BTreeSet<String>,
) -> BTreeMap<String, String> {
    let mut ordered: Vec<&RepositoryView> = views.iter().collect();
    ordered.sort_by(|a, b| {
        (!rendered.contains(&a.key()))
            .cmp(&!rendered.contains(&b.key()))
            .then(a.created.cmp(&b.created))
            .then_with(|| a.key().cmp(&b.key()))
    });
    let mut kept: Vec<&RepositoryView> = Vec::new();
    let mut lost = BTreeMap::new();
    for view in ordered {
        let p = &view.spec.prefix;
        match kept
            .iter()
            .find(|k| belongs_to(p, &k.spec.prefix) || belongs_to(&k.spec.prefix, p))
        {
            Some(winner) => {
                lost.insert(view.key(), winner.key());
            }
            None => kept.push(view),
        }
    }
    lost
}

/// What only the repository's spec decides: its rules, regexes and the
/// consent `prune` needs. Warnings (`RuleMayFail`) on success.
pub fn validate(
    spec: &RepositorySpec,
    annotations: &BTreeMap<String, String>,
    kube: Kube<'_>,
    checker: &dyn CelChecker,
) -> Result<(config::Repository, Vec<String>), Problems> {
    let mut problems = Problems::new();
    let mut warnings = Vec::new();
    if !is_namespace(&spec.prefix) {
        problems.push(format!(
            "spec.prefix: {:?} is not an OCI namespace",
            spec.prefix
        ));
    }
    if !spec.upstreams.is_empty() && !(spec.access.push.is_empty() && spec.access.delete.is_empty())
    {
        problems
            .push("spec.access: a pull-through repository (upstreams) refuses every push".into());
    }
    let deletion_allowed = annotations
        .get(annotations::ALLOW_DATA_DELETION)
        .map(String::as_str)
        == Some("true");
    for (i, d) in spec.downstreams.iter().enumerate() {
        if d.prune && !deletion_allowed {
            problems.push(format!(
                "spec.downstreams[{i}].prune propagates deletions: it needs the annotation {}: \"true\"",
                annotations::ALLOW_DATA_DELETION
            ));
        }
    }
    let mut repo = config::Repository {
        namespace: spec.mirror_of.clone(),
        immutable_tags: spec.immutable_tags,
        immutable_tags_exclusions: spec.immutable_tags_exclusions.clone(),
        ..Default::default()
    };
    match rules::repository_access(&spec.access, kube, checker) {
        Ok(checked) => {
            warnings.extend(checked.warnings);
            repo.access_policy = Some(Policy {
                default: Some(config::AccessMode::Deny),
                rules: checked.rules,
            });
        }
        Err(more) => problems.extend(more),
    }
    if let Some(retention) = &spec.retention {
        match rules::image_rules("spec.retention.rules", &retention.rules, checker) {
            Ok(checked) => {
                warnings.extend(checked.warnings);
                repo.retention_policy = Some(RetentionPolicy {
                    rules: checked.rules,
                });
            }
            Err(more) => problems.extend(more),
        }
    }
    if let Some(index) = &spec.index {
        match rules::index_policy("spec.index", index, checker) {
            Ok((policy, more)) => {
                warnings.extend(more);
                repo.index = Some(policy);
            }
            Err(more) => problems.extend(more),
        }
    }
    if problems.is_empty() {
        Ok((repo, warnings))
    } else {
        Err(problems)
    }
}

async fn basic(
    credentials: &dyn Credentials,
    namespace: &str,
    r: Option<&api::repository::BasicCredentials>,
) -> Result<(Option<String>, Option<String>), CredentialError> {
    let Some(r) = r else {
        return Ok((None, None));
    };
    Ok((
        Some(
            credentials
                .read(namespace, &r.name, &r.username_key)
                .await?,
        ),
        Some(
            credentials
                .read(namespace, &r.name, &r.password_key)
                .await?,
        ),
    ))
}

async fn upstream(
    credentials: &dyn Credentials,
    namespace: &str,
    u: &Upstream,
) -> Result<config::Upstream, CredentialError> {
    let (username, password) =
        basic(credentials, namespace, u.credentials_secret_ref.as_ref()).await?;
    Ok(config::Upstream {
        url: u.url.clone(),
        max_redirect: u.max_redirect,
        connect_timeout_secs: u.connect_timeout_secs,
        read_timeout_secs: u.read_timeout_secs,
        tls: Default::default(),
        username,
        password,
    })
}

async fn downstream(
    credentials: &dyn Credentials,
    namespace: &str,
    d: &Downstream,
) -> Result<config::Downstream, CredentialError> {
    let (username, password) =
        basic(credentials, namespace, d.credentials_secret_ref.as_ref()).await?;
    Ok(config::Downstream {
        name: d.name.clone(),
        client: config::Upstream {
            url: d.url.clone(),
            username,
            password,
            ..Default::default()
        },
        mode: d.mode.map(|m| match m {
            api::repository::ReplicationMode::EventAndReconcile => {
                config::ReplicationMode::EventAndReconcile
            }
            api::repository::ReplicationMode::EventOnly => config::ReplicationMode::EventOnly,
            api::repository::ReplicationMode::ReconcileOnly => {
                config::ReplicationMode::ReconcileOnly
            }
        }),
        namespace_filter: d.namespace_filter.clone(),
        max_concurrent_pushes: d.max_concurrent_pushes,
        prune: d.prune.then_some(true),
    })
}

/// A repository as its table, its Secrets read: or what keeps it out.
pub async fn render(
    view: &RepositoryView,
    kube: Kube<'_>,
    checker: &dyn CelChecker,
    credentials: &dyn Credentials,
) -> Result<(config::Repository, Vec<String>), (RenderState, String)> {
    let (mut repo, warnings) = validate(&view.spec, &view.annotations, kube, checker)
        .map_err(|problems| (RenderState::Invalid, problems.join("; ")))?;
    let refused = |e: CredentialError| {
        let state = match e {
            CredentialError::NotAllowed(_) => RenderState::SecretNotAllowed,
            _ => RenderState::SecretMissing,
        };
        (state, e.to_string())
    };
    for u in &view.spec.upstreams {
        repo.upstream.push(
            upstream(credentials, &view.namespace, u)
                .await
                .map_err(refused)?,
        );
    }
    for d in &view.spec.downstreams {
        repo.downstream.push(
            downstream(credentials, &view.namespace, d)
                .await
                .map_err(refused)?,
        );
    }
    Ok((repo, warnings))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::test_support::{FakeCel, FakeCredentials};

    fn view(ns: &str, name: &str, created: &str, spec: serde_json::Value) -> RepositoryView {
        RepositoryView {
            namespace: ns.into(),
            name: name.into(),
            created: Some(created.parse().unwrap()),
            generation: Some(1),
            annotations: BTreeMap::new(),
            spec: serde_json::from_value(spec).unwrap(),
            deleting: false,
        }
    }

    #[test]
    fn the_older_of_two_overlapping_prefixes_keeps_it() {
        let views = [
            view(
                "b",
                "app",
                "2026-01-02T00:00:00Z",
                json!({"prefix": "team/app"}),
            ),
            view(
                "a",
                "team",
                "2026-01-01T00:00:00Z",
                json!({"prefix": "team"}),
            ),
            view(
                "c",
                "teams",
                "2026-01-03T00:00:00Z",
                json!({"prefix": "teams"}),
            ),
            view(
                "d",
                "same",
                "2026-01-01T00:00:00Z",
                json!({"prefix": "team"}),
            ),
        ];
        let lost = conflicts(&views, &Default::default());
        assert_eq!(
            lost,
            BTreeMap::from([
                ("b/app".to_string(), "a/team".to_string()),
                ("d/same".to_string(), "a/team".to_string()),
            ])
        );
        // In service, `b/app` keeps its prefix against the older `a/team`.
        let lost = conflicts(&views, &["b/app".to_string()].into());
        assert_eq!(lost.get("a/team").map(String::as_str), Some("b/app"));
    }

    #[tokio::test]
    async fn a_pull_through_renders_with_its_credentials() {
        let credentials = FakeCredentials::default();
        credentials.insert("team", "hub", &[("username", "u"), ("password", "p")]);
        let v = view(
            "team",
            "hub",
            "2026-01-01T00:00:00Z",
            json!({
                "prefix": "dockerhub",
                "access": {"pull": ["authenticated"]},
                "upstreams": [{"url": "https://registry-1.docker.io", "credentialsSecretRef": {"name": "hub"}}],
                "retention": {"rules": ["top_pushed(10)"]},
            }),
        );
        let (repo, warnings) = render(&v, None, &FakeCel::default(), &credentials)
            .await
            .unwrap();
        assert!(warnings.is_empty());
        assert_eq!(repo.upstream[0].username.as_deref(), Some("u"));
        assert_eq!(repo.access_policy.unwrap().rules.len(), 1);
        assert_eq!(repo.retention_policy.unwrap().rules, ["top_pushed(10)"]);
        // A Secret of another namespace is not this repository's.
        let other = RepositoryView {
            namespace: "other".into(),
            ..v.clone()
        };
        assert_eq!(
            render(&other, None, &FakeCel::default(), &credentials)
                .await
                .unwrap_err()
                .0,
            RenderState::SecretMissing
        );
    }

    #[test]
    fn what_the_spec_alone_breaks_is_refused() {
        let cel = FakeCel::default();
        let check = |spec: serde_json::Value, annotations: &[(&str, &str)]| {
            let spec: RepositorySpec = serde_json::from_value(spec).unwrap();
            let annotations = annotations
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect();
            validate(&spec, &annotations, None, &cel).map(|_| ())
        };
        assert!(check(json!({"prefix": "Team"}), &[]).is_err());
        assert!(check(
            json!({"prefix": "hub", "upstreams": [{"url": "https://r"}], "access": {"push": ["anonymous"]}}),
            &[]
        )
        .is_err());
        let prune = json!({"prefix": "team", "downstreams": [{"name": "dr", "url": "https://m", "prune": true}]});
        assert!(check(prune.clone(), &[]).is_err());
        assert!(check(prune, &[(annotations::ALLOW_DATA_DELETION, "true")]).is_ok());
    }
}
