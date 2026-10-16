use k8s_openapi::apimachinery::pkg::apis::meta::v1::LabelSelector;
use kube::CustomResource;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

pub use weebo_kit_api::{AttachNamespaces, NamespacesFrom, SharedWith};

use crate::status::Status;

/// The short names the rest of the workspace uses.
pub use self::AngosLocalInstance as LocalInstance;
pub use self::AngosLocalInstanceSpec as LocalInstanceSpec;

/// An Angos registry the operator runs in this namespace: its
/// `Deployment`, its configuration (rendered from this spec, its
/// repositories and identities) and the objects around them. `Ready` tells
/// whether its pods run the desired configuration.
#[derive(CustomResource, Serialize, Deserialize, Clone, Debug, Default, PartialEq, JsonSchema)]
#[kube(
    group = "angos.weebo.io",
    version = "v1alpha1",
    kind = "AngosLocalInstance",
    plural = "angoslocalinstances",
    shortname = "ali",
    namespaced,
    status = "Status",
    printcolumn = r#"{"name":"Endpoint","type":"string","jsonPath":".status.endpoint"}"#,
    printcolumn = r#"{"name":"Version","type":"string","jsonPath":".status.version"}"#,
    printcolumn = r#"{"name":"Ready","type":"string","jsonPath":".status.conditions[?(@.type=='Ready')].status"}"#,
    printcolumn = r#"{"name":"Reason","type":"string","jsonPath":".status.conditions[?(@.type=='Ready')].reason"}"#,
    printcolumn = r#"{"name":"Age","type":"date","jsonPath":".metadata.creationTimestamp"}"#,
    // The instance's name is its Service's (a DNS-1035 label) and the
    // `app.kubernetes.io/instance` label of every object it owns.
    validation = Rule::new(
        "size(self.metadata.name) <= 63 && self.metadata.name.matches('^[a-z]([-a-z0-9]*[a-z0-9])?$')"
    ).message("the instance's name names its Service: a DNS-1035 label (lowercase letters, digits and '-', starting with a letter), at most 63 characters")
)]
#[serde(rename_all = "camelCase")]
pub struct AngosLocalInstanceSpec {
    /// Which `AngosRepository`s the instance serves, and from which
    /// namespaces.
    #[serde(default, skip_serializing_if = "RepositorySelector::is_default")]
    pub repository_selector: RepositorySelector,
}

/// Which `AngosRepository`s (and `AngosIdentity`s) the instance serves.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct RepositorySelector {
    /// Only the objects whose labels match; all of them when unset.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selector: Option<LabelSelector>,
    /// The namespaces whose objects may attach to this instance (with an
    /// `instanceRef` naming it): its own always.
    #[serde(default, skip_serializing_if = "AttachNamespaces::is_default")]
    pub namespaces: AttachNamespaces,
}

impl RepositorySelector {
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }
}

/// What the instance shares with the repositories and identities of other
/// namespaces (its `repositorySelector.namespaces`).
pub fn object_sharing(instance: &LocalInstance) -> Option<SharedWith> {
    instance.spec.repository_selector.namespaces.shared_with()
}

#[cfg(test)]
mod tests {
    use kube::CustomResourceExt;

    use super::*;

    #[test]
    fn the_name_and_the_short_name_are_checked() {
        let crd = serde_json::to_value(LocalInstance::crd()).unwrap();
        assert_eq!(crd["spec"]["names"]["shortNames"][0], "ali");
        let rules =
            &crd["spec"]["versions"][0]["schema"]["openAPIV3Schema"]["x-kubernetes-validations"];
        assert!(rules.to_string().contains("DNS-1035"), "{rules}");
    }

    #[test]
    fn an_instance_shares_only_through_its_selector() {
        let mut instance = LocalInstance::new("reg", LocalInstanceSpec::default());
        assert_eq!(object_sharing(&instance), None);
        instance.spec.repository_selector.namespaces.from = NamespacesFrom::All;
        assert!(object_sharing(&instance).is_some());
    }
}
