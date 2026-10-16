//! Whether a pod loaded the hot layer the operator wrote (spec 2.5, 6.1):
//! Angos has no signal for it but its log, JSON lines of `tracing`. The
//! messages are those of Angos 1.12.3, checked against the image by the
//! live tests:
//!
//! | target | level | message | meaning |
//! | --- | --- | --- | --- |
//! | `angos::configuration::watcher` | INFO | `Configuration reloaded` | applied |
//! | `angos::configuration::watcher` | WARN | `Failed to reload configuration: <error>` | refused at load |
//! | `angos::command::server::command::notifier` | ERROR | `Failed to apply configuration: <error>` | refused by a component… |
//! | `angos::configuration::watcher` | WARN | `Configuration was not applied; keeping the previous one` | …and kept the old one |
//!
//! A message carries no hash: what counts is the last reload logged after
//! the write. A pod that started after the write read the file at start.

use api::status::PodReloadState;
use k8s_openapi::jiff::{SignedDuration, Timestamp};
use weebo_kit_application::workload::{LogLine, PodState};

/// The target of the watcher's messages (what `RUST_LOG` must let through
/// at `info`).
pub const WATCHER_TARGET: &str = "angos::configuration::watcher";
pub const RELOADED: &str = "Configuration reloaded";
pub const FAILED_TO_RELOAD: &str = "Failed to reload configuration: ";
pub const FAILED_TO_APPLY: &str = "Failed to apply configuration: ";
pub const NOT_APPLIED: &str = "Configuration was not applied; keeping the previous one";

/// How far before the write the logs are read: the kubelet's clock and the
/// operator's differ a little.
pub const LOG_SLACK: SignedDuration = SignedDuration::from_secs(5);

/// What a pod's log says of the last reload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    Applied,
    /// Refused, with Angos's error (redacted).
    Rejected(String),
}

/// One line of Angos's log, as `tracing`'s JSON formatter writes it.
#[derive(Debug, serde::Deserialize)]
struct Entry {
    #[serde(default)]
    level: String,
    #[serde(default)]
    target: String,
    #[serde(default)]
    fields: Fields,
}

#[derive(Debug, Default, serde::Deserialize)]
struct Fields {
    #[serde(default)]
    message: String,
}

/// What a refusal says when Angos panicked reloading (cel-parser 0.10.1
/// panics on a CEL syntax error): its watcher is dead, only a restart
/// brings it back.
pub const CRASHED: &str = "Angos crashed reloading it (its configuration watcher stopped)";

/// The last reload `lines` report after `written` (exclusive), `None` when
/// none happened yet.
pub fn verdict(lines: &[LogLine], written: Timestamp) -> Option<Verdict> {
    let mut last = None;
    let mut apply_error: Option<String> = None;
    for line in lines.iter().filter(|l| l.time > written) {
        let Ok(entry) = serde_json::from_str::<Entry>(&line.text) else {
            // A panic is printed as plain text, not by `tracing`.
            if line.text.contains("panicked at") {
                last = Some(Verdict::Rejected(CRASHED.into()));
            }
            continue;
        };
        let message = entry.fields.message.as_str();
        if entry.target == WATCHER_TARGET {
            if message == RELOADED {
                last = Some(Verdict::Applied);
                apply_error = None;
            } else if let Some(error) = message.strip_prefix(FAILED_TO_RELOAD) {
                last = Some(Verdict::Rejected(redact(error)));
            } else if message == NOT_APPLIED {
                last =
                    Some(Verdict::Rejected(apply_error.take().unwrap_or_else(|| {
                        "a component refused the configuration".into()
                    })));
            }
        } else if entry.level == "ERROR"
            && let Some(error) = message.strip_prefix(FAILED_TO_APPLY)
        {
            apply_error = Some(redact(error));
        }
    }
    last
}

/// Why the pod's configuration watcher stopped, if its log says it did
/// (`Config watcher failed: …`, e.g. no inotify instance left on the
/// node): such a pod never reloads, only a restart gives it a new file.
pub fn watcher_failure(lines: &[LogLine]) -> Option<String> {
    lines.iter().find_map(|line| {
        let entry = serde_json::from_str::<Entry>(&line.text).ok()?;
        (entry.target == WATCHER_TARGET && entry.level == "ERROR")
            .then(|| redact(&entry.fields.message))
    })
}

/// How much of a pod's log, from its start, is read for its watcher's
/// failure.
pub const STARTUP_LOG_BYTES: i64 = 64 * 1024;

/// The last `ERROR` line of `lines`, redacted: why a pod doesn't start.
pub fn last_error(lines: &[LogLine]) -> Option<String> {
    lines.iter().rev().find_map(|line| {
        let entry = serde_json::from_str::<Entry>(&line.text).ok()?;
        (entry.level == "ERROR").then(|| redact(&entry.fields.message))
    })
}

