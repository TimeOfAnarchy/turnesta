use anyhow::{Context, Result};
use tracing_subscriber::EnvFilter;

pub fn init() -> Result<()> {
    let filter = std::env::var("RUST_LOG").unwrap_or_else(|_| "info,sqlx=warn".into());
    let filter = EnvFilter::try_new(filter).context("RUST_LOG contains an invalid log filter")?;

    tracing_subscriber::fmt()
        .json()
        .with_env_filter(filter)
        .with_ansi(false)
        .with_current_span(true)
        .with_span_list(false)
        .try_init()
        .map_err(|error| anyhow::anyhow!("could not initialize logging: {error}"))
}
