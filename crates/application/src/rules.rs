//! The access vocabulary and the CEL rules of a spec, compiled and checked
//! as Angos would run them (spec 2.6): every rule — the vocabulary's and
//! the raw ones — goes through the [`CelChecker`] (Angos's engine at its
//! version) before it is written. A rule Angos would refuse, or crash on,
//! is `InvalidSpec`; one that fails on a plausible request is reported
//! (`RuleMayFail`).

use api::config::GlobalAccess;
use api::repository::{ImagePolicy, IndexDefault, RepositoryAccess};
use domain::access::{
    self, Subject, VERB_ADMIN, VERB_BROWSE, VERB_CATALOG, VERB_DELETE, VERB_JOBS, VERB_METRICS,
    VERB_PULL, VERB_PUSH, Verb, grant,
};
use domain::config::{IndexAction, Policy};

use crate::ports::{CelChecker, RuleContext, RuleVerdict};

/// Rules ready to render, and what may fail in them.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Checked {
    pub rules: Vec<String>,
    /// `RuleMayFail` messages.
    pub warnings: Vec<String>,
}

/// What can't be rendered, field by field.
pub type Problems = Vec<String>;

/// The name of the instance's ServiceAccount provider, when it has one.
pub type Kube<'a> = Option<&'a str>;

fn check(
    checker: &dyn CelChecker,
    context: RuleContext,
    field: &str,
    rules: &[String],
    checked: &mut Checked,
    problems: &mut Problems,
) {
    for (i, rule) in rules.iter().enumerate() {
        match checker.check(context, rule) {
            RuleVerdict::Ok => checked.rules.push(rule.clone()),
            RuleVerdict::MayFail(why) => {
                checked.warnings.push(format!("{field}[{i}] {why}"));
                checked.rules.push(rule.clone());
            }
            RuleVerdict::Refused(why) => problems.push(format!("{field}[{i}] {why}")),
        }
    }
}

fn grants(
    field: &str,
    verbs: &[(Verb, &[String])],
    kube: Kube<'_>,
    problems: &mut Problems,
) -> Vec<String> {
    let mut rules = Vec::new();
    for (verb, list) in verbs {
        match access::subjects(
            &format!("{field}.{}", verb.name),
            *verb,
            list,
            kube.is_some(),
        ) {
            Ok(subjects) => rules.extend(grant(*verb, &subjects, kube.unwrap_or("kube"))),
            Err(e) => problems.push(e.0),
        }
    }
    rules
}

/// A repository's `access`: the vocabulary's grants then its raw rules, all
/// checked.
pub fn repository_access(
    access: &RepositoryAccess,
    kube: Kube<'_>,
    checker: &dyn CelChecker,
) -> Result<Checked, Problems> {
    let mut problems = Problems::new();
    let vocabulary = grants(
        "spec.access",
        &[
            (VERB_PULL, &access.pull),
            (VERB_BROWSE, &access.browse),
            (VERB_PUSH, &access.push),
            (VERB_DELETE, &access.delete),
        ],
        kube,
        &mut problems,
    );
    let mut checked = Checked::default();
    check(
        checker,
        RuleContext::Access,
        "spec.access (vocabulary)",
        &vocabulary,
        &mut checked,
        &mut problems,
    );
    check(
        checker,
        RuleContext::Access,
        "spec.access.rules",
        &access.rules,
        &mut checked,
        &mut problems,
    );
    if problems.is_empty() {
        Ok(checked)
    } else {
        Err(problems)
    }
}

/// The instance's `global.access`: its grants then its raw rules, checked
/// (the operator's own rules come around them).
pub fn global_access(
    access: &GlobalAccess,
    kube: Kube<'_>,
    checker: &dyn CelChecker,
) -> Result<Checked, Problems> {
    let mut problems = Problems::new();
    let vocabulary = grants(
        "spec.global.access",
        &[
            (VERB_CATALOG, &access.catalog),
            (VERB_JOBS, &access.jobs),
            (VERB_ADMIN, &access.admin),
            (VERB_METRICS, &access.metrics),
        ],
        kube,
        &mut problems,
    );
    let mut checked = Checked::default();
    check(
        checker,
        RuleContext::Access,
        "spec.global.access (vocabulary)",
        &vocabulary,
        &mut checked,
        &mut problems,
    );
    check(
        checker,
        RuleContext::Access,
        "spec.global.access.rules",
        &access.rules,
        &mut checked,
        &mut problems,
    );
    if problems.is_empty() {
        Ok(checked)
    } else {
        Err(problems)
    }
}

