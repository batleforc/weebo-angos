//! An `AngosMaintenance` (spec 4.3, 6.6): the operator is its scheduler.
//!
//! 1. A run in progress is followed: its `Job` finished (succeeded,
//!    failed, timed out — the end of its log kept, the dry run's report
//!    too) or gone (`MaintenanceInterrupted`, never retried).
//! 2. A run is due on its schedule (from the last run or skip, never
//!    catching up), on a new `angos.weebo.io/run-now`, or once without a
//!    schedule.
//! 3. A due run waits (an advisory, the slot skipped) while suspended, while
//!    the instance isn't ready, while another writing task runs on it, and —
//!    a prune — while the prune lock holds.
//! 4. A run starts: recorded `Running` first (a pass failing after that
//!    never starts a second one), then its `Job` created.

use std::collections::BTreeMap;

use api::annotations;
use api::maintenance::{DryRun, MaintenanceSpec, MaintenanceStatus, Run, RunState, Task};
use domain::error::ReasonCode;
use domain::{Advisory, Failure};
use k8s_openapi::jiff::Timestamp;
use weebo_kit_application::workload::LogQuery;

use super::{Outcome, instance_failure, workload_failure};
use crate::cron::Schedule;
use crate::ports::{
    AttachedInstances, Jobs, MaintenanceDirectory, OwnedObjects, Owner, PodLogs, StatusLedger,
};

pub struct MaintenancePass<'a> {
    pub owner: &'a Owner,
    pub created: Option<Timestamp>,
    pub spec: &'a MaintenanceSpec,
    pub annotations: &'a BTreeMap<String, String>,
    pub status: &'a MaintenanceStatus,
    pub now: Timestamp,
    pub default_image: &'a str,
}

pub struct MaintenancePorts<'a> {
    pub instances: &'a dyn AttachedInstances,
    /// The Jobs, owned by the maintenance.
    pub objects: &'a dyn OwnedObjects,
    pub jobs: &'a dyn Jobs,
    pub logs: &'a dyn PodLogs,
    pub maintenances: &'a dyn MaintenanceDirectory,
    pub repositories: &'a dyn crate::ports::RepositoryDirectory,
    pub ledger: &'a dyn StatusLedger,
}

#[derive(Debug, Clone)]
pub struct MaintenanceReport {
    pub outcome: Outcome,
    pub status: MaintenanceStatus,
    pub events: Vec<(ReasonCode, String)>,
    /// When to look again, seconds.
    pub due_in: Option<u64>,
}

fn advisory(reason: ReasonCode, message: impl Into<String>) -> Advisory {
    Advisory {
        reason,
        message: message.into(),
    }
}

/// How many log lines a run keeps.
const TAIL_LINES: usize = 20;

pub async fn reconcile_maintenance(
    pass: MaintenancePass<'_>,
    ports: &MaintenancePorts<'_>,
) -> MaintenanceReport {
    let mut report = MaintenanceReport {
        outcome: Outcome::synced(""),
        status: pass.status.clone(),
        events: Vec::new(),
        due_in: None,
    };
    let mut advisories = Vec::new();
    let ready = run(&pass, ports, &mut report, &mut advisories).await;
    report.outcome = match ready {
        Ok(message) => Outcome::synced(message),
        Err(failure) => Outcome::failed(failure),
    };
    report.outcome.instance = report.status.common.instance.clone();
    report.outcome.advisories = advisories;
    report
}

async fn tail(ports: &MaintenancePorts<'_>, namespace: &str, pod: Option<&str>) -> Option<String> {
    let pod = pod?;
    let lines = ports
        .logs
        .read(
            namespace,
            pod,
            &LogQuery {
                container: Some("angos".into()),
                since: None,
                limit_bytes: Some(256 * 1024),
            },
        )
        .await
        .ok()?;
    let messages: Vec<String> = lines
        .iter()
        .map(|l| {
            serde_json::from_str::<serde_json::Value>(&l.text)
                .ok()
                .and_then(|v| v["fields"]["message"].as_str().map(String::from))
                .unwrap_or_else(|| l.text.clone())
        })
        .map(|m| crate::reload::redact(&m))
        .collect();
    let start = messages.len().saturating_sub(TAIL_LINES);
    Some(messages[start..].join("\n"))
}