/// Angos's error as the status may show it: its first line, without the
/// quoted values (a parse error quotes the file, secrets included), at
/// most 300 characters.
pub fn redact(error: &str) -> String {
    let first = error.lines().next().unwrap_or_default();
    let mut out = String::new();
    let mut quoted = None;
    for c in first.chars() {
        match quoted {
            Some(q) if c == q => {
                out.push('…');
                out.push(c);
                quoted = None;
            }
            Some(_) => {}
            None => {
                out.push(c);
                if c == '"' || c == '\'' {
                    quoted = Some(c);
                }
            }
        }
    }
    if quoted.is_some() {
        out.push('…');
    }
    if out.chars().count() > 300 {
        out = out.chars().take(299).collect::<String>() + "…";
    }
    out
}

/// A pod's answer to the hot layer written at `written`: started after it
/// (it read it at start, once ready), or what its log says.
pub fn pod_answer(
    pod: &PodState,
    written: Timestamp,
    log: Option<&[LogLine]>,
) -> (PodReloadState, Option<String>) {
    if pod.started.is_some_and(|started| started > written) {
        return if pod.ready {
            (PodReloadState::Restarted, None)
        } else {
            (PodReloadState::Pending, None)
        };
    }
    match log.and_then(|lines| verdict(lines, written)) {
        Some(Verdict::Applied) => (PodReloadState::Applied, None),
        Some(Verdict::Rejected(why)) => (PodReloadState::Rejected, Some(why)),
        None => (PodReloadState::Pending, None),
    }
}

/// Where the confirmation of a hot layer stands, over every ready pod.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Confirmation {
    /// Every ready pod has it.
    Applied,
    /// A pod refused it: its name and Angos's error.
    Rejected { pod: String, why: String },
    /// Some pods haven't answered (named).
    Pending(Vec<String>),
}

