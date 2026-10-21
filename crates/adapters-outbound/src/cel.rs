//! [`CelChecker`] on `cel-interpreter` **0.10.0** — Angos 1.12.3's engine
//! (spec 2.6): a rule is compiled as Angos compiles it
//! (`CelRule::compile`: blank refused, then `Program::compile`), then
//! **evaluated** against synthetic contexts shaped as Angos builds them
//! (`src/policy/access_policy.rs:109-119`, `retention_policy.rs:180-232`):
//! a reference to a variable or a function that doesn't exist is refused;
//! an error on one plausible context is reported (in an access policy an
//! error is a denial).
//!
//! `cel-parser` 0.10.1 **panics** on a syntax error rather than returning
//! it: so does Angos when it loads such a rule. A rule is never written
//! without passing here.

use application::ports::{CelChecker, RuleContext, RuleVerdict};
use cel_interpreter::{Context, ExecutionError, Program, Value};
use serde_json::{Value as Json, json};

pub struct CelInterpreterChecker;

/// The identities a rule is tried with.
fn identities() -> Vec<(&'static str, Json)> {
    let none = json!({"organizations": [], "common_names": []});
    vec![
        (
            "an anonymous request",
            json!({"id": null, "username": null, "certificate": none, "oidc": null, "client_ip": "10.0.0.1"}),
        ),
        (
            "a Basic identity",
            json!({"id": "ci", "username": "ci", "certificate": none, "oidc": null, "client_ip": "10.0.0.1"}),
        ),
        (
            "an OIDC token with groups",
            json!({"id": null, "username": null, "certificate": none, "client_ip": "10.0.0.1",
                   "oidc": {"provider_name": "weebo", "claims": {"sub": "u", "groups": ["devs"], "email": "u@example.com"}}}),
        ),
        (
            "an OIDC token without groups",
            json!({"id": null, "username": null, "certificate": none, "client_ip": "10.0.0.1",
                   "oidc": {"provider_name": "kube", "claims": {"sub": "system:serviceaccount:team:ci"}}}),
        ),
        (
            "a client certificate",
            json!({"id": null, "username": null, "client_ip": "10.0.0.1", "oidc": null,
                   "certificate": {"organizations": ["org"], "common_names": ["cn"]}}),
        ),
    ]
}

/// One request per action, with the fields Angos gives it
/// (`src/identity/action/mod.rs`).
fn requests() -> Vec<Json> {
    let ns = "team/app";
    let digest = "sha256:0000000000000000000000000000000000000000000000000000000000000000";
    vec![
        json!({"action": "healthz"}),
        json!({"action": "readyz"}),
        json!({"action": "metrics"}),
        json!({"action": "ui-asset"}),
        json!({"action": "ui-config"}),
        json!({"action": "get-token"}),
        json!({"action": "get-api-version"}),
        json!({"action": "list-repositories"}),
        json!({"action": "list-catalog", "n": 10u64}),
        json!({"action": "list-tags", "namespace": ns, "n": 10u64}),
        json!({"action": "start-upload", "namespace": ns}),
        json!({"action": "mount-blob", "namespace": ns, "digest": digest, "from": "other/app"}),
        json!({"action": "update-upload", "namespace": ns, "session_id": "s"}),
        json!({"action": "complete-upload", "namespace": ns, "digest": digest, "session_id": "s"}),
        json!({"action": "get-blob", "namespace": ns, "digest": digest}),
        json!({"action": "delete-blob", "namespace": ns, "digest": digest}),
        json!({"action": "get-manifest", "namespace": ns, "reference": "latest"}),
        json!({"action": "delete-manifest", "namespace": ns, "reference": "latest"}),
        json!({"action": "put-manifest", "namespace": ns, "tags": ["latest"]}),
        json!({"action": "get-referrers", "namespace": ns, "digest": digest}),
        json!({"action": "list-revisions", "namespace": ns}),
        json!({"action": "list-uploads", "namespace": ns}),
        json!({"action": "list-namespaces", "repository": "team"}),
        json!({"action": "list-jobs", "queue": "cache"}),
        json!({"action": "retry-job", "queue": "cache"}),
    ]
}

