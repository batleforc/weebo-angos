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