async fn run(
    pass: &MaintenancePass<'_>,
    ports: &MaintenancePorts<'_>,
    report: &mut MaintenanceReport,
    advisories: &mut Vec<Advisory>,
) -> Result<String, Failure> {
    let owner = pass.owner;
    let spec = pass.spec;
    let status = &mut report.status;
    let schedule = spec
        .schedule
        .as_deref()
        .map(Schedule::parse)
        .transpose()
        .map_err(|e| Failure::new(ReasonCode::InvalidSpec, format!("spec.schedule: {e}")))?;

    // 1. A run in progress.
    if let Some(run) = status
        .last_run
        .clone()
        .filter(|r| r.state == RunState::Running)
    {
        let job = ports
            .jobs
            .job(&owner.namespace, &run.job_name)
            .await
            .map_err(|e| Failure::new(ReasonCode::KubernetesApiError, e))?;
        let finished = |state: RunState, exit_code: Option<i32>, message: Option<String>| Run {
            state,
            completion_time: Some(pass.now.to_string()),
            exit_code,
            message,
            ..run.clone()
        };
        match job {
            None => {
                status.last_run = Some(finished(
                    RunState::Interrupted,
                    None,
                    Some(format!("the Job {} is gone", run.job_name)),
                ));
                status.failure_count += 1;
            }
            Some(job) if job.active || !(job.succeeded || job.failed) => {
                advisories.push(advisory(
                    ReasonCode::MaintenanceRunning,
                    format!("Job {} running since {}", run.job_name, run.start_time),
                ));
                report.due_in = Some(15);
                return Ok(format!("{:?} running", spec.task));
            }
            Some(job) => {
                let message = tail(ports, &owner.namespace, job.pod.as_deref()).await;
                let state = if job.succeeded {
                    RunState::Succeeded
                } else if job.deadline_exceeded {
                    RunState::TimedOut
                } else {
                    RunState::Failed
                };
                if state != RunState::Succeeded {
                    status.failure_count += 1;
                }
                if state == RunState::Succeeded && run.dry_run {
                    status.last_dry_run = Some(DryRun {
                        time: pass.now.to_string(),
                        summary: message.clone().unwrap_or_default(),
                    });
                }
                status.last_run = Some(finished(state, job.exit_code, message));
            }
        }
    }

    // The instance (this namespace's).
    let bound = ports
        .instances
        .resolve_attached(
            &owner.namespace,
            spec.instance_ref.as_ref(),
            status.common.instance.as_ref(),
        )
        .await
        .map_err(instance_failure)?;
    if bound.namespace.as_deref() != Some(owner.namespace.as_str()) {
        return Err(Failure::new(
            ReasonCode::InstanceNotAllowed,
            "a maintenance runs on an instance of its own namespace",
        ));
    }
    status.common.instance = Some(bound.clone());
    let instance = ports
        .instances
        .instance(&bound)
        .await
        .map_err(|e| Failure::new(ReasonCode::KubernetesApiError, e))?
        .ok_or_else(|| Failure::new(ReasonCode::InstanceNotFound, format!("{bound} is gone")))?;
    if let Some(why) = crate::maintenance::unreachable_store(&instance) {
        return Err(Failure::new(ReasonCode::InvalidSpec, why));
    }

    // 2. Due?
    let run_now = pass.annotations.get(annotations::RUN_NOW).cloned();
    let asked = run_now.is_some() && run_now != status.run_now;
    let since = [
        status.last_run.as_ref().map(|r| r.start_time.clone()),
        status.last_skip_time.clone(),
    ]
    .into_iter()
    .flatten()
    .filter_map(|t| t.parse::<Timestamp>().ok())
    .max()
    .or(pass.created)
    .unwrap_or(pass.now);
    let next = schedule.as_ref().and_then(|s| s.next_after(since));
    status.next_run_time = next.map(|t| t.to_string());
    let due = asked
        || match &schedule {
            Some(_) => next.is_some_and(|n| n <= pass.now),
            None => status.last_run.is_none() && status.last_skip_time.is_none(),
        };
    if let Some(next) = next.filter(|n| *n > pass.now) {
        report.due_in = Some(pass.now.duration_until(next).as_secs().max(1) as u64);
    }

    // 3. May it run?
    if due {
        let instance_status = instance.status.clone().unwrap_or_default();
        let instance_ready = weebo_kit_api::status::ready(&instance_status.conditions)
            .is_some_and(|c| c.status == "True");
        let others = ports
            .maintenances
            .in_namespace(&owner.namespace)
            .await
            .map_err(|e| Failure::new(ReasonCode::KubernetesApiError, e))?;
        let skipped = if spec.suspend {
            Some((ReasonCode::MaintenanceSuspended, "spec.suspend".to_string()))
        } else if !instance_ready || instance_status.pending_boot_hash.is_some() {
            Some((
                ReasonCode::MaintenanceSuspended,
                format!("{bound} isn't ready (rolling out, or its configuration not confirmed)"),
            ))
        } else if let Some(other) = spec
            .task
            .writes()
            .then(|| crate::maintenance::writer_running(&others, &owner.name, &bound))
            .flatten()
        {
            Some((
                ReasonCode::MaintenanceDeferred,
                format!("the maintenance {other} writes to the store: one at a time"),
            ))
        } else if spec.task == Task::Prune {
            let instance_owner = Owner {
                namespace: bound.namespace.clone().unwrap_or_default(),
                name: bound.name.clone(),
                uid: instance.metadata.uid.clone().unwrap_or_default(),
            };
            let aiming = ports
                .repositories
                .aiming_at(&instance_owner)
                .await
                .map_err(|e| Failure::new(ReasonCode::KubernetesApiError, e))?;
            crate::maintenance::prune_lock(&instance, &aiming, pass.now)
                .map(|why| (ReasonCode::MaintenanceSkipped, format!("prune lock: {why}")))
        } else {
            None
        };
        if let Some((reason, why)) = skipped {
            advisories.push(advisory(reason, why));
            // A deferred run is tried again at the next pass; the others wait
            // for the next slot.
            if reason != ReasonCode::MaintenanceDeferred {
                status.last_skip_time = Some(pass.now.to_string());
                if asked {
                    status.run_now = run_now;
                }
            } else {
                report.due_in = Some(30);
            }
        } else {
            // 4. Start: recorded first, then created.
            let boot_hash = instance_status.boot_hash.clone().ok_or_else(|| {
                Failure::new(
                    ReasonCode::WorkloadNotReady,
                    format!("{bound} runs no startup layer yet"),
                )
            })?;
            let job_name = format!("{}-{}", owner.name, pass.now.as_second());
            status.last_run = Some(Run {
                state: RunState::Running,
                start_time: pass.now.to_string(),
                completion_time: None,
                job_name: job_name.clone(),
                exit_code: None,
                message: None,
                dry_run: spec.params.dry_run,
            });
            status.run_count += 1;
            if asked {
                status.run_now = run_now;
            }
            let mut fields = serde_json::Map::new();
            fields.insert(
                "lastRun".into(),
                serde_json::to_value(&status.last_run).unwrap_or_default(),
            );
            fields.insert("runCount".into(), status.run_count.into());
            fields.insert(
                "runNow".into(),
                serde_json::to_value(&status.run_now).unwrap_or_default(),
            );
            fields.insert(
                "instance".into(),
                serde_json::to_value(&status.common.instance).unwrap_or_default(),
            );
            ports
                .ledger
                .record(&owner.namespace, &owner.name, fields)
                .await
                .map_err(|e| Failure::new(ReasonCode::KubernetesApiError, e))?;
            let image = crate::workload::image(&instance.spec, pass.default_image);
            let job = crate::maintenance::job(
                &job_name,
                &owner.name,
                &owner.namespace,
                spec,
                &instance,
                &image,
                &boot_hash,
            );
            ports
                .objects
                .apply(owner, &[job])
                .await
                .map_err(workload_failure)?;
            report.events.push((
                ReasonCode::MaintenanceStarted,
                format!(
                    "{:?}{} started (Job {job_name})",
                    spec.task,
                    if spec.params.dry_run { ", dry run" } else { "" }
                ),
            ));
            report.due_in = Some(15);
            return Ok(format!("{:?} running", spec.task));
        }
    }

    // The last run says how it went.
    match status.last_run.as_ref() {
        Some(run) if run.state == RunState::Failed => Err(Failure::new(
            ReasonCode::MaintenanceFailed,
            format!(
                "Job {} failed (exit code {}): {}",
                run.job_name,
                run.exit_code.map_or("?".into(), |c| c.to_string()),
                run.message.clone().unwrap_or_default()
            ),
        )),
        Some(run) if run.state == RunState::TimedOut => Err(Failure::new(
            ReasonCode::MaintenanceTimeout,
            format!(
                "Job {} outlived timeoutSeconds ({})",
                run.job_name, spec.timeout_seconds
            ),
        )),
        Some(run) if run.state == RunState::Interrupted => Err(Failure::new(
            ReasonCode::MaintenanceInterrupted,
            run.message.clone().unwrap_or_default(),
        )),
        Some(run) => Ok(format!("last run {:?} at {}", run.state, run.start_time)),
        None => Ok("no run yet".into()),
    }
}

#[cfg(test)]
mod tests;
