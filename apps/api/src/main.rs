use std::{future::IntoFuture, process::ExitCode, time::Duration};

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use sqlx::PgPool;
use tokio::{net::TcpListener, sync::watch};
use turnesta_api::{
    auth::{
        cli::{self as auth_cli, AuthCommand},
        delivery::{self, Worker},
    },
    config::Config,
    core::cli::{self as core_cli, CoreCommand},
    database,
    http::{self, AppState},
    operations::{self, OperationsCommand},
    payments::MockDispatcher,
    queue_live::LiveHub,
    telemetry,
};

#[derive(Parser)]
#[command(version, about = "Turnesta API server and database migrations")]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Start the HTTP server (the default command).
    Serve,
    /// Apply embedded SQL migrations, then exit.
    Migrate,
    /// Trusted operator commands for Core data (not an HTTP authentication flow).
    Core {
        #[command(subcommand)]
        command: CoreCommand,
    },
    /// Issue an email login invitation (trusted administrator only).
    Auth {
        #[command(subcommand)]
        command: AuthCommand,
    },
    /// Trusted read-only diagnostics (migration database login required).
    Ops {
        #[command(subcommand)]
        command: OperationsCommand,
    },
    /// Deliver queued invitation emails with bounded retries.
    Worker {
        /// Process at most one job and exit (for operational checks).
        #[arg(long)]
        once: bool,
    },
}

#[tokio::main]
async fn main() -> ExitCode {
    let cli = Cli::parse();
    match run(cli).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            tracing::error!(error = %error, "application failed");
            eprintln!("Turnesta failed: {error:#}");
            ExitCode::FAILURE
        }
    }
}

async fn run(cli: Cli) -> Result<()> {
    telemetry::init()?;
    let config = Config::from_env()?;
    let pool = database::connect(&config.database)
        .await
        .context("could not connect to PostgreSQL")?;

    match cli.command.unwrap_or(Command::Serve) {
        Command::Migrate => {
            tracing::info!("applying database migrations");
            let result = database::migrate(&pool)
                .await
                .context("database migrations failed");
            tokio::time::timeout(config.shutdown_timeout, pool.close())
                .await
                .context("database pool shutdown deadline exceeded")?;
            result
        }
        Command::Serve => serve(&config, pool).await,
        Command::Core { command } => {
            let result = core_cli::run(command, &pool).await;
            tokio::time::timeout(config.shutdown_timeout, pool.close())
                .await
                .context("database pool shutdown deadline exceeded")?;
            result
        }
        Command::Auth { command } => {
            let result = auth_cli::run(command, &pool, &config.auth.public_origin).await;
            tokio::time::timeout(config.shutdown_timeout, pool.close())
                .await
                .context("database pool shutdown deadline exceeded")?;
            result
        }
        Command::Ops { command } => {
            let result = operations::run(command, &pool).await;
            tokio::time::timeout(config.shutdown_timeout, pool.close())
                .await
                .context("database pool shutdown deadline exceeded")?;
            result
        }
        Command::Worker { once } => run_worker(&config, pool, once).await,
    }
}

async fn serve(config: &Config, pool: PgPool) -> Result<()> {
    anyhow::ensure!(
        database::is_runtime_role(&pool).await?,
        "serve requires the restricted runtime database role"
    );
    let queue_live = LiveHub::new(pool.clone());
    let state = AppState::new(pool.clone(), config.ready_timeout)
        .with_queue_live(queue_live.clone())
        .with_auth(config.auth.clone())
        .with_payments(config.payments.clone())
        .with_web_dist(config.web_dist.clone());
    let app = http::router(state.clone());
    let listener = TcpListener::bind(config.bind_addr)
        .await
        .context("could not bind HTTP listener")?;
    let address = listener.local_addr()?;
    let queue_live_task = queue_live.start();
    tracing::info!(%address, version = env!("CARGO_PKG_VERSION"), "HTTP server listening");

    let (shutdown_tx, mut shutdown_rx) = watch::channel(false);
    let server = axum::serve(listener, app)
        .with_graceful_shutdown(async move {
            let _ = shutdown_rx.changed().await;
        })
        .into_future();
    tokio::pin!(server);

    let result = tokio::select! {
        result = &mut server => result.context("HTTP server failed"),
        signal = shutdown_signal() => {
            signal?;
            state.begin_shutdown();
            let _ = shutdown_tx.send(true);
            tracing::info!("draining HTTP requests");
            return tokio::time::timeout(config.shutdown_timeout, async {
                (&mut server).await.context("HTTP server failed during shutdown")?;
                queue_live_task.await.context("live updates task failed during shutdown")?;
                pool.close().await;
                Ok::<(), anyhow::Error>(())
            })
                .await
                .context("application shutdown deadline exceeded")?;
        }
    };

    queue_live.shutdown();
    tokio::time::timeout(config.shutdown_timeout, async {
        queue_live_task.await.context("live updates task failed")?;
        pool.close().await;
        Ok::<_, anyhow::Error>(())
    })
    .await
    .context("database pool shutdown deadline exceeded")??;
    result
}

