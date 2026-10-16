use k8s_openapi::api::core::v1::ResourceRequirements;
use kube::CustomResource;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

pub use weebo_kit_api::InstanceRef;

use crate::config::open_object;
use crate::status::Status;

/// The short names the rest of the workspace uses.
pub use self::AngosMaintenance as Maintenance;
pub use self::AngosMaintenanceSpec as MaintenanceSpec;

/// A task of Angos's CLI on an instance of this namespace (`angos scrub`,
/// `prune`, `reconcile …`), once or on a schedule, run as a `Job` with the
/// instance's image and configuration. The operator schedules it: at most
/// one run per due time, never two writing tasks at once on an instance,
/// none while it rolls out — and a prune never while a repository could
/// lose its data (spec 6.6). Every task runs as a dry run unless
/// `params.dryRun` is `false`.
#[derive(CustomResource, Serialize, Deserialize, Clone, Debug, PartialEq, JsonSchema)]
#[kube(
    group = "angos.weebo.io",
    version = "v1alpha1",
    kind = "AngosMaintenance",
    plural = "angosmaintenances",
    shortname = "amaint",
    namespaced,
    status = "MaintenanceStatus",
    printcolumn = r#"{"name":"Task","type":"string","jsonPath":".spec.task"}"#,
    printcolumn = r#"{"name":"Schedule","type":"string","jsonPath":".spec.schedule"}"#,
    printcolumn = r#"{"name":"Last","type":"string","jsonPath":".status.lastRun.state"}"#,
    printcolumn = r#"{"name":"Ready","type":"string","jsonPath":".status.conditions[?(@.type=='Ready')].status"}"#,
    printcolumn = r#"{"name":"Reason","type":"string","jsonPath":".status.conditions[?(@.type=='Ready')].reason"}"#,
    printcolumn = r#"{"name":"Age","type":"date","jsonPath":".metadata.creationTimestamp"}"#
)]
#[serde(rename_all = "camelCase")]
pub struct AngosMaintenanceSpec {
    /// The instance, of this namespace: unset, its only one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instance_ref: Option<InstanceRef>,
    pub task: Task,
    #[serde(default)]
    pub params: Params,
    /// A 5-field cron expression, in UTC (`15 * * * *`). Unset: one run.
    /// A time missed (the operator down) is not caught up.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schedule: Option<String>,
    /// No run while true.
    #[serde(default)]
    pub suspend: bool,
    /// The Job's `activeDeadlineSeconds`.
    #[serde(default = "hour")]
    #[schemars(range(min = 60))]
    pub timeout_seconds: i64,
    /// The Job's resources (the tasks spawn up to 64 threads; `concurrency`
    /// bounds the work).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(schema_with = "open_object")]
    pub resources: Option<ResourceRequirements>,
    /// How long a finished Job is kept.
    #[serde(default = "day")]
    pub ttl_seconds_after_finished: i32,
}

fn hour() -> i64 {
    3600
}

fn day() -> i32 {
    86_400
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, JsonSchema)]
pub enum Task {
    /// `angos scrub`: repair inconsistencies, quarantine unknown objects.
    Scrub,
    /// `angos prune`: apply the retention policies, reclaim old uploads —
    /// and **delete every namespace no repository declares**.
    Prune,
    /// `angos reconcile replication`.
    ReconcileReplication,
    /// `angos reconcile scan`.
    ReconcileScan,
    /// `angos reconcile index`.
    ReconcileIndex,
}

impl Task {
    /// Whether it changes the store (one such task at a time per instance).
    pub fn writes(self) -> bool {
        matches!(self, Task::Scrub | Task::Prune)
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Params {
    /// `-d`: report only. **True unless set to false.**
    #[serde(default = "yes")]
    pub dry_run: bool,
    /// Scrub: `--delete-unknown` (delete instead of quarantining).
    #[serde(default)]
    pub delete_unknown: bool,
    /// Prune: `-u`, the age of the upload leftovers reclaimed.
    #[serde(default = "uploads")]
    pub upload_timeout: String,
    /// `--concurrency`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(range(min = 1))]
    pub concurrency: Option<u32>,
    /// Reconcile scan/index: `--force`.
    #[serde(default)]
    pub force: bool,
}

fn yes() -> bool {
    true
}

fn uploads() -> String {
    "24h".into()
}

impl Default for Params {
    fn default() -> Self {
        Self {
            dry_run: true,
            delete_unknown: false,
            upload_timeout: uploads(),
            concurrency: None,
            force: false,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct MaintenanceStatus {
    #[serde(flatten)]
    pub common: Status,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_run: Option<Run>,
    /// The last dry run's report (its last lines), to read before running
    /// for real.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_dry_run: Option<DryRun>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_run_time: Option<String>,
    #[serde(default)]
    pub run_count: u64,
    #[serde(default)]
    pub failure_count: u64,
    /// When a due run was last skipped (prune lock, write lock, suspended):
    /// the schedule moves on from there.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_skip_time: Option<String>,
    /// The `angos.weebo.io/run-now` value last run for.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_now: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Run {
    pub state: RunState,
    pub start_time: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completion_time: Option<String>,
    pub job_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
    /// The end of its log (redacted), or why it didn't run.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    #[serde(default)]
    pub dry_run: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub enum RunState {
    Running,
    Succeeded,
    Failed,
    TimedOut,
    Interrupted,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct DryRun {
    pub time: String,
    pub summary: String,
}
