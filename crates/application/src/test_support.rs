//! In-memory adapters for use-case and controller tests (exposed to other
//! crates by the `test-support` feature): the kit's owned objects and pod
//! logs, and the operator's own ports.

use std::collections::BTreeMap;
use std::sync::Mutex;

pub use weebo_kit_application::test_support::{FakeOwnedObjects, FakePodLogs};

use crate::ports::{ClusterIssuer, CredentialError, Credentials, KeyGenerator, StatusLedger};

/// A Secret: whether it carries the credential label, and its keys.
type Labelled = (bool, BTreeMap<String, String>);

/// Secrets by `namespace/name`.
#[derive(Default)]
pub struct FakeCredentials {
    secrets: Mutex<BTreeMap<String, Labelled>>,
}

impl FakeCredentials {
    /// A labelled Secret.
    pub fn insert(&self, namespace: &str, name: &str, data: &[(&str, &str)]) {
        self.put(namespace, name, true, data);
    }

    /// A Secret without the credential label.
    pub fn insert_unlabelled(&self, namespace: &str, name: &str, data: &[(&str, &str)]) {
        self.put(namespace, name, false, data);
    }

    fn put(&self, namespace: &str, name: &str, labelled: bool, data: &[(&str, &str)]) {
        self.secrets.lock().unwrap().insert(
            format!("{namespace}/{name}"),
            (
                labelled,
                data.iter()
                    .map(|(k, v)| (k.to_string(), v.to_string()))
                    .collect(),
            ),
        );
    }
}

#[async_trait::async_trait]
impl Credentials for FakeCredentials {
    async fn read(
        &self,
        namespace: &str,
        name: &str,
        key: &str,
    ) -> Result<String, CredentialError> {
        let secrets = self.secrets.lock().unwrap();
        let Some((labelled, data)) = secrets.get(&format!("{namespace}/{name}")) else {
            return Err(CredentialError::Missing(format!(
                "secret {namespace}/{name} not found"
            )));
        };
        if !labelled {
            return Err(CredentialError::NotAllowed(format!(
                "secret {namespace}/{name} lacks the label {}",
                api::CREDENTIAL_LABEL
            )));
        }
        data.get(key).cloned().ok_or_else(|| {
            CredentialError::Missing(format!("secret {namespace}/{name} has no key {key}"))
        })
    }

    async fn consents(&self, namespace: &str, name: &str) -> Result<Option<bool>, CredentialError> {
        Ok(self
            .secrets
            .lock()
            .unwrap()
            .get(&format!("{namespace}/{name}"))
            .map(|(labelled, _)| *labelled))
    }
}

/// A fixed issuer, or none (`Err`).
pub struct FakeIssuer(pub Option<String>);

#[async_trait::async_trait]
impl ClusterIssuer for FakeIssuer {
    async fn issuer(&self) -> Result<String, String> {
        self.0.clone().ok_or_else(|| "forbidden".to_string())
    }
}

/// Keys counted up: `key-1`, `key-2`… padded to a valid token key.
#[derive(Default)]
pub struct CountingKeys(Mutex<u32>);

impl KeyGenerator for CountingKeys {
    fn token_key(&self) -> String {
        let mut n = self.0.lock().unwrap();
        *n += 1;
        // 33 bytes once decoded: a valid key, distinct per call.
        format!("{:0>44}", format!("key{n}"))
    }
}

/// What was recorded, in order.
#[derive(Default)]
pub struct FakeLedger(pub Mutex<Vec<serde_json::Map<String, serde_json::Value>>>);

#[async_trait::async_trait]
impl StatusLedger for FakeLedger {
    async fn record(
        &self,
        _namespace: &str,
        _name: &str,
        fields: serde_json::Map<String, serde_json::Value>,
    ) -> Result<(), String> {
        self.0.lock().unwrap().push(fields);
        Ok(())
    }
}

/// A CEL engine that accepts every rule but those it was told about.
#[derive(Default)]
pub struct FakeCel {
    refused: Mutex<Vec<String>>,
    may_fail: Mutex<Vec<String>>,
}

impl FakeCel {
    pub fn refuse(&self, rule: &str) {
        self.refused.lock().unwrap().push(rule.into());
    }

    pub fn may_fail(&self, rule: &str) {
        self.may_fail.lock().unwrap().push(rule.into());
    }
}

impl crate::ports::CelChecker for FakeCel {
    fn check(&self, _: crate::ports::RuleContext, rule: &str) -> crate::ports::RuleVerdict {
        if self.refused.lock().unwrap().iter().any(|r| r == rule) {
            crate::ports::RuleVerdict::Refused("refused by the fake".into())
        } else if self.may_fail.lock().unwrap().iter().any(|r| r == rule) {
            crate::ports::RuleVerdict::MayFail("fails on the fake".into())
        } else {
            crate::ports::RuleVerdict::Ok
        }
    }
}

/// Repositories bound to any instance, as set by the test.
#[derive(Default)]
pub struct FakeRepositories(pub Mutex<Vec<crate::repositories::RepositoryView>>);

#[async_trait::async_trait]
impl crate::ports::RepositoryDirectory for FakeRepositories {
    async fn bound_to(
        &self,
        _instance: &crate::ports::Owner,
        _selector: &api::local_instance::RepositorySelector,
    ) -> Result<Vec<crate::repositories::RepositoryView>, String> {
        Ok(self.0.lock().unwrap().clone())
    }
}
