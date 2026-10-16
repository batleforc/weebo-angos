//! The operator's own metrics (spec 6.9), beside the kit's reconcile ones:
//!
//! - `weebo_angos_instance_ready{instance_namespace,instance_name}`: 1 when
//!   the `AngosLocalInstance` is `Ready`, gone with it;
//! - `weebo_angos_config_reloads_total{instance_namespace,instance_name,result}`:
//!   hot layers `applied`, `rejected`, or `unconfirmed` (the pods rolled
//!   instead);
//! - `weebo_angos_rollouts_total{instance_namespace,instance_name,cause}`:
//!   the pods rolled onto a new startup layer (`boot`), a new Angos version
//!   (`image`), a confirmed storage change (`storage`), or for a reload
//!   they didn't confirm (`fallback`);
//! - `weebo_angos_maintenance_runs_total{task,result}`: maintenance runs
//!   ended (`success`, `failure`) or skipped (`skipped`).
//!
//! Each is read off what a pass changed in the status. Not
//! `namespace`/`instance`: Prometheus sets those on every scraped series
//! (the operator's pod). The counters are named without `_total`, which the
//! Prometheus exporter adds.

use std::collections::BTreeMap;
use std::sync::{Mutex, OnceLock};

use api::maintenance::{MaintenanceStatus, RunState, Task};
use api::status::InstanceStatus;
use opentelemetry::KeyValue;
use opentelemetry::metrics::Counter;

use super::FIELD_MANAGER;

type Key = (String, String);

fn ready() -> &'static Mutex<BTreeMap<Key, bool>> {
    static READY: OnceLock<Mutex<BTreeMap<Key, bool>>> = OnceLock::new();
    READY.get_or_init(Default::default)
}

fn labels(namespace: &str, name: &str) -> [KeyValue; 2] {
    [
        KeyValue::new("instance_namespace", namespace.to_string()),
        KeyValue::new("instance_name", name.to_string()),
    ]
}

fn counter(name: &'static str, description: &'static str) -> Counter<u64> {
    opentelemetry::global::meter(FIELD_MANAGER)
        .u64_counter(name)
        .with_description(description)
        .build()
}

struct Counters {
    reloads: Counter<u64>,
    rollouts: Counter<u64>,
    runs: Counter<u64>,
}

fn counters() -> &'static Counters {
    static COUNTERS: OnceLock<Counters> = OnceLock::new();
    COUNTERS.get_or_init(|| {
        // The gauge's callback lives as long as the meter provider.
        let _ = opentelemetry::global::meter(FIELD_MANAGER)
            .u64_observable_gauge("weebo_angos_instance_ready")
            .with_description("1 when the AngosLocalInstance is Ready")
            .with_callback(|observer| {
                for ((namespace, name), ready) in ready().lock().expect("metrics lock").iter() {
                    observer.observe(u64::from(*ready), &labels(namespace, name));
                }
            })
            .build();
        Counters {
            // `weebo_angos_config_reloads_total`
            reloads: counter(
                "weebo_angos_config_reloads",
                "Hot layers applied, rejected or unconfirmed",
            ),
            // `weebo_angos_rollouts_total`
            rollouts: counter(
                "weebo_angos_rollouts",
                "Rollouts of an instance's pods, by cause",
            ),
            // `weebo_angos_maintenance_runs_total`
            runs: counter(
                "weebo_angos_maintenance_runs",
                "Maintenance runs ended or skipped, by task and result",
            ),
        }
    })
}

/// What a pass of an instance changed: the reloads and rollouts to count.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct InstanceChanges {
    pub reloads: Vec<&'static str>,
    pub rollouts: Vec<&'static str>,
}

/// The reloads and rollouts between two statuses of an instance.
pub fn instance_changes(previous: &InstanceStatus, now: &InstanceStatus) -> InstanceChanges {
    let mut changes = InstanceChanges::default();
    let first = previous.applied_config_hash.is_none();
    if !first
        && now.last_applied_time.is_some()
        && now.last_applied_time != previous.last_applied_time
    {
        changes.reloads.push("applied");
    }
    if now.last_rejection_time.is_some() && now.last_rejection_time != previous.last_rejection_time
    {
        changes.reloads.push("rejected");
    }
    if now.rolled_out_for.is_some() && now.rolled_out_for != previous.rolled_out_for {
        changes.reloads.push("unconfirmed");
        changes.rollouts.push("fallback");
    }
    if previous.boot_hash.is_some() {
        if now.version.is_some() && now.version != previous.version {
            changes.rollouts.push("image");
        } else if now.boot_hash != previous.boot_hash {
            changes.rollouts.push(
                if previous.pending_boot_hash.is_some()
                    && previous.pending_boot_hash == now.boot_hash
                {
                    "storage"
                } else {
                    "boot"
                },
            );
        }
    }
    changes
}