/// The pods' answers taken together.
pub fn confirmation(answers: &[(String, PodReloadState, Option<String>)]) -> Confirmation {
    if let Some((pod, _, why)) = answers
        .iter()
        .find(|(_, state, _)| *state == PodReloadState::Rejected)
    {
        return Confirmation::Rejected {
            pod: pod.clone(),
            why: why.clone().unwrap_or_default(),
        };
    }
    let pending: Vec<String> = answers
        .iter()
        .filter(|(_, state, _)| *state == PodReloadState::Pending)
        .map(|(pod, _, _)| pod.clone())
        .collect();
    if pending.is_empty() && !answers.is_empty() {
        Confirmation::Applied
    } else {
        Confirmation::Pending(pending)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(s: &str) -> Timestamp {
        s.parse().unwrap()
    }

    fn line(time: &str, level: &str, target: &str, message: &str) -> LogLine {
        LogLine {
            time: at(time),
            text: serde_json::json!({
                "timestamp": time, "level": level,
                "fields": {"message": message}, "target": target,
            })
            .to_string(),
        }
    }

    const W: &str = WATCHER_TARGET;
    const NOTIFIER: &str = "angos::command::server::command::notifier";

    #[test]
    fn the_last_reload_after_the_write_counts() {
        let written = at("2026-10-16T10:00:00Z");
        let lines = [
            // Before the write: someone else's.
            line("2026-10-16T09:59:58Z", "INFO", W, RELOADED),
            line(
                "2026-10-16T10:00:30Z",
                "INFO",
                W,
                "Configuration change detected, reloading",
            ),
            line(
                "2026-10-16T10:00:30Z",
                "INFO",
                "angos::command::bootstrap",
                "Using filesystem storage backend",
            ),
            line("2026-10-16T10:00:31Z", "INFO", W, RELOADED),
        ];
        assert_eq!(verdict(&lines[..1], written), None);
        assert_eq!(verdict(&lines, written), Some(Verdict::Applied));
        // A TLS reload is not the configuration's.
        let tls = [line(
            "2026-10-16T10:00:31Z",
            "INFO",
            W,
            "TLS configuration reloaded",
        )];
        assert_eq!(verdict(&tls, written), None);
    }

    #[test]
    fn both_refusals_are_read_with_angos_s_error() {
        let written = at("2026-10-16T10:00:00Z");
        // Refused at load (the real 1.12.3 message).
        let load = [line(
            "2026-10-16T10:00:04Z",
            "WARN",
            W,
            "Failed to reload configuration: invalid configuration format: global.max_manifest_size must be greater than zero\nmerged from /etc/angos/boot/config.toml, /etc/angos/hot/config.toml",
        )];
        assert_eq!(
            verdict(&load, written),
            Some(Verdict::Rejected(
                "invalid configuration format: global.max_manifest_size must be greater than zero"
                    .into()
            ))
        );
        // Refused by a component: the ERROR line before says why.
        let apply = [
            line(
                "2026-10-16T10:00:06Z",
                "ERROR",
                NOTIFIER,
                "Failed to apply configuration: auth.token_service.secret_key must decode to at least 32 bytes",
            ),
            line("2026-10-16T10:00:06Z", "WARN", W, NOT_APPLIED),
        ];
        assert_eq!(
            verdict(&apply, written),
            Some(Verdict::Rejected(
                "auth.token_service.secret_key must decode to at least 32 bytes".into()
            ))
        );
        // A refusal then a success: the success counts.
        let both = [
            apply[0].clone(),
            apply[1].clone(),
            line("2026-10-16T10:00:09Z", "INFO", W, RELOADED),
        ];
        assert_eq!(verdict(&both, written), Some(Verdict::Applied));
    }

    #[test]
    fn quoted_values_never_reach_the_status() {
        assert_eq!(
            redact("TOML parse error at line 3: secret_key = \"s3cr3t\" is 'bad'\nmore"),
            "TOML parse error at line 3: secret_key = \"…\" is '…'"
        );
        assert_eq!(redact("unterminated \"abc"), "unterminated \"…");
        assert_eq!(redact(&"x".repeat(400)).chars().count(), 300);
        let lines = [
            line(
                "2026-10-16T10:00:01Z",
                "ERROR",
                "angos",
                "Failed to bind to 0.0.0.0:8000: \"in use\"",
            ),
            line("2026-10-16T10:00:02Z", "INFO", W, "noise"),
        ];
        assert_eq!(
            last_error(&lines).as_deref(),
            Some("Failed to bind to 0.0.0.0:8000: \"…\"")
        );
    }

    #[test]
    fn a_panic_while_reloading_is_a_refusal() {
        let written = at("2026-10-16T10:00:00Z");
        let lines = [
            line("2026-10-16T10:00:02Z", "INFO", W, "Configuration change detected, reloading"),
            LogLine {
                time: at("2026-10-16T10:00:02Z"),
                text: "thread 'tokio-rt-worker' (64) panicked at /usr/local/cargo/registry/src/antlr4rust-0.3.0-rc2/src/tree.rs:383:9:".into(),
            },
        ];
        assert_eq!(
            verdict(&lines, written),
            Some(Verdict::Rejected(CRASHED.into()))
        );
    }

    #[test]
    fn a_dead_watcher_is_read_from_the_pod_s_start() {
        let lines = [
            line(
                "2026-10-16T10:00:00Z",
                "INFO",
                W,
                "Setting up config watcher for: [\"/etc/angos/hot/config.toml\"]",
            ),
            line(
                "2026-10-16T10:00:00Z",
                "ERROR",
                W,
                "Config watcher failed: unable to read configuration: Watcher error: No file descriptors available (os error 24)",
            ),
        ];
        assert!(watcher_failure(&lines).unwrap().contains("os error 24"));
        assert_eq!(watcher_failure(&lines[..1]), None);
    }

    #[test]
    fn a_pod_started_after_the_write_read_it_at_start() {
        let written = at("2026-10-16T10:00:00Z");
        let pod = PodState {
            name: "p".into(),
            ready: true,
            started: Some(at("2026-10-16T10:01:00Z")),
            ..Default::default()
        };
        assert_eq!(pod_answer(&pod, written, None).0, PodReloadState::Restarted);
        let starting = PodState {
            ready: false,
            ..pod.clone()
        };
        assert_eq!(
            pod_answer(&starting, written, None).0,
            PodReloadState::Pending
        );
        let older = PodState {
            started: Some(at("2026-10-16T09:00:00Z")),
            ..pod
        };
        assert_eq!(
            pod_answer(&older, written, Some(&[])).0,
            PodReloadState::Pending
        );
        let lines = [line("2026-10-16T10:00:40Z", "INFO", W, RELOADED)];
        assert_eq!(
            pod_answer(&older, written, Some(&lines)).0,
            PodReloadState::Applied
        );
    }

    #[test]
    fn one_refusal_rejects_and_every_answer_applies() {
        use PodReloadState::*;
        let answers = |states: &[PodReloadState]| -> Vec<(String, PodReloadState, Option<String>)> {
            states
                .iter()
                .enumerate()
                .map(|(i, s)| (format!("p{i}"), *s, (*s == Rejected).then(|| "why".into())))
                .collect()
        };
        assert_eq!(
            confirmation(&answers(&[Applied, Restarted])),
            Confirmation::Applied
        );
        assert_eq!(
            confirmation(&answers(&[Applied, Pending])),
            Confirmation::Pending(vec!["p1".into()])
        );
        assert_eq!(
            confirmation(&answers(&[Pending, Rejected])),
            Confirmation::Rejected {
                pod: "p1".into(),
                why: "why".into()
            }
        );
        // No ready pod: nothing is confirmed.
        assert_eq!(confirmation(&[]), Confirmation::Pending(vec![]));
    }
}
