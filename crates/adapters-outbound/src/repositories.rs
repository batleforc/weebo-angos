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
use weebo_kit_adapters::instance::STORE_READY_TIMEOUT;
use weebo_kit_api::BoundInstance;
use weebo_kit_application::sharing::shares_with;
use weebo_kit_runtime::shared::SharedWatches;

pub struct KubeRepositories {
    client: Client,
    watched_namespace: Option<String>,
    selector: Option<Selector>,
    /// The binary's shared watches: each kind read from its store once
    /// listed, from the API otherwise.
    watches: SharedWatches,
}

impl KubeRepositories {
    pub fn new(client: Client) -> Self {
        Self {
            client,
            watched_namespace: None,
            selector: None,
            watches: SharedWatches::default(),
        }
    }

    /// Read the repositories and identities from the binary's shared
    /// watches (on the scope of [`Self::with_scope`]) instead of a LIST on
    /// every instance pass.
    pub fn with_watches(mut self, watches: SharedWatches) -> Self {
        self.watches = watches;
        self
    }

    /// Every `K` in the operator's scope: from its shared watch once it
    /// has listed (an unsynced store is never read as empty), from the API
    /// otherwise; sorted as the API server would.
    async fn list<K: Bound>(&self) -> Result<Vec<K>, String> {
        if let Some(shared) = self.watches.get::<K>() {
            let store = shared.store();
            if weebo_kit_adapters::wait_synced(&store, STORE_READY_TIMEOUT).await {
                let mut all: Vec<K> = store.state().iter().map(|o| K::clone(o)).collect();
                all.sort_by_key(|o| (o.namespace(), o.name_any()));
                return Ok(all);
            }
        }
        let api: Api<K> = match &self.watched_namespace {
            Some(ns) => Api::namespaced(self.client.clone(), ns),
            None => Api::all(self.client.clone()),
        };
        let params = match &self.selector {
            Some(s) => ListParams::default().labels_from(s),
            None => ListParams::default(),
        };
        api.list(&params)
            .await
            .map(|l| l.items)
            .map_err(|e| format!("listing {}: {e}", K::kind(&())))
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
    async fn aiming_at(&self, instance: &Owner) -> Result<Vec<String>, String> {
        let listed = self.list::<Repository>().await?;
        let bound = BoundInstance::local(instance.namespace.clone(), instance.name.clone());
        Ok(listed
            .into_iter()
            .filter(|repo| {
                let namespace = repo.namespace().unwrap_or_default();
                match repo.bound() {
                    Some(b) => *b == bound,
                    // Not bound yet: whatever it names, or this namespace's
                    // instance by default.
                    None => match &repo.spec.instance_ref {
                        Some(r) => {
                            r.name == instance.name
                                && r.namespace.as_deref().unwrap_or(&namespace)
                                    == instance.namespace
                        }
                        None => namespace == instance.namespace,
                    },
                }
            })
            .map(|repo| {
                format!(
                    "{}/{}",
                    repo.namespace().unwrap_or_default(),
                    repo.name_any()
                )
            })
            .collect())
    }

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
        let listed = self.list::<K>().await?;
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
        for repo in listed {
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
