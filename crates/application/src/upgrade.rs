//! Angos upgrades (spec 6.8): which tags an instance may move to, and the
//! steps that move it. The configuration model, the validation, the
//! startup keys and the log messages are the operator's version's: an
//! instance runs a tag of [`api::angos::SUPPORTED_VERSIONS`], never back
//! below the version it runs, never more than one minor ahead — unless
//! `angos.weebo.io/approved-upgrade` names that tag.
//!
//! Orchestrated by the instance's pass: the store is scrubbed on the old
//! version (dry run, then for real — no other writer running), the pods
//! roll, the new version checks the store (`scrub -d`) and rebuilds the
//! index (`reconcile index`). Each step's `Job` is named after the target,
//! the step and the attempt: a pass that finds it again carries on.

use api::local_instance::{LocalInstance, LocalInstanceSpec};
use api::maintenance::{MaintenanceSpec, Params, Task};
use api::status::UpgradeStep;
use weebo_kit_application::workload::OwnedObject;

use crate::ports::JobState;

/// A `major.minor.patch` tag (a leading `v` allowed).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Version {
    pub major: u64,
    pub minor: u64,
    pub patch: u64,
}

impl Version {
    pub fn parse(tag: &str) -> Option<Self> {
        let mut parts = tag.strip_prefix('v').unwrap_or(tag).split('.');
        let mut next = || parts.next()?.parse::<u64>().ok();
        let version = Self {
            major: next()?,
            minor: next()?,
            patch: next()?,
        };
        parts.next().is_none().then_some(version)
    }
}

/// Why an upgrade isn't allowed, and how to allow it anyway.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct UpgradeRefused(pub String);

/// Why the instance may not run `to` while it runs `current` (`None`: a
/// new instance), unless `approved` names `to`.
pub fn check(
    current: Option<&str>,
    to: &str,
    approved: Option<&str>,
) -> Result<(), UpgradeRefused> {
    if approved == Some(to) {
        return Ok(());
    }
    let lift = format!(
        "annotate the instance {}={to} to run it anyway",
        api::annotations::APPROVED_UPGRADE
    );
    if !api::angos::SUPPORTED_VERSIONS.contains(&to) {
        return Err(UpgradeRefused(format!(
            "Angos {to} isn't a version the operator is tested with ({}): {lift}",
            api::angos::SUPPORTED_VERSIONS.join(", ")
        )));
    }
    let (Some(current), Some(target)) = (current, Version::parse(to)) else {
        return Ok(());
    };
    let Some(running) = Version::parse(current) else {
        return Ok(());
    };
    if target < running {
        return Err(UpgradeRefused(format!(
            "Angos {to} is older than the {current} the instance runs: its store may hold what \
             {to} doesn't read; {lift}"
        )));
    }
    if target.major != running.major || target.minor > running.minor.saturating_add(1) {
        return Err(UpgradeRefused(format!(
            "Angos {current} → {to} skips a minor version: upgrade one minor at a time, or {lift}"
        )));
    }
    Ok(())
}

/// The step an upgrade starts at: the scrub, unless no `Job` can reach the
/// store.
pub fn first_step(instance: &LocalInstance) -> UpgradeStep {
    if crate::maintenance::unreachable_store(instance).is_some() {
        UpgradeStep::Rollout
    } else {
        UpgradeStep::ScrubDryRun
    }
}

/// The step after `step`, `None` after the last.
pub fn next(step: UpgradeStep, instance: &LocalInstance) -> Option<UpgradeStep> {
    let reachable = crate::maintenance::unreachable_store(instance).is_none();
    match step {
        UpgradeStep::ScrubDryRun => Some(UpgradeStep::Scrub),
        UpgradeStep::Scrub => Some(UpgradeStep::Rollout),
        UpgradeStep::Rollout if reachable => Some(UpgradeStep::Verify),
        UpgradeStep::Rollout => None,
        UpgradeStep::Verify => Some(UpgradeStep::ReconcileIndex),
        UpgradeStep::ReconcileIndex => None,
    }
}

/// Whether `step` runs on the old version (the pods stay on it).
pub fn before_rollout(step: UpgradeStep) -> bool {
    step < UpgradeStep::Rollout
}

/// The task a step's `Job` runs, and whether for real.
fn task(step: UpgradeStep) -> Option<(Task, bool)> {
    match step {
        UpgradeStep::ScrubDryRun => Some((Task::Scrub, true)),
        UpgradeStep::Scrub => Some((Task::Scrub, false)),
        UpgradeStep::Rollout => None,
        UpgradeStep::Verify => Some((Task::Scrub, true)),
        UpgradeStep::ReconcileIndex => Some((Task::ReconcileIndex, false)),
    }
}

/// The name of a step's `Job`: the instance's (shortened), the step, a
/// hash of the target and the attempt.
pub fn job_name(instance: &str, to: &str, step: UpgradeStep, attempt: u32) -> String {
    let step = match step {
        UpgradeStep::ScrubDryRun => "scrub-d",
        UpgradeStep::Scrub => "scrub",
        UpgradeStep::Rollout => "rollout",
        UpgradeStep::Verify => "verify",
        UpgradeStep::ReconcileIndex => "index",
    };
    let hash = weebo_kit_application::workload::content_hash(format!("{to}/{attempt}").as_bytes());
    let prefix: String = instance.chars().take(40).collect();
    format!("{}-up-{step}-{}", prefix.trim_end_matches('-'), &hash[..8])
}

