use std::{env, net::SocketAddr, path::PathBuf, time::Duration};

use crate::auth::AuthConfig;
use sqlx::{ConnectOptions, postgres::PgConnectOptions};
use thiserror::Error;

pub struct Config {
    pub bind_addr: SocketAddr,
    pub database: DatabaseConfig,
    pub ready_timeout: Duration,
    pub shutdown_timeout: Duration,
    pub auth: AuthConfig,
    pub web_dist: PathBuf,
    pub payments: crate::payments::PaymentConfig,
}

// No Debug implementation: connection options can contain credentials.
pub struct DatabaseConfig {
    pub options: PgConnectOptions,
    pub max_connections: u32,
    pub acquire_timeout: Duration,
}

#[derive(Debug, Error)]
#[error("invalid configuration: {name} {reason}")]
pub struct ConfigError {
    name: &'static str,
    reason: &'static str,
}

impl Config {
    pub fn from_env() -> Result<Self, ConfigError> {
        Self::from_lookup(|name| env::var(name).ok())
    }

    pub fn from_lookup(lookup: impl Fn(&str) -> Option<String>) -> Result<Self, ConfigError> {
        let bind_addr = lookup("TURNESTA_BIND_ADDR")
            .unwrap_or_else(|| "127.0.0.1:8080".into())
            .parse()
            .map_err(|_| invalid("TURNESTA_BIND_ADDR", "must be an IP address and port"))?;

        let database_url = lookup("DATABASE_URL")
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| invalid("DATABASE_URL", "is required"))?;
        if !database_url.starts_with("postgres://") && !database_url.starts_with("postgresql://") {
            return Err(invalid("DATABASE_URL", "must be a PostgreSQL URL"));
        }
        let options: PgConnectOptions = database_url
            .parse()
            .map_err(|_| invalid("DATABASE_URL", "must be a valid PostgreSQL URL"))?;

        Ok(Self {
            payments: crate::payments::PaymentConfig::from_lookup(&lookup).map_err(|_| {
                invalid(
                    "TURNESTA_TIPS_MODE / TURNESTA_MOCK_PAYMENT_KEY / TURNESTA_MOCK_CALLBACK_URL",
                    "must define disabled or valid mock configuration",
                )
            })?,
            bind_addr,
            web_dist: lookup("TURNESTA_WEB_DIST")
                .unwrap_or_else(|| "apps/web/dist".into())
                .into(),
            database: DatabaseConfig {
                options: options
                    .application_name("turnesta-api")
                    .disable_statement_logging(),
                max_connections: positive(&lookup, "TURNESTA_DB_MAX_CONNECTIONS", 10)?,
                acquire_timeout: millis(&lookup, "TURNESTA_DB_ACQUIRE_TIMEOUT_MS", 3_000)?,
            },
            ready_timeout: millis(&lookup, "TURNESTA_READY_TIMEOUT_MS", 1_000)?,
            shutdown_timeout: millis(&lookup, "TURNESTA_SHUTDOWN_TIMEOUT_MS", 10_000)?,
            auth: AuthConfig::parse(
                &lookup("TURNESTA_PUBLIC_ORIGIN").unwrap_or_else(|| "http://127.0.0.1:8080".into()),
                i32::try_from(positive(&lookup, "TURNESTA_SESSION_TTL_SECONDS", 28_800)?)
                    .map_err(|_| invalid("TURNESTA_SESSION_TTL_SECONDS", "must be 60..86400"))?,
            )
            .map_err(|reason| {
                invalid(
                    "TURNESTA_PUBLIC_ORIGIN / TURNESTA_SESSION_TTL_SECONDS",
                    reason,
                )
            })?,
        })
    }
}

fn positive(
    lookup: &impl Fn(&str) -> Option<String>,
    name: &'static str,
    default: u32,
) -> Result<u32, ConfigError> {
    match lookup(name) {
        None => Ok(default),
        Some(raw) => raw
            .parse::<u32>()
            .ok()
            .filter(|value| *value > 0)
            .ok_or_else(|| invalid(name, "must be a positive integer")),
    }
}

fn millis(
    lookup: &impl Fn(&str) -> Option<String>,
    name: &'static str,
    default: u32,
) -> Result<Duration, ConfigError> {
    positive(lookup, name, default).map(|value| Duration::from_millis(u64::from(value)))
}

fn invalid(name: &'static str, reason: &'static str) -> ConfigError {
    ConfigError { name, reason }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(values: &[(&str, &str)]) -> Result<Config, ConfigError> {
        Config::from_lookup(|name| {
            values
                .iter()
                .find(|(key, _)| *key == name)
                .map(|(_, value)| (*value).to_owned())
        })
    }

    #[test]
    fn missing_database_url_is_rejected() {
        assert!(matches!(config(&[]), Err(error) if error.name == "DATABASE_URL"));
    }

    #[test]
    fn defaults_bind_only_to_loopback_and_limit_the_pool() {
        let settings = config(&[(
            "DATABASE_URL",
            "postgres://user:password@localhost/turnesta",
        )])
        .expect("valid configuration");
        assert!(settings.bind_addr.ip().is_loopback());
        assert_eq!(settings.database.max_connections, 10);
        assert_eq!(settings.ready_timeout, Duration::from_secs(1));
    }

    #[test]
    fn zero_pool_or_timeout_is_rejected() {
        for name in [
            "TURNESTA_DB_MAX_CONNECTIONS",
            "TURNESTA_DB_ACQUIRE_TIMEOUT_MS",
            "TURNESTA_READY_TIMEOUT_MS",
            "TURNESTA_SHUTDOWN_TIMEOUT_MS",
        ] {
            let result = config(&[
                ("DATABASE_URL", "postgres://localhost/turnesta"),
                (name, "0"),
            ]);
            assert!(matches!(result, Err(error) if error.name == name));
        }
    }

    #[test]
    fn invalid_values_do_not_echo_credentials() {
        let error = match config(&[("DATABASE_URL", "https://user:private-password@localhost")]) {
            Err(error) => error,
            Ok(_) => panic!("invalid database URL was accepted"),
        };
        assert!(!error.to_string().contains("private-password"));
    }

    #[test]
    fn malformed_bind_address_is_rejected() {
        let result = config(&[
            ("DATABASE_URL", "postgres://localhost/turnesta"),
            ("TURNESTA_BIND_ADDR", "localhost:8080"),
        ]);
        assert!(matches!(result, Err(error) if error.name == "TURNESTA_BIND_ADDR"));
    }
}
