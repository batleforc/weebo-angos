//! What an instance's pass reads of the cluster besides its own objects:
//! the Secrets its spec references (only those labelled
//! `angos.weebo.io/credential: "true"`, spec 2.8), the cluster's
//! ServiceAccount issuer, and fresh keys.

use application::ports::{ClusterIssuer, CredentialError, Credentials, KeyGenerator};
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
    async fn issuer(&self) -> Result<String, String> {
        self.issuer
            .get_or_try_init(|| async {
                let request = http::Request::get("/.well-known/openid-configuration")
                    .body(Vec::new())
                    .map_err(|e| e.to_string())?;
                let discovery: serde_json::Value = self
                    .client
                    .request(request)
                    .await
                    .map_err(|e| format!("the API server's OIDC discovery: {e}"))?;
                discovery["issuer"]
                    .as_str()
                    .map(String::from)
                    .ok_or_else(|| "the API server's OIDC discovery has no issuer".to_string())
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
}

#[cfg(test)]
mod tests {
    use super::*;

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