/// A step's `Job`, on `image` and the startup layer `boot_hash`; `None` for
/// the rollout.
pub fn job(
    instance: &LocalInstance,
    namespace: &str,
    step: UpgradeStep,
    job_name: &str,
    image: &str,
    boot_hash: &str,
) -> Option<OwnedObject> {
    let (task, dry_run) = task(step)?;
    let name = instance.metadata.name.clone().unwrap_or_default();
    let spec = MaintenanceSpec {
        instance_ref: None,
        task,
        params: Params {
            dry_run,
            ..Default::default()
        },
        schedule: None,
        suspend: false,
        timeout_seconds: 6 * 3600,
        resources: None,
        ttl_seconds_after_finished: 7 * 86_400,
    };
    Some(crate::maintenance::job(
        job_name,
        &format!("{name}-upgrade"),
        namespace,
        &spec,
        instance,
        image,
        boot_hash,
    ))
}

/// What a step's `Job` says.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StepState {
    /// Not created yet.
    Absent,
    Running,
    Done,
    Failed(String),
}

pub fn state(job_name: &str, job: Option<&JobState>) -> StepState {
    match job {
        None => StepState::Absent,
        Some(j) if j.succeeded => StepState::Done,
        Some(j) if j.deadline_exceeded => {
            StepState::Failed(format!("Job {job_name} outlived its deadline (6 h)"))
        }
        Some(j) if j.failed => StepState::Failed(format!(
            "Job {job_name} failed (exit code {}): kubectl logs job/{job_name}",
            j.exit_code.map_or("?".into(), |c| c.to_string())
        )),
        Some(_) => StepState::Running,
    }
}

/// The instance an upgrade's `Job` is built for.
pub fn instance_of(name: &str, spec: &LocalInstanceSpec) -> LocalInstance {
    LocalInstance::new(name, spec.clone())
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn versions_read_as_tags_are_written() {
        assert_eq!(
            Version::parse("v1.12.3"),
            Some(Version {
                major: 1,
                minor: 12,
                patch: 3
            })
        );
        for bad in ["latest", "1.12", "1.12.3.4", "1.x.3", "1.12.3-rc1"] {
            assert_eq!(Version::parse(bad), None, "{bad}");
        }
    }

    #[test]
    fn an_instance_moves_to_a_tested_version_one_minor_at_a_time() {
        assert_eq!(check(None, "1.12.3", None), Ok(()));
        assert_eq!(check(Some("1.12.2"), "1.12.3", None), Ok(()));
        assert_eq!(check(Some("1.12.3"), "1.12.3", None), Ok(()));
        // Untested, older, too far: each lifted by the approval.
        for (current, to) in [
            (None, "1.13.0"),
            (Some("1.12.3"), "latest"),
            (Some("1.12.3"), "1.12.2"),
        ] {
            let refused = check(current, to, None).unwrap_err();
            assert!(refused.0.contains("approved-upgrade"), "{refused}");
            assert_eq!(check(current, to, Some(to)), Ok(()));
        }
        assert!(
            check(Some("1.10.0"), "1.12.3", None)
                .unwrap_err()
                .0
                .contains("skips a minor")
        );
    }

    #[test]
    fn the_steps_scrub_before_and_check_after_the_rollout() {
        let s3 = instance_of(
            "reg",
            &serde_json::from_value(json!({"storage": {"s3": {"endpoint": "http://s3",
                "bucket": "b", "region": "r", "credentialsSecretRef": {"name": "s3"}}}}))
            .unwrap(),
        );
        let mut step = first_step(&s3);
        let mut steps = vec![step];
        while let Some(n) = next(step, &s3) {
            steps.push(n);
            step = n;
        }
        assert_eq!(
            steps,
            [
                UpgradeStep::ScrubDryRun,
                UpgradeStep::Scrub,
                UpgradeStep::Rollout,
                UpgradeStep::Verify,
                UpgradeStep::ReconcileIndex
            ]
        );
        // An emptyDir store: no Job reaches it, the pods just roll.
        let empty = instance_of(
            "reg",
            &serde_json::from_value(json!({"storage": {"fs": {}}})).unwrap(),
        );
        assert_eq!(first_step(&empty), UpgradeStep::Rollout);
        assert_eq!(next(UpgradeStep::Rollout, &empty), None);
        let name = job_name("reg", "1.12.3", UpgradeStep::Scrub, 0);
        assert!(name.starts_with("reg-up-scrub-"), "{name}");
        assert_ne!(name, job_name("reg", "1.12.3", UpgradeStep::Scrub, 1));
        let job = job(
            &s3,
            "team",
            UpgradeStep::ScrubDryRun,
            &name,
            "img:1.12.2",
            "h",
        )
        .unwrap();
        let args = &job.manifest["spec"]["template"]["spec"]["containers"][0]["args"];
        assert_eq!(args[4], "scrub");
        assert_eq!(args[5], "-d");
        assert!(super::job(&s3, "team", UpgradeStep::Rollout, &name, "i", "h").is_none());
    }
}
