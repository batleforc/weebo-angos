//! The importer on weebo-si's real values (`tests/fixtures`, the chart at
//! weebo-si `develop` b2fda552): what it prints is what the operator admits,
//! its rules judged by Angos's engine.

#![expect(clippy::unwrap_used, reason = "test helpers fail the test on purpose")]

use adapters_outbound::CelInterpreterChecker;
use api::local_instance::LocalInstance;
use api::repository::Repository;
use importer::{Import, Options, import, to_yaml};
use serde_json::Value;

fn values() -> Value {
    serde_norway::from_str(include_str!("fixtures/weebo-si-values.yaml")).unwrap()
}

fn imported(values: &Value) -> Import {
    import(values, &Options::default()).unwrap()
}

fn kind<'a>(import: &'a Import, kind: &str) -> Vec<&'a Value> {
    import
        .objects
        .iter()
        .filter(|o| o["kind"] == kind)
        .collect()
}

#[test]
fn weebo_si_imports_as_the_same_store_and_repositories() {
    let import = imported(&values());
    let kinds: Vec<&str> = import
        .objects
        .iter()
        .map(|o| o["kind"].as_str().unwrap())
        .collect();
    assert_eq!(
        kinds,
        [
            "ExternalSecret",
            "AngosLocalInstance",
            "AngosRepository",
            "AngosRepository",
            "AngosRepository",
            "AngosRepository",
            "AngosMaintenance",
            "AngosMaintenance",
        ]
    );
    let instance: LocalInstance = serde_json::from_value(import.objects[1].clone()).unwrap();
    let s3 = instance.spec.storage.s3.as_ref().unwrap();
    assert_eq!(
        (
            s3.endpoint.as_str(),
            s3.bucket.as_str(),
            s3.key_prefix.as_deref()
        ),
        ("https://s3-main.weebo.poc", "angos", None)
    );
    assert_eq!(instance.spec.image.tag.as_deref(), Some("1.12.2"));
    let ts = instance.spec.auth.token_service.as_ref().unwrap();
    assert_eq!(ts.realm, "https://registry.pkg.weebo.poc/token");
    assert_eq!(ts.key_secret_ref.as_ref().unwrap().key, "token-key");
    assert_eq!(instance.spec.global.access.admin, ["group:weebo_admin"]);
    assert_eq!(instance.spec.service_account.name.as_deref(), Some("angos"));
    assert!(!instance.spec.service_account.create);
    // Prefixes byte for byte, access 1:1.
    let repositories: Vec<Repository> = kind(&import, "AngosRepository")
        .into_iter()
        .map(|o| serde_json::from_value(o.clone()).unwrap())
        .collect();
    let prefixes: Vec<&str> = repositories
        .iter()
        .map(|r| r.spec.prefix.as_str())
        .collect();
    assert_eq!(prefixes, ["ghcr", "hub", "quay", "weebo"]);
    let weebo = &repositories[3].spec;
    assert!(weebo.upstreams.is_empty());
    assert_eq!(weebo.access.push, ["provider:weebo"]);
    assert_eq!(weebo.access.delete, ["group:weebo_admin"]);
    assert_eq!(
        repositories[1].spec.upstreams[0].url,
        "https://registry-1.docker.io"
    );
    // The maintenances wait, the prune as a dry run.
    for m in kind(&import, "AngosMaintenance") {
        assert_eq!(m["spec"]["suspend"], true);
        if m["spec"]["task"] == "Prune" {
            assert_eq!(m["spec"]["params"]["dryRun"], true);
            assert_eq!(m["spec"]["schedule"], "30 3 * * *");
        }
    }
    // The ExternalSecret reads the chart's Vault paths, labelled.
    let es = &import.objects[0];
    assert_eq!(
        es["spec"]["target"]["template"]["metadata"]["labels"][api::CREDENTIAL_LABEL],
        "true"
    );
    assert!(es.to_string().contains("mv/angos/s3"));
    assert!(import.warnings.iter().any(|w| w.contains("us-east-")));
}

#[test]
fn what_is_imported_is_what_the_operator_admits() {
    let cel = CelInterpreterChecker;
    let mut values = values();
    // The scanner on, scanning the local registry.
    values["angos"]["scanner"]["enabled"] = true.into();
    values["angos"]["repositories"]["weebo"]["scan"] = true.into();
    let import = imported(&values);
    assert_eq!(kind(&import, "AngosIdentity").len(), 1);
    let instance: LocalInstance =
        serde_json::from_value(kind(&import, "AngosLocalInstance")[0].clone()).unwrap();
    assert!(instance.spec.scan.is_some());
    application::admission::check_instance(
        "registry",
        "angos",
        &instance.spec,
        &[api::angos::IMAGE_REPOSITORY.to_string()],
        "ghcr.io/project-angos/angos:1.12.3",
        &cel,
    )
    .unwrap();
    for object in kind(&import, "AngosRepository") {
        let repository: Repository = serde_json::from_value(object.clone()).unwrap();
        let (_, warnings) = application::repositories::validate(
            &repository.spec,
            &Default::default(),
            Some("kube"),
            &cel,
        )
        .unwrap();
        assert!(
            warnings.is_empty(),
            "{}: {warnings:?}",
            repository.spec.prefix
        );
    }
    let yaml = to_yaml(&import).unwrap();
    assert!(yaml.starts_with("# Before applying:"));
    assert_eq!(yaml.matches("\n---\n").count(), import.objects.len());
}

#[test]
fn values_of_another_chart_are_refused() {
    assert!(import(&serde_json::json!({"replicas": 1}), &Options::default()).is_err());
    let mut off = values();
    off["angos"]["enabled"] = false.into();
    assert!(import(&off, &Options::default()).is_err());
    assert_eq!(importer::cr_name("Team/App_1"), "team-app-1");
}
