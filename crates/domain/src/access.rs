//! The access vocabulary (spec 2.6, weebo-si's `_access.tpl` word for word):
//! subjects and verbs compiled into Angos's CEL. Rules are rendered as TOML
//! literal strings (apostrophes), so the CEL inside uses double quotes and
//! a name may hold neither quote.

use std::fmt;

/// Who a grant is for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Subject {
    /// Anyone, unauthenticated included.
    Anonymous,
    /// Any identity: Basic, OIDC or a client certificate.
    Authenticated,
    /// Any token of the OIDC provider `name`.
    Provider(String),
    /// The Basic identity of that username.
    Identity(String),
    /// An OIDC token whose `groups` claim holds the group.
    Group(String),
    /// Any ServiceAccount token of the namespace (the instance's
    /// `auth.kubernetes` provider).
    Namespace(String),
    /// The ServiceAccount `namespace:name`.
    ServiceAccount(String, String),
}

/// Why a subject doesn't read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubjectError(pub String);

impl fmt::Display for SubjectError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

fn name(text: &str, what: &str, subject: &str) -> Result<String, SubjectError> {
    if text.is_empty() {
        return Err(SubjectError(format!("{subject:?}: the {what} is empty")));
    }
    if text.contains(['\'', '"', '\\']) || text.chars().any(char::is_control) {
        return Err(SubjectError(format!(
            "{subject:?}: the {what} may hold no quote, backslash or control character"
        )));
    }
    Ok(text.to_string())
}

impl Subject {
    /// Reads `anonymous`, `authenticated`, `provider:<n>`, `identity:<n>`,
    /// `group:<g>`, `namespace:<ns>`, `serviceaccount:<ns>:<sa>`.
    pub fn parse(subject: &str) -> Result<Self, SubjectError> {
        match subject {
            "anonymous" => return Ok(Subject::Anonymous),
            "authenticated" => return Ok(Subject::Authenticated),
            _ => {}
        }
        let Some((kind, rest)) = subject.split_once(':') else {
            return Err(SubjectError(format!(
                "{subject:?} is not a subject: anonymous, authenticated, provider:<name>, \
                 identity:<username>, group:<group>, namespace:<namespace> or \
                 serviceaccount:<namespace>:<name>"
            )));
        };
        Ok(match kind {
            "provider" => Subject::Provider(name(rest, "provider", subject)?),
            "identity" => Subject::Identity(name(rest, "username", subject)?),
            "group" => Subject::Group(name(rest, "group", subject)?),
            "namespace" => Subject::Namespace(name(rest, "namespace", subject)?),
            "serviceaccount" => {
                let (ns, sa) = rest.split_once(':').ok_or_else(|| {
                    SubjectError(format!("{subject:?}: serviceaccount:<namespace>:<name>"))
                })?;
                Subject::ServiceAccount(name(ns, "namespace", subject)?, name(sa, "name", subject)?)
            }
            _ => {
                return Err(SubjectError(format!(
                    "{subject:?}: unknown kind of subject {kind:?}"
                )));
            }
        })
    }

    /// Whether it may stand for anyone of a provider at least: never for
    /// `jobs` nor `admin`.
    pub fn is_broad(&self) -> bool {
        matches!(
            self,
            Subject::Anonymous | Subject::Authenticated | Subject::Provider(_)
        )
    }

    /// Whether it needs the instance's `auth.kubernetes` provider.
    pub fn needs_kubernetes(&self) -> bool {
        matches!(self, Subject::Namespace(_) | Subject::ServiceAccount(..))
    }

    /// The CEL condition, `kube` the name of the instance's ServiceAccount
    /// provider.
    pub fn cel(&self, kube: &str) -> String {
        match self {
            Subject::Anonymous => "true".into(),
            Subject::Authenticated => "identity.username != null || identity.oidc != null || \
                 size(identity.certificate.common_names) > 0"
                .into(),
            Subject::Provider(n) => {
                format!("identity.oidc != null && identity.oidc.provider_name == \"{n}\"")
            }
            Subject::Identity(n) => format!("identity.username == \"{n}\""),
            Subject::Group(g) => format!(
                "identity.oidc != null && \"groups\" in identity.oidc.claims && \"{g}\" in \
                 identity.oidc.claims[\"groups\"]"
            ),
            Subject::Namespace(ns) => format!(
                "identity.oidc != null && identity.oidc.provider_name == \"{kube}\" && \
                 identity.oidc.claims[\"sub\"].startsWith(\"system:serviceaccount:{ns}:\")"
            ),
            Subject::ServiceAccount(ns, sa) => format!(
                "identity.oidc != null && identity.oidc.provider_name == \"{kube}\" && \
                 identity.oidc.claims[\"sub\"] == \"system:serviceaccount:{ns}:{sa}\""
            ),
        }
    }
}

/// The repository verbs and Angos's actions they grant.
pub const PULL: &[&str] = &["get-manifest", "get-blob", "get-referrers", "list-tags"];
pub const BROWSE: &[&str] = &["list-revisions", "list-uploads"];
pub const PUSH: &[&str] = &[
    "start-upload",
    "update-upload",
    "complete-upload",
    "cancel-upload",
    "get-upload",
    "mount-blob",
    "put-manifest",
];
pub const DELETE: &[&str] = &["delete-manifest", "delete-blob"];

/// The instance's global verbs.
pub const CATALOG: &[&str] = &["list-catalog", "list-repositories", "list-namespaces"];
pub const JOBS: &[&str] = &["list-jobs", "list-failed-jobs", "retry-job", "delete-job"];
pub const METRICS: &[&str] = &["metrics"];

