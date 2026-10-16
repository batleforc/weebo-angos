//! [`RepositoryDirectory`] and [`IdentityDirectory`] on the Kubernetes API:
//! the `AngosRepository`s (and `AngosIdentity`s) in
//! the operator's scope recording an instance in `status.instance` (their
//! own controller binds them), kept only when the instance still accepts
//! them — its `repositorySelector.selector` matches their labels, and they
//! are in its namespace or one its `repositorySelector.namespaces`
//! accepts (a consent withdrawn after the binding takes them out).

use std::collections::BTreeMap;

use api::local_instance::RepositorySelector;
use api::{Identity, Repository};
use application::identities::IdentityView;
use application::ports::{IdentityDirectory, Owner, RepositoryDirectory};
use application::repositories::RepositoryView;
use k8s_openapi::api::core::v1::Namespace;
use kube::api::{Api, ListParams};
use kube::core::{Selector, SelectorExt};
use kube::{Client, ResourceExt};
use weebo_kit_api::BoundInstance;
use weebo_kit_application::sharing::shares_with;

pub struct KubeRepositories {
    client: Client,
    watched_namespace: Option<String>,
    selector: Option<Selector>,
}

impl KubeRepositories {
    pub fn new(client: Client) -> Self {
        Self {
            client,
            watched_namespace: None,
            selector: None,
        }
    }

    /// The operator's watch scope.
    pub fn with_scope(
        mut self,
        watched_namespace: Option<String>,
        selector: Option<Selector>,
    ) -> Self {
        self.watched_namespace = watched_namespace;
        self.selector = selector;
        self
    }
}

/// What the directory needs of a kind bound to an instance.
trait Bound:
    kube::Resource<DynamicType = (), Scope = k8s_openapi::NamespaceResourceScope>
    + Clone
    + std::fmt::Debug
    + serde::de::DeserializeOwned
    + Send
    + Sync
    + 'static
{
    fn bound(&self) -> Option<&BoundInstance>;
}

impl Bound for Repository {
    fn bound(&self) -> Option<&BoundInstance> {
        self.status.as_ref()?.common.instance.as_ref()
    }
}

impl Bound for Identity {
    fn bound(&self) -> Option<&BoundInstance> {
        self.status.as_ref()?.common.instance.as_ref()
    }
}

#[async_trait::async_trait]
impl RepositoryDirectory for KubeRepositories {
    async fn bound_to(
        &self,
        instance: &Owner,
        selector: &RepositorySelector,
    ) -> Result<Vec<RepositoryView>, String> {
        Ok(self
            .bound::<Repository>(instance, selector)
            .await?
            .into_iter()
            .map(|repo| RepositoryView {
                namespace: repo.namespace().unwrap_or_default(),
                name: repo.name_any(),
                created: repo.metadata.creation_timestamp.as_ref().map(|t| t.0),
                generation: repo.metadata.generation,
                annotations: repo.annotations().clone(),
                spec: repo.spec.clone(),
                deleting: repo.metadata.deletion_timestamp.is_some(),
            })
            .collect())
    }
}

#[async_trait::async_trait]
impl IdentityDirectory for KubeRepositories {
    async fn bound_to(
        &self,
        instance: &Owner,
        selector: &RepositorySelector,
    ) -> Result<Vec<IdentityView>, String> {
        Ok(self
            .bound::<Identity>(instance, selector)
            .await?
            .into_iter()
            .map(|identity| IdentityView {
                namespace: identity.namespace().unwrap_or_default(),
                name: identity.name_any(),
                created: identity.metadata.creation_timestamp.as_ref().map(|t| t.0),
                username: identity
                    .spec
                    .username
                    .clone()
                    .unwrap_or_else(|| identity.name_any()),
            })
            .collect())
    }
}

impl KubeRepositories {
    async fn bound<K: Bound>(
        &self,
        instance: &Owner,
        selector: &RepositorySelector,
    ) -> Result<Vec<K>, String> {
        let api: Api<K> = match &self.watched_namespace {
            Some(ns) => Api::namespaced(self.client.clone(), ns),
            None => Api::all(self.client.clone()),
        };
        let params = match &self.selector {
            Some(s) => ListParams::default().labels_from(s),
            None => ListParams::default(),
        };
        let listed = api
            .list(&params)
            .await
            .map_err(|e| format!("listing {}: {e}", K::kind(&())))?;
        let bound = BoundInstance::local(instance.namespace.clone(), instance.name.clone());
        let own: Option<Selector> = selector
            .selector
            .clone()
            .map(Selector::try_from)
            .transpose()
            .map_err(|e| format!("repositorySelector.selector: {e}"))?;
        let shared = selector.namespaces.shared_with();
        let mut labels_of: BTreeMap<String, Option<BTreeMap<String, String>>> = BTreeMap::new();
        let mut out = Vec::new();
        for repo in listed.items {
            if repo.bound() != Some(&bound) {
                continue;
            }
            let namespace = repo.namespace().unwrap_or_default();
            if let Some(own) = &own
                && !own.matches(repo.labels())
            {
                continue;
            }
            if namespace != instance.namespace {
                if !labels_of.contains_key(&namespace) {
                    let labels = Api::<Namespace>::all(self.client.clone())
                        .get_opt(&namespace)
                        .await
                        .map_err(|e| format!("reading namespace {namespace}: {e}"))?
                        .map(|n| n.labels().clone());
                    labels_of.insert(namespace.clone(), labels);
                }
                let labels = labels_of[&namespace].as_ref();
                if !shares_with(shared.as_ref(), Some(&namespace), labels) {
                    continue;
                }
            }
            out.push(repo);
        }
        Ok(out)
    }
}