/// Image rules (retention, index), checked.
pub fn image_rules(
    field: &str,
    rules: &[String],
    checker: &dyn CelChecker,
) -> Result<Checked, Problems> {
    let mut problems = Problems::new();
    let mut checked = Checked::default();
    check(
        checker,
        RuleContext::Image,
        field,
        rules,
        &mut checked,
        &mut problems,
    );
    if problems.is_empty() {
        Ok(checked)
    } else {
        Err(problems)
    }
}

/// An `index` policy, its rules checked.
pub fn index_policy(
    field: &str,
    policy: &ImagePolicy,
    checker: &dyn CelChecker,
) -> Result<(Policy<IndexAction>, Vec<String>), Problems> {
    let checked = image_rules(&format!("{field}.rules"), &policy.rules, checker)?;
    Ok((
        Policy {
            default: policy.default.map(|d| match d {
                IndexDefault::Index => IndexAction::Index,
                IndexDefault::Skip => IndexAction::Skip,
            }),
            rules: checked.rules,
        },
        checked.warnings,
    ))
}

/// Subjects are read here too, for the webhook's messages.
pub fn parse_subject(subject: &str) -> Result<Subject, String> {
    Subject::parse(subject).map_err(|e| e.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::FakeCel;

    fn list(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn a_repository_s_vocabulary_and_rules_are_checked() {
        let access = RepositoryAccess {
            pull: list(&["anonymous"]),
            push: list(&["identity:ci", "group:devs"]),
            rules: list(&["request.action == \"list-tags\""]),
            ..Default::default()
        };
        let checked = repository_access(&access, None, &FakeCel::default()).unwrap();
        assert_eq!(checked.rules.len(), 3);
        assert!(checked.rules[0].starts_with("request.action in [\"get-manifest\""));
        assert!(checked.rules[1].contains("identity.username == \"ci\""));
        assert_eq!(checked.rules[2], "request.action == \"list-tags\"");
    }

    #[test]
    fn refused_rules_and_bad_subjects_are_problems() {
        let cel = FakeCel::default();
        cel.refuse("broken(");
        cel.may_fail("flaky");
        let access = RepositoryAccess {
            pull: list(&["namespace:ci"]),
            rules: list(&["broken(", "flaky"]),
            ..Default::default()
        };
        let problems = repository_access(&access, None, &cel).unwrap_err();
        assert_eq!(problems.len(), 2, "{problems:?}");
        assert!(problems[0].contains("auth.kubernetes"));
        assert!(problems[1].contains("spec.access.rules[0]"));
        // With the provider and without the broken rule: a warning only.
        let access = RepositoryAccess {
            pull: list(&["namespace:ci"]),
            rules: list(&["flaky"]),
            ..Default::default()
        };
        let checked = repository_access(&access, Some("kube"), &cel).unwrap();
        assert_eq!(checked.warnings.len(), 1);
        assert!(checked.rules[0].contains("provider_name == \"kube\""));
    }

    #[test]
    fn the_instance_s_trusted_verbs_name_who_they_trust() {
        let cel = FakeCel::default();
        let broad = GlobalAccess {
            admin: list(&["authenticated"]),
            ..Default::default()
        };
        assert!(global_access(&broad, None, &cel).is_err());
        let named = GlobalAccess {
            catalog: list(&["authenticated"]),
            admin: list(&["group:ops"]),
            jobs: list(&["identity:ci"]),
            metrics: list(&["identity:prometheus"]),
            ..Default::default()
        };
        assert_eq!(global_access(&named, None, &cel).unwrap().rules.len(), 4);
    }
}
