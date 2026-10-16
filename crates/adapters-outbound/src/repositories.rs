//! [`RepositoryDirectory`] on the Kubernetes API: the `AngosRepository`s in
//! the operator's scope recording an instance in `status.instance` (their
//! own controller binds them), kept only when the instance still accepts
//! them — its `repositorySelector.selector` matches their labels, and they
//! are in its namespace or one its `repositorySelector.namespaces`
//! accepts (a consent withdrawn after the binding takes them out).

use std::collections::BTreeMap;

use api::Repository;
use api::local_instance::RepositorySelector;
use application::ports::{Owner, RepositoryDirectory};
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

#[async_trait::async_trait]
impl RepositoryDirectory for KubeRepositories {
    async fn bound_to(
        &self,
        instance: &Owner,
        selector: &RepositorySelector,
    ) -> Result<Vec<RepositoryView>, String> {
        let api: Api<Repository> = match &self.watched_namespace {
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
            .map_err(|e| format!("listing AngosRepositories: {e}"))?;
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
            if repo
                .status
                .as_ref()
                .and_then(|s| s.common.instance.as_ref())
                != Some(&bound)
            {
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
            out.push(RepositoryView {
                namespace,
                name: repo.name_any(),
                created: repo.metadata.creation_timestamp.as_ref().map(|t| t.0),
                generation: repo.metadata.generation,
                annotations: repo.annotations().clone(),
                spec: repo.spec.clone(),
            });
        }
        Ok(out)
    }
}