/// Images an image rule is tried with: tagged or not, pulled or not.
fn images() -> Vec<(&'static str, Json)> {
    vec![
        (
            "a tagged image",
            json!({"namespace": "team/app", "tag": "v1", "pushed_at": 1_700_000_000i64,
                   "last_pulled_at": 1_700_100_000i64, "scanned_at": 0i64}),
        ),
        (
            "an untagged image",
            json!({"namespace": "team/app", "tag": null, "pushed_at": 1_700_000_000i64,
                   "last_pulled_at": 0i64, "scanned_at": 0i64}),
        ),
    ]
}

fn days(n: i64) -> i64 {
    n.saturating_mul(86_400)
}

fn hours(n: i64) -> i64 {
    n.saturating_mul(3_600)
}

fn minutes(n: i64) -> i64 {
    n.saturating_mul(60)
}

fn top(_n: i64) -> bool {
    true
}

/// An error naming something that doesn't exist: the rule is wrong
/// whatever the request.
fn undeclared(error: &ExecutionError) -> bool {
    matches!(error, ExecutionError::UndeclaredReference(_))
        || matches!(error, ExecutionError::FunctionError { .. })
            && error.to_string().contains("Undeclared")
}

impl CelInterpreterChecker {
    #[expect(
        clippy::expect_used,
        reason = "the variables are serde_json::Values built here, which always serialize"
    )]
    fn contexts(rule_context: RuleContext) -> Vec<(String, Context<'static>)> {
        let mut out = Vec::new();
        match rule_context {
            RuleContext::Access => {
                for (who, identity) in identities() {
                    for request in requests() {
                        let mut context = Context::default();
                        context
                            .add_variable("request", request.clone())
                            .expect("a request serializes");
                        context
                            .add_variable("identity", identity.clone())
                            .expect("an identity serializes");
                        context.add_function("has_repository_policy", || true);
                        out.push((
                            format!(
                                "{who} asking {}",
                                request.get("action").and_then(|a| a.as_str()).unwrap_or("")
                            ),
                            context,
                        ));
                    }
                }
            }
            RuleContext::Image => {
                for (what, image) in images() {
                    let mut context = Context::default();
                    context
                        .add_variable("image", image)
                        .expect("an image serializes");
                    context.add_function("now", || 1_700_200_000i64);
                    context.add_function("days", days);
                    context.add_function("hours", hours);
                    context.add_function("minutes", minutes);
                    context.add_function("top_pushed", top);
                    context.add_function("top_pulled", top);
                    out.push((what.to_string(), context));
                }
            }
        }
        out
    }
}

impl CelChecker for CelInterpreterChecker {
    fn check(&self, rule_context: RuleContext, rule: &str) -> RuleVerdict {
        if rule.trim().is_empty() {
            return RuleVerdict::Refused("CEL rule cannot be empty".into());
        }
        // cel-parser 0.10.1 panics on a syntax error instead of returning
        // it (as it would inside Angos, at load): caught, and refused.
        let compiled = std::panic::catch_unwind(|| Program::compile(rule));
        let program = match compiled {
            Ok(Ok(program)) => program,
            Ok(Err(e)) => return RuleVerdict::Refused(format!("doesn't compile: {e}")),
            Err(_) => {
                return RuleVerdict::Refused(
                    "is not valid CEL (Angos's parser would crash on it)".into(),
                );
            }
        };
        let mut may_fail = None;
        for (what, context) in Self::contexts(rule_context) {
            match program.execute(&context) {
                Ok(Value::Bool(_)) => {}
                Ok(value) => {
                    return RuleVerdict::Refused(format!(
                        "is not a boolean (a {} on {what})",
                        value.type_of()
                    ));
                }
                Err(e) if undeclared(&e) => {
                    return RuleVerdict::Refused(format!("names what Angos doesn't define: {e}"));
                }
                Err(e) => {
                    may_fail.get_or_insert_with(|| format!("fails on {what}: {e}"));
                }
            }
        }
        match may_fail {
            Some(why) => RuleVerdict::MayFail(why),
            None => RuleVerdict::Ok,
        }
    }
}