async fn shutdown_signal() -> Result<()> {
    #[cfg(unix)]
    {
        let mut terminate =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
                .context("could not register SIGTERM handler")?;
        tokio::select! {
            result = tokio::signal::ctrl_c() => result.context("could not listen for Ctrl+C")?,
            _ = terminate.recv() => {},
        }
    }
    #[cfg(not(unix))]
    tokio::signal::ctrl_c()
        .await
        .context("could not listen for Ctrl+C")?;

    Ok(())
}

async fn run_worker(config: &Config, pool: PgPool, once: bool) -> Result<()> {
    anyhow::ensure!(
        delivery::is_worker_role(&pool).await?,
        "worker requires the dedicated delivery database role"
    );
    let worker = Worker::from_env(pool.clone())?;
    let payments = MockDispatcher::new(pool.clone(), config.payments.clone())?;
    if once {
        let has_work = worker_step(&worker, &payments).await?;
        tracing::info!(has_work, "worker step completed");
        tokio::time::timeout(config.shutdown_timeout, pool.close())
            .await
            .context("database pool shutdown deadline exceeded")?;
        return Ok(());
    }
    let (stop_tx, mut stop_rx) = watch::channel(false);
    let mut task = tokio::spawn(async move {
        loop {
            if *stop_rx.borrow() {
                break;
            }
            // Both bounded attempts run concurrently, keeping the combined drain
            // within the existing deadline. Neither is canceled on stop.
            let delay = match worker_step(&worker, &payments).await {
                Ok(true) => Duration::from_millis(10),
                Ok(_) => Duration::from_secs(1),
                Err(error) => {
                    tracing::warn!(error = %error, "delivery worker step failed");
                    Duration::from_secs(5)
                }
            };
            tokio::select! {
                _ = stop_rx.changed() => break,
                _ = tokio::time::sleep(delay) => {},
            }
        }
    });
    tokio::select! {
        signal = shutdown_signal() => {
            signal?;
            let _ = stop_tx.send(true);
            tracing::info!("draining delivery worker");
            tokio::time::timeout(Duration::from_secs(30), async {
                (&mut task).await.context("delivery worker task failed")?;
                pool.close().await;
                Ok::<(), anyhow::Error>(())
            }).await.context("delivery worker shutdown deadline exceeded")??;
            Ok(())
        },
        result = &mut task => {
            result.context("delivery worker task failed")?;
            tokio::time::timeout(config.shutdown_timeout, pool.close()).await
                .context("database pool shutdown deadline exceeded")?;
            anyhow::bail!("delivery worker exited unexpectedly")
        }
    }
}

async fn worker_step(mail: &Worker, payments: &MockDispatcher) -> Result<bool> {
    let (mail, payments) = tokio::join!(mail.run_once(), payments.run_once());
    let results = [
        (
            "invitations",
            mail.map(|value| value.has_work())
                .map_err(anyhow::Error::from),
        ),
        ("mock_payments", payments.map_err(anyhow::Error::from)),
    ];
    let mut has_work = false;
    let mut failed = false;
    for (queue, result) in results {
        match result {
            Ok(value) => has_work |= value,
            Err(error) => {
                failed = true;
                tracing::warn!(queue, error=%error, "worker queue step failed");
            }
        }
    }
    anyhow::ensure!(!failed, "a delivery queue step failed");
    Ok(has_work)
}
