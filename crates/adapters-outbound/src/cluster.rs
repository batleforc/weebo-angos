//! What an instance's pass reads of the cluster besides its own objects:
//! the Secrets its spec references (only those labelled
//! `angos.weebo.io/credential: "true"`, spec 2.8), the cluster's
//! ServiceAccount issuer, and fresh keys.

use application::ports::{ApiError, ClusterIssuer, CredentialError, Credentials, KeyGenerator};
use base64::Engine;
use k8s_openapi::api::core::v1::Secret;
use kube::api::Api;
use kube::{Client, ResourceExt};
use tokio::sync::OnceCell;

/// [`Credentials`] on the Kubernetes API.
pub struct KubeCredentials {
    client: Client,
}

impl KubeCredentials {
    pub fn new(client: Client) -> Self {
        Self { client }
    }
}

#[async_trait::async_trait]
impl Credentials for KubeCredentials {
    async fn read_all(
        &self,
        namespace: &str,
        name: &str,
    ) -> Result<std::collections::BTreeMap<String, String>, CredentialError> {
        let api: Api<Secret> = Api::namespaced(self.client.clone(), namespace);
        let secret = api
            .get_opt(name)
            .await
            .map_err(|e| CredentialError::Kubernetes(format!("secret {namespace}/{name}: {e}")))?
            .ok_or_else(|| {
                CredentialError::Missing(format!("secret {namespace}/{name} not found"))
            })?;
        if secret
            .labels()
            .get(api::CREDENTIAL_LABEL)
            .map(String::as_str)
            != Some("true")
        {
            return Err(CredentialError::NotAllowed(
                weebo_kit_application::pod_safety::secret_not_labelled(
                    namespace,
                    name,
                    api::CREDENTIAL_LABEL,
                ),
            ));
        }
        // A key that isn't UTF-8 is said so, not dropped (it would read as
        // missing).
        secret
            .data
            .unwrap_or_default()
            .into_iter()
            .map(|(k, v)| match String::from_utf8(v.0) {
                Ok(v) => Ok((k, v)),
                Err(_) => Err(CredentialError::Missing(format!(
                    "secret {namespace}/{name}: key {k:?} is not UTF-8"
                ))),
            })
            .collect()
    }

    async fn read(
        &self,
        namespace: &str,
        name: &str,
        key: &str,
    ) -> Result<String, CredentialError> {
        let api: Api<Secret> = Api::namespaced(self.client.clone(), namespace);
        let secret = api
            .get_opt(name)
            .await
            .map_err(|e| CredentialError::Kubernetes(format!("secret {namespace}/{name}: {e}")))?
            .ok_or_else(|| {
                CredentialError::Missing(format!("secret {namespace}/{name} not found"))
            })?;
        if secret
            .labels()
            .get(api::CREDENTIAL_LABEL)
            .map(String::as_str)
            != Some("true")
        {
            return Err(CredentialError::NotAllowed(
                weebo_kit_application::pod_safety::secret_not_labelled(
                    namespace,
                    name,
                    api::CREDENTIAL_LABEL,
                ),
            ));
        }
        let bytes = secret
            .data
            .as_ref()
            .and_then(|d| d.get(key))
            .ok_or_else(|| {
                CredentialError::Missing(format!("secret {namespace}/{name} has no key {key:?}"))
            })?;
        String::from_utf8(bytes.0.clone()).map_err(|_| {
            CredentialError::Missing(format!(
                "secret {namespace}/{name}: key {key:?} is not UTF-8"
            ))
        })
    }

    async fn consents(&self, namespace: &str, name: &str) -> Result<Option<bool>, CredentialError> {
        let api: Api<Secret> = Api::namespaced(self.client.clone(), namespace);
        let meta = api
            .get_metadata_opt(name)
            .await
            .map_err(|e| CredentialError::Kubernetes(format!("secret {namespace}/{name}: {e}")))?;
        Ok(meta.map(|m| m.labels().get(api::CREDENTIAL_LABEL).map(String::as_str) == Some("true")))
    }
}

/// The issuer of `/.well-known/openid-configuration`, read once.
pub struct KubeClusterIssuer {
    client: Client,
    issuer: OnceCell<String>,
}

impl KubeClusterIssuer {
    pub fn new(client: Client) -> Self {
        Self {
            client,
            issuer: OnceCell::new(),
        }
    }
}

#[async_trait::async_trait]
impl ClusterIssuer for KubeClusterIssuer {
    async fn issuer(&self) -> Result<String, ApiError> {
        self.issuer
            .get_or_try_init(|| async {
                let request = http::Request::get("/.well-known/openid-configuration")
                    .body(Vec::new())
                    .map_err(|e| ApiError::Failed(e.to_string()))?;
                let discovery: serde_json::Value = self
                    .client
                    .request(request)
                    .await
                    .map_err(crate::api_error("the API server's OIDC discovery"))?;
                discovery["issuer"]
                    .as_str()
                    .map(String::from)
                    .ok_or_else(|| {
                        ApiError::Failed("the API server's OIDC discovery has no issuer".into())
                    })
            })
            .await
            .cloned()
    }
}

/// Keys from the OS's random source.
pub struct RandomKeys;

impl KeyGenerator for RandomKeys {
    fn token_key(&self) -> String {
        let mut key = [0u8; 32];
        getrandom::fill(&mut key).expect("the OS provides randomness");
        base64::engine::general_purpose::STANDARD.encode(key)
    }

    fn password(&self) -> String {
        let mut bytes = [0u8; 32];
        getrandom::fill(&mut bytes).expect("the OS provides randomness");
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
    }
}

/// Argon2id v0x13, m=19456 KiB, t=2, p=1, a 16-byte random salt: what
/// `angos argon` writes (`src/command/argon/mod.rs`).
pub struct Argon2Hasher;

impl application::ports::PasswordHasher for Argon2Hasher {
    fn hash(&self, password: &str) -> String {
        use argon2::password_hash::{PasswordHasher as _, SaltString, rand_core::OsRng};
        let params = argon2::Params::new(19_456, 2, 1, None).expect("Angos's parameters are valid");
        let argon =
            argon2::Argon2::new(argon2::Algorithm::Argon2id, argon2::Version::V0x13, params);
        argon
            .hash_password(password.as_bytes(), &SaltString::generate(&mut OsRng))
            .expect("hashing a password")
            .to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hashes_verify_as_angos_does() {
        use application::ports::PasswordHasher as _;
        use argon2::password_hash::{PasswordHash, PasswordVerifier};
        let hash = Argon2Hasher.hash("s3cr3t");
        assert!(
            hash.starts_with("$argon2id$v=19$m=19456,t=2,p=1$"),
            "{hash}"
        );
        // Angos checks with `Argon2::default()`, the hash's own params.
        let parsed = PasswordHash::new(&hash).unwrap();
        assert!(
            argon2::Argon2::default()
                .verify_password(b"s3cr3t", &parsed)
                .is_ok()
        );
        assert!(
            argon2::Argon2::default()
                .verify_password(b"other", &parsed)
                .is_err()
        );
        assert_eq!(RandomKeys.password().len(), 43);
    }

    #[test]
    fn keys_are_32_random_bytes() {
        let decode = |k: String| base64::engine::general_purpose::STANDARD.decode(k).unwrap();
        let (a, b) = (
            decode(RandomKeys.token_key()),
            decode(RandomKeys.token_key()),
        );
        assert_eq!(a.len(), 32);
        assert_ne!(a, b);
    }
}