/// Records an instance's pass.
pub fn record_instance(
    namespace: &str,
    name: &str,
    is_ready: bool,
    previous: &InstanceStatus,
    now: &InstanceStatus,
) {
    let counters = counters();
    ready()
        .lock()
        .expect("metrics lock")
        .insert((namespace.to_string(), name.to_string()), is_ready);
    let changes = instance_changes(previous, now);
    for result in changes.reloads {
        let [ns, n] = labels(namespace, name);
        counters
            .reloads
            .add(1, &[ns, n, KeyValue::new("result", result)]);
    }
    for cause in changes.rollouts {
        let [ns, n] = labels(namespace, name);
        counters
            .rollouts
            .add(1, &[ns, n, KeyValue::new("cause", cause)]);
    }
}

/// The instance is gone: its gauge too.
pub fn forget_instance(namespace: &str, name: &str) {
    ready()
        .lock()
        .expect("metrics lock")
        .remove(&(namespace.to_string(), name.to_string()));
}

/// The run a maintenance's pass ended or skipped: `success`, `failure` or
/// `skipped`.
pub fn maintenance_result(
    previous: &MaintenanceStatus,
    now: &MaintenanceStatus,
) -> Option<&'static str> {
    let was_running = previous
        .last_run
        .as_ref()
        .is_some_and(|r| r.state == RunState::Running);
    if let Some(run) = now.last_run.as_ref().filter(|_| was_running)
        && run.state != RunState::Running
    {
        return Some(if run.state == RunState::Succeeded {
            "success"
        } else {
            "failure"
        });
    }
    (now.last_skip_time.is_some() && now.last_skip_time != previous.last_skip_time)
        .then_some("skipped")
}

/// Records a maintenance's pass.
pub fn record_maintenance(task: Task, previous: &MaintenanceStatus, now: &MaintenanceStatus) {
    if let Some(result) = maintenance_result(previous, now) {
        counters().runs.add(
            1,
            &[
                KeyValue::new("task", format!("{task:?}")),
                KeyValue::new("result", result),
            ],
        );
    }
}

#[cfg(test)]
mod tests {
    use api::maintenance::Run;

    use super::*;

    fn status(f: impl FnOnce(&mut InstanceStatus)) -> InstanceStatus {
        let mut s = InstanceStatus {
            boot_hash: Some("b1".into()),
            applied_config_hash: Some("h1".into()),
            last_applied_time: Some("t1".into()),
            version: Some("1.12.2".into()),
            ..Default::default()
        };
        f(&mut s);
        s
    }

    #[test]
    fn a_pass_s_reloads_and_rollouts_are_read_off_its_status() {
        let before = status(|_| {});
        assert_eq!(
            instance_changes(&before, &before),
            InstanceChanges::default()
        );
        let reloaded = status(|s| s.last_applied_time = Some("t2".into()));
        assert_eq!(instance_changes(&before, &reloaded).reloads, ["applied"]);
        let rejected = status(|s| s.last_rejection_time = Some("t2".into()));
        assert_eq!(instance_changes(&before, &rejected).reloads, ["rejected"]);
        let fallback = status(|s| s.rolled_out_for = Some("h2".into()));
        assert_eq!(
            instance_changes(&before, &fallback),
            InstanceChanges {
                reloads: vec!["unconfirmed"],
                rollouts: vec!["fallback"]
            }
        );
        let image = status(|s| {
            s.version = Some("1.12.3".into());
            s.boot_hash = Some("b2".into());
        });
        assert_eq!(instance_changes(&before, &image).rollouts, ["image"]);
        let boot = status(|s| s.boot_hash = Some("b2".into()));
        assert_eq!(instance_changes(&before, &boot).rollouts, ["boot"]);
        let pending = status(|s| s.pending_boot_hash = Some("b2".into()));
        assert_eq!(instance_changes(&pending, &boot).rollouts, ["storage"]);
        // The first configuration is neither a reload nor a rollout.
        let new = InstanceStatus::default();
        assert_eq!(instance_changes(&new, &before), InstanceChanges::default());
    }

    #[test]
    fn a_maintenance_run_counts_once_when_it_ends() {
        let run = |state| MaintenanceStatus {
            last_run: Some(Run {
                state,
                start_time: "t".into(),
                completion_time: None,
                job_name: "j".into(),
                exit_code: None,
                message: None,
                dry_run: false,
            }),
            ..Default::default()
        };
        let running = run(RunState::Running);
        assert_eq!(maintenance_result(&running, &running), None);
        assert_eq!(
            maintenance_result(&running, &run(RunState::Succeeded)),
            Some("success")
        );
        assert_eq!(
            maintenance_result(&running, &run(RunState::TimedOut)),
            Some("failure")
        );
        let done = run(RunState::Succeeded);
        assert_eq!(maintenance_result(&done, &done), None);
        let skipped = MaintenanceStatus {
            last_skip_time: Some("t".into()),
            ..done.clone()
        };
        assert_eq!(maintenance_result(&done, &skipped), Some("skipped"));
    }
}
