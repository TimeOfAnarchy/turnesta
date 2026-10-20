use std::{
    future::Future,
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use hmac::{Hmac, KeyInit, Mac};
use sha2::Sha256;
use sqlx::PgPool;
use thiserror::Error;
use url::Url;
use zeroize::Zeroizing;

use crate::tips;

#[derive(Clone, Default)]
pub struct PaymentConfig {
    mock: Option<Arc<MockSettings>>,
}

struct MockSettings {
    key: Zeroizing<[u8; 32]>,
    callback_url: Url,
}

#[derive(Debug, Error)]
pub enum PaymentError {
    #[error("invalid mock payment configuration")]
    Configuration,
    #[error("invalid mock payment signature")]
    Signature,
    #[error("mock payment delivery database operation failed")]
    Database,
    #[error("could not prepare mock payment delivery")]
    Preparation,
}

impl PaymentConfig {
    pub fn from_lookup(lookup: &impl Fn(&str) -> Option<String>) -> Result<Self, PaymentError> {
        match lookup("TURNESTA_TIPS_MODE")
            .as_deref()
            .unwrap_or("disabled")
        {
            "disabled" => Ok(Self::default()),
            "mock" => {
                let key = Zeroizing::new(
                    lookup("TURNESTA_MOCK_PAYMENT_KEY").ok_or(PaymentError::Configuration)?,
                );
                let url = lookup("TURNESTA_MOCK_CALLBACK_URL")
                    .unwrap_or_else(|| "http://127.0.0.1:8080/hooks/tips/mock".into());
                Self::mock(&key, &url)
            }
            _ => Err(PaymentError::Configuration),
        }
    }

    pub fn mock(encoded_key: &str, callback_url: &str) -> Result<Self, PaymentError> {
        if encoded_key.len() != 64 {
            return Err(PaymentError::Configuration);
        }
        let mut key = Zeroizing::new([0; 32]);
        hex::decode_to_slice(encoded_key, &mut *key).map_err(|_| PaymentError::Configuration)?;
        let callback_url = Url::parse(callback_url).map_err(|_| PaymentError::Configuration)?;
        let host = callback_url.host_str().ok_or(PaymentError::Configuration)?;
        let local_http = callback_url.scheme() == "http"
            && matches!(host, "api" | "localhost" | "127.0.0.1" | "[::1]");
        if (!local_http && callback_url.scheme() != "https")
            || !callback_url.username().is_empty()
            || callback_url.password().is_some()
            || callback_url.query().is_some()
            || callback_url.fragment().is_some()
            || callback_url.path() != "/hooks/tips/mock"
        {
            return Err(PaymentError::Configuration);
        }
        Ok(Self {
            mock: Some(Arc::new(MockSettings { key, callback_url })),
        })
    }

    pub fn enabled(&self) -> bool {
        self.mock.is_some()
    }

    pub fn sign(&self, timestamp: u64, body: &[u8]) -> Result<String, PaymentError> {
        let mac = self.mac(timestamp, body)?;
        Ok(hex::encode(mac.finalize().into_bytes()))
    }

    pub fn verify(
        &self,
        timestamp: &str,
        signature: &str,
        body: &[u8],
    ) -> Result<(), PaymentError> {
        let parsed: u64 = timestamp.parse().map_err(|_| PaymentError::Signature)?;
        if parsed.to_string() != timestamp
            || unix_seconds()?.abs_diff(parsed) > 300
            || signature.len() != 64
        {
            return Err(PaymentError::Signature);
        }
        let mut bytes = [0_u8; 32];
        hex::decode_to_slice(signature, &mut bytes).map_err(|_| PaymentError::Signature)?;
        self.mac(parsed, body)?
            .verify_slice(&bytes)
            .map_err(|_| PaymentError::Signature)
    }

    fn mac(&self, timestamp: u64, body: &[u8]) -> Result<Hmac<Sha256>, PaymentError> {
        let settings = self.mock.as_ref().ok_or(PaymentError::Configuration)?;
        let mut mac = Hmac::<Sha256>::new_from_slice(settings.key.as_slice())
            .map_err(|_| PaymentError::Configuration)?;
        mac.update(b"turnesta.mock-payment.v1\n");
        mac.update(timestamp.to_string().as_bytes());
        mac.update(b"\n");
        mac.update(body);
        Ok(mac)
    }
}

pub struct MockDispatcher {
    pool: PgPool,
    config: PaymentConfig,
    client: reqwest::Client,
}

impl MockDispatcher {
    pub fn new(pool: PgPool, config: PaymentConfig) -> Result<Self, PaymentError> {
        // Keep one process-wide provider shared with SQLx and SMTP. An existing
        // provider is retained; rustls-no-provider requires explicit setup.
        let _ = rustls::crypto::ring::default_provider().install_default();
        let client = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(2))
            .timeout(Duration::from_secs(5))
            .redirect(reqwest::redirect::Policy::none())
            .no_proxy()
            .referer(false)
            .pool_max_idle_per_host(1)
            .build()
            .map_err(|_| PaymentError::Configuration)?;
        Ok(Self {
            pool,
            config,
            client,
        })
    }

    pub async fn run_once(&self) -> Result<bool, PaymentError> {
        let Some(settings) = &self.config.mock else {
            return Ok(false);
        };
        let Some(job) = database(tips::claim_mock_delivery(&self.pool)).await? else {
            return Ok(false);
        };
        let body = serde_json::to_vec(&job.event).map_err(|_| PaymentError::Preparation)?;
        let timestamp = unix_seconds()?;
        let signature = self.config.sign(timestamp, &body)?;
        // The HTTP result may be unknown after a timeout. Retrying this immutable
        // event is safe because the recipient stores an idempotent inbox.
        let acknowledged = async {
            let mut response = self
                .client
                .post(settings.callback_url.clone())
                .header("content-type", "application/json")
                .header("x-turnesta-mock-timestamp", timestamp.to_string())
                .header("x-turnesta-mock-signature", signature)
                .body(body)
                .send()
                .await
                .ok()?;
            if response.status() != reqwest::StatusCode::OK {
                return None;
            }
            let mut bytes = Vec::new();
            while let Some(chunk) = response.chunk().await.ok()? {
                if bytes.len() + chunk.len() > 1024 {
                    return None;
                }
                bytes.extend_from_slice(&chunk);
            }
            let result: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
            (result.get("accepted") == Some(&serde_json::Value::Bool(true))).then_some(())
        }
        .await
        .is_some();
        let confirmed = database(tips::finish_mock_delivery(
            &self.pool,
            job.job_id,
            job.lease_token,
            acknowledged,
        ))
        .await?;
        tracing::info!(job_id=%job.job_id, attempt=job.attempt, acknowledged, confirmed, "mock payment callback attempt finished");
        Ok(true)
    }
}

