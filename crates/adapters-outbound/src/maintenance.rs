//! [`Jobs`] and [`MaintenanceDirectory`] on the Kubernetes API: a
//! maintenance's `Job` (its conditions, its pod's exit code) and the
//! maintenances of a namespace.

use api::Maintenance;
use application::ports::{JobState, Jobs, MaintenanceDirectory};
use k8s_openapi::api::batch::v1::Job;
use k8s_openapi::api::core::v1::Pod;
use kube::api::{Api, ListParams};
use kube::{Client, ResourceExt};
use weebo_kit_adapters::instance::STORE_READY_TIMEOUT;
use weebo_kit_runtime::shared::SharedWatches;

pub struct KubeJobs {
    client: Client,
}

impl KubeJobs {
    pub fn new(client: Client) -> Self {
        Self { client }
    }
}

#[async_trait::async_trait]
impl Jobs for KubeJobs {
    async fn job(&self, namespace: &str, name: &str) -> Result<Option<JobState>, String> {
        let jobs: Api<Job> = Api::namespaced(self.client.clone(), namespace);
        let Some(job) = jobs
            .get_opt(name)
            .await
            .map_err(|e| format!("Job {namespace}/{name}: {e}"))?
        else {
            return Ok(None);
        };
        let status = job.status.clone().unwrap_or_default();
        let condition = |kind: &str| {
            status
                .conditions
                .iter()
                .flatten()
                .find(|c| c.type_ == kind && c.status == "True")
        };
        let failed = condition("Failed");
        let pods: Api<Pod> = Api::namespaced(self.client.clone(), namespace);
        let pod = pods
            .list(&ListParams::default().labels(&format!("job-name={name}")))
            .await
            .map_err(|e| format!("pods of Job {namespace}/{name}: {e}"))?
            .items
            .into_iter()
            .next();
        let exit_code = pod.as_ref().and_then(|p| {
            p.status
                .as_ref()?
                .container_statuses
                .as_ref()?
                .first()?
                .state
                .as_ref()?
                .terminated
                .as_ref()
                .map(|t| t.exit_code)
        });
        Ok(Some(JobState {
            active: status.active.unwrap_or(0) > 0,
            succeeded: condition("Complete").is_some(),
            failed: failed.is_some(),
            deadline_exceeded: failed
                .and_then(|c| c.reason.as_deref())
                .is_some_and(|r| r == "DeadlineExceeded"),
            pod: pod.map(|p| p.name_any()),
            exit_code,
        }))
    }
}

pub struct KubeMaintenances {
    client: Client,
    /// The binary's shared watches: the maintenances read from the store
    /// once listed, from the API otherwise.
    watches: SharedWatches,
}

impl KubeMaintenances {
    pub fn new(client: Client) -> Self {
        Self {
            client,
            watches: SharedWatches::default(),
        }
    }

    /// Read the maintenances from the binary's shared watches instead of a
    /// LIST on every pass.
    pub fn with_watches(mut self, watches: SharedWatches) -> Self {
        self.watches = watches;
        self
    }
}

#[async_trait::async_trait]
impl MaintenanceDirectory for KubeMaintenances {
    async fn in_namespace(&self, namespace: &str) -> Result<Vec<Maintenance>, String> {
        if let Some(shared) = self.watches.get::<Maintenance>() {
            let store = shared.store();
            // An unsynced store is never read as empty.
            if weebo_kit_adapters::wait_synced(&store, STORE_READY_TIMEOUT).await {
                let mut found: Vec<Maintenance> = store
                    .state()
                    .iter()
                    .filter(|m| m.namespace().as_deref() == Some(namespace))
                    .map(|m| Maintenance::clone(m))
                    .collect();
                found.sort_by_key(ResourceExt::name_any);
                return Ok(found);
            }
        }
        Api::<Maintenance>::namespaced(self.client.clone(), namespace)
            .list(&ListParams::default())
            .await
            .map(|l| l.items)
            .map_err(|e| format!("listing AngosMaintenances in {namespace}: {e}"))
    }
}