#[cfg(test)]
mod tests {
    use domain::access::{
        Subject, VERB_ADMIN, VERB_CATALOG, VERB_DELETE, VERB_JOBS, VERB_METRICS, VERB_PULL,
        VERB_PUSH, grant,
    };

    use super::*;

    fn check(context: RuleContext, rule: &str) -> RuleVerdict {
        CelInterpreterChecker.check(context, rule)
    }

    #[test]
    fn every_rule_of_the_vocabulary_evaluates_everywhere() {
        let subjects = [
            Subject::Anonymous,
            Subject::Authenticated,
            Subject::Provider("weebo".into()),
            Subject::Identity("ci".into()),
            Subject::Group("devs".into()),
            Subject::Namespace("team".into()),
            Subject::ServiceAccount("team".into(), "ci".into()),
        ];
        for verb in [
            VERB_PULL,
            VERB_PUSH,
            VERB_DELETE,
            VERB_CATALOG,
            VERB_JOBS,
            VERB_ADMIN,
            VERB_METRICS,
        ] {
            for subject in &subjects {
                let rule = grant(verb, std::slice::from_ref(subject), "kube").unwrap();
                assert_eq!(check(RuleContext::Access, &rule), RuleVerdict::Ok, "{rule}");
            }
            let all = grant(verb, &subjects[1..], "kube").unwrap();
            assert_eq!(check(RuleContext::Access, &all), RuleVerdict::Ok, "{all}");
        }
        // The operator's own rules.
        for rule in application::render::builtin_rules(true) {
            assert_eq!(check(RuleContext::Access, &rule), RuleVerdict::Ok, "{rule}");
        }
        assert_eq!(
            check(RuleContext::Access, application::render::REPOSITORY_RULE),
            RuleVerdict::Ok
        );
    }

    #[test]
    fn what_angos_would_refuse_is_refused() {
        for rule in [
            "",
            "  ",
            "request.action ==",
            "request.action",
            "unknown.thing == 1",
            "nope()",
        ] {
            assert!(
                matches!(check(RuleContext::Access, rule), RuleVerdict::Refused(_)),
                "{rule:?}: {:?}",
                check(RuleContext::Access, rule)
            );
        }
        // An image function in an access policy, and the reverse.
        assert!(matches!(
            check(RuleContext::Access, "image.tag == \"v1\""),
            RuleVerdict::Refused(_)
        ));
        assert!(matches!(
            check(RuleContext::Image, "has_repository_policy()"),
            RuleVerdict::Refused(_)
        ));
    }

    #[test]
    fn a_rule_failing_on_some_requests_is_reported() {
        // `groups` is missing from some tokens.
        let rule = "identity.oidc != null && \"devs\" in identity.oidc.claims[\"groups\"]";
        assert!(
            matches!(check(RuleContext::Access, rule), RuleVerdict::MayFail(_)),
            "{:?}",
            check(RuleContext::Access, rule)
        );
        // `request.namespace` only exists for repository actions.
        assert!(matches!(
            check(
                RuleContext::Access,
                "request.namespace.startsWith(\"team/\")"
            ),
            RuleVerdict::MayFail(_)
        ));
    }

    #[test]
    fn retention_rules_evaluate_on_tagged_and_untagged_images() {
        for rule in [
            "top_pushed(10)",
            "image.pushed_at > now() - days(30)",
            "image.tag != null && image.tag.startsWith(\"v\")",
            "image.last_pulled_at > now() - hours(12) || top_pulled(3)",
        ] {
            assert_eq!(check(RuleContext::Image, rule), RuleVerdict::Ok, "{rule}");
        }
        // Untagged images have a null tag.
        assert!(matches!(
            check(RuleContext::Image, "image.tag.startsWith(\"v\")"),
            RuleVerdict::MayFail(_)
        ));
    }
}