async fn database<T>(
    future: impl Future<Output = Result<T, tips::TipsError>>,
) -> Result<T, PaymentError> {
    tokio::time::timeout(Duration::from_secs(3), future)
        .await
        .map_err(|_| PaymentError::Database)?
        .map_err(|_| PaymentError::Database)
}

fn unix_seconds() -> Result<u64, PaymentError> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|value| value.as_secs())
        .map_err(|_| PaymentError::Preparation)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn dispatcher_builds_with_the_shared_ring_tls_provider() {
        let pool = sqlx::postgres::PgPoolOptions::new()
            .connect_lazy("postgres://localhost/unused")
            .unwrap();
        assert!(MockDispatcher::new(pool, PaymentConfig::default()).is_ok());
    }

    #[test]
    fn signatures_authenticate_exact_bytes_time_and_key() {
        let config =
            PaymentConfig::mock(&"13".repeat(32), "http://api:8080/hooks/tips/mock").unwrap();
        let other =
            PaymentConfig::mock(&"27".repeat(32), "http://api:8080/hooks/tips/mock").unwrap();
        let now = unix_seconds().unwrap();
        let body = br#"{"amount_minor":12500}"#;
        let signature = config.sign(now, body).unwrap();
        assert!(config.verify(&now.to_string(), &signature, body).is_ok());
        assert!(
            config
                .verify(&now.to_string(), &signature, br#"{"amount_minor":12501}"#)
                .is_err()
        );
        assert!(other.verify(&now.to_string(), &signature, body).is_err());
        assert!(
            config
                .verify(
                    &(now - 301).to_string(),
                    &config.sign(now - 301, body).unwrap(),
                    body
                )
                .is_err()
        );
        assert!(config.verify(&format!("0{now}"), &signature, body).is_err());
    }

    #[test]
    fn mock_is_opt_in_and_bad_configuration_never_echoes_secrets() {
        assert!(!PaymentConfig::from_lookup(&|_| None).unwrap().enabled());
        assert!(
            PaymentConfig::from_lookup(
                &|name| (name == "TURNESTA_TIPS_MODE").then(|| "production".into())
            )
            .is_err()
        );
        let error = PaymentConfig::mock(
            "private credential",
            "http://remote.example/hooks/tips/mock",
        )
        .err()
        .unwrap();
        assert!(!error.to_string().contains("private credential"));
        for url in [
            "http://remote.example/hooks/tips/mock",
            "https://name:password@example.test/hooks/tips/mock",
            "https://example.test/wrong",
            "http://api:8080/hooks/tips/mock?secret=1",
        ] {
            assert!(PaymentConfig::mock(&"13".repeat(32), url).is_err());
        }
    }
}
