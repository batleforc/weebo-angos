//! The `AngosIdentity`s bound to an instance, as its configuration's
//! `[auth.identity.<username>]` tables (spec 4.4): the hash their own
//! Secret holds, read with the credential label. Two identities with one
//! username: the one in service keeps it, then the older; a username equal
//! to an OIDC provider's name (the instance's `auth.oidc` or
//! `auth.kubernetes`) is refused — Angos reads `Basic <provider>:<jwt>` as
//! a token.

use std::collections::{BTreeMap, BTreeSet};

use api::identity::{keys, password_secret};
use api::status::{IdentityRender, RenderState};
use k8s_openapi::jiff::Timestamp;

use crate::ports::{CredentialError, Credentials};

/// An identity as the instance's pass sees it.
#[derive(Debug, Clone)]
pub struct IdentityView {
    pub namespace: String,
    pub name: String,
    pub created: Option<Timestamp>,
    /// `spec.username`, or the name.
    pub username: String,
}

impl IdentityView {
    pub fn key(&self) -> String {
        format!("{}/{}", self.namespace, self.name)
    }
}

/// The identities kept, and, for each refused one, why.
pub fn conflicts(
    views: &[IdentityView],
    in_service: &BTreeSet<String>,
    providers: &BTreeSet<String>,
) -> BTreeMap<String, (Option<String>, String)> {
    let mut ordered: Vec<&IdentityView> = views.iter().collect();
    ordered.sort_by(|a, b| {
        (!in_service.contains(&a.key()))
            .cmp(&!in_service.contains(&b.key()))
            .then(a.created.cmp(&b.created))
            .then_with(|| a.key().cmp(&b.key()))
    });
    let mut kept: BTreeMap<&str, String> = BTreeMap::new();
    let mut lost = BTreeMap::new();
    for view in ordered {
        if providers.contains(&view.username) {
            lost.insert(
                view.key(),
                (
                    None,
                    format!(
                        "the username {:?} is an OIDC provider's name on the instance",
                        view.username
                    ),
                ),
            );
        } else if let Some(winner) = kept.get(view.username.as_str()) {
            lost.insert(
                view.key(),
                (
                    Some(winner.clone()),
                    format!("the username {:?} is {winner}'s", view.username),
                ),
            );
        } else {
            kept.insert(&view.username, view.key());
        }
    }
    lost
}

/// An identity's hash and version, from its own Secret.
pub async fn hash_of(
    view: &IdentityView,
    credentials: &dyn Credentials,
) -> Result<(String, u64), CredentialError> {
    let secret = password_secret(&view.name);
    let hash = credentials
        .read(&view.namespace, &secret, keys::HASH)
        .await?;
    let version = credentials
        .read(&view.namespace, &secret, keys::VERSION)
        .await?
        .trim()
        .parse()
        .unwrap_or(1);
    Ok((hash, version))
}

/// The render of an identity that isn't in the configuration.
pub fn refused(
    view: &IdentityView,
    state: RenderState,
    message: String,
    conflict_with: Option<String>,
) -> IdentityRender {
    IdentityRender {
        username: view.username.clone(),
        state,
        version: None,
        message: Some(message),
        conflict_with,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn view(ns: &str, name: &str, created: &str, username: &str) -> IdentityView {
        IdentityView {
            namespace: ns.into(),
            name: name.into(),
            created: Some(created.parse().unwrap()),
            username: username.into(),
        }
    }

    #[test]
    fn a_username_belongs_to_one_identity_and_never_to_a_provider() {
        let views = [
            view("b", "ci", "2026-01-02T00:00:00Z", "ci"),
            view("a", "ci", "2026-01-01T00:00:00Z", "ci"),
            view("a", "kube", "2026-01-01T00:00:00Z", "kube"),
        ];
        let providers = BTreeSet::from(["kube".to_string()]);
        let lost = conflicts(&views, &BTreeSet::new(), &providers);
        assert_eq!(lost["b/ci"].0.as_deref(), Some("a/ci"));
        assert!(lost["a/kube"].1.contains("OIDC provider"));
        assert!(!lost.contains_key("a/ci"));
        // In service, `b/ci` keeps it.
        let lost = conflicts(&views, &BTreeSet::from(["b/ci".to_string()]), &providers);
        assert_eq!(lost["a/ci"].0.as_deref(), Some("b/ci"));
    }
}