/// A verb: its name in messages and the actions it grants (`None`: every
/// action, `admin`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Verb {
    pub name: &'static str,
    pub actions: Option<&'static [&'static str]>,
}

pub const VERB_PULL: Verb = Verb {
    name: "pull",
    actions: Some(PULL),
};
pub const VERB_BROWSE: Verb = Verb {
    name: "browse",
    actions: Some(BROWSE),
};
pub const VERB_PUSH: Verb = Verb {
    name: "push",
    actions: Some(PUSH),
};
pub const VERB_DELETE: Verb = Verb {
    name: "delete",
    actions: Some(DELETE),
};
pub const VERB_CATALOG: Verb = Verb {
    name: "catalog",
    actions: Some(CATALOG),
};
pub const VERB_JOBS: Verb = Verb {
    name: "jobs",
    actions: Some(JOBS),
};
pub const VERB_ADMIN: Verb = Verb {
    name: "admin",
    actions: None,
};
pub const VERB_METRICS: Verb = Verb {
    name: "metrics",
    actions: Some(METRICS),
};

/// The rule granting `verb` to `subjects` (none: no rule).
pub fn grant(verb: Verb, subjects: &[Subject], kube: &str) -> Option<String> {
    if subjects.is_empty() {
        return None;
    }
    let who = if subjects.contains(&Subject::Anonymous) {
        "true".to_string()
    } else if let [only] = subjects {
        only.cel(kube)
    } else {
        subjects
            .iter()
            .map(|s| format!("({})", s.cel(kube)))
            .collect::<Vec<_>>()
            .join(" || ")
    };
    Some(match verb.actions {
        None => who,
        Some(actions) => {
            let list = actions
                .iter()
                .map(|a| format!("\"{a}\""))
                .collect::<Vec<_>>()
                .join(", ");
            if who == "true" {
                format!("request.action in [{list}]")
            } else {
                format!("request.action in [{list}] && ({who})")
            }
        }
    })
}

/// Every subject of `subjects` read, the first problem otherwise; `broad`
/// refused when the verb is `jobs` or `admin`, and the kubernetes ones
/// without its provider.
pub fn subjects(
    field: &str,
    verb: Verb,
    subjects: &[String],
    kubernetes: bool,
) -> Result<Vec<Subject>, SubjectError> {
    subjects
        .iter()
        .map(|text| {
            let subject =
                Subject::parse(text).map_err(|e| SubjectError(format!("{field}: {e}")))?;
            if subject.is_broad() && matches!(verb.name, "jobs" | "admin") {
                return Err(SubjectError(format!(
                    "{field}: {text:?} is too broad for {}: name identities, groups or \
                     ServiceAccounts",
                    verb.name
                )));
            }
            if subject.needs_kubernetes() && !kubernetes {
                return Err(SubjectError(format!(
                    "{field}: {text:?} needs the instance's spec.auth.kubernetes"
                )));
            }
            Ok(subject)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn subjects_read_as_weebo_si_writes_them() {
        assert_eq!(Subject::parse("anonymous"), Ok(Subject::Anonymous));
        assert_eq!(
            Subject::parse("serviceaccount:ci:builder"),
            Ok(Subject::ServiceAccount("ci".into(), "builder".into()))
        );
        assert_eq!(
            Subject::parse("group:devs"),
            Ok(Subject::Group("devs".into()))
        );
        for bad in [
            "nobody",
            "group:",
            "group:a'b",
            "identity:a\"b",
            "serviceaccount:ci",
            "role:x",
        ] {
            assert!(Subject::parse(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn grants_compile_to_the_vocabulary_s_cel() {
        assert_eq!(grant(VERB_PULL, &[], "kube"), None);
        assert_eq!(
            grant(VERB_PULL, &[Subject::Anonymous], "kube").unwrap(),
            "request.action in [\"get-manifest\", \"get-blob\", \"get-referrers\", \"list-tags\"]"
        );
        assert_eq!(
            grant(
                VERB_PUSH,
                &[
                    Subject::Identity("ci".into()),
                    Subject::Namespace("team".into())
                ],
                "kube"
            )
            .unwrap(),
            "request.action in [\"start-upload\", \"update-upload\", \"complete-upload\", \
             \"cancel-upload\", \"get-upload\", \"mount-blob\", \"put-manifest\"] && \
             ((identity.username == \"ci\") || (identity.oidc != null && \
             identity.oidc.provider_name == \"kube\" && \
             identity.oidc.claims[\"sub\"].startsWith(\"system:serviceaccount:team:\")))"
        );
        assert_eq!(
            grant(VERB_ADMIN, &[Subject::Group("ops".into())], "kube").unwrap(),
            Subject::Group("ops".into()).cel("kube")
        );
        // Rendered as TOML literal strings: never an apostrophe.
        for verb in [VERB_PULL, VERB_ADMIN, VERB_JOBS] {
            assert!(
                !grant(verb, &[Subject::Authenticated], "kube")
                    .unwrap()
                    .contains('\'')
            );
        }
    }

    #[test]
    fn jobs_and_admin_name_who_they_trust() {
        let s = |list: &[&str]| list.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert!(subjects("x", VERB_JOBS, &s(&["authenticated"]), false).is_err());
        assert!(subjects("x", VERB_ADMIN, &s(&["provider:weebo"]), false).is_err());
        assert!(subjects("x", VERB_ADMIN, &s(&["group:ops"]), false).is_ok());
        assert!(subjects("x", VERB_PULL, &s(&["anonymous"]), false).is_ok());
        assert!(subjects("x", VERB_PULL, &s(&["namespace:ci"]), false).is_err());
        assert!(subjects("x", VERB_PULL, &s(&["namespace:ci"]), true).is_ok());
    }
}
