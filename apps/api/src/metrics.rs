use std::{sync::Arc, time::Duration};

use axum::{
    Router,
    extract::State,
    http::{StatusCode, header::CONTENT_TYPE},
    response::{IntoResponse, Response},
    routing::get,
};
use prometheus::{
    Encoder, Gauge, HistogramOpts, HistogramVec, IntCounterVec, IntGauge, IntGaugeVec, Opts,
    TextEncoder, core::Collector,
};
use sqlx::PgPool;

pub(crate) struct ApiMetrics {
    responses: IntCounterVec,
    duration: HistogramVec,
    rate_limited: IntCounterVec,
    pub(crate) sse: IntGauge,
}

impl ApiMetrics {
    pub(crate) fn new() -> Self {
        Self {
            responses: IntCounterVec::new(
                Opts::new(
                    "turnesta_http_responses_total",
                    "HTTP responses by matched route and status.",
                ),
                &["route", "status"],
            )
            .expect("static response metric"),
            duration: HistogramVec::new(
                HistogramOpts::new(
                    "turnesta_http_request_duration_seconds",
                    "Time to response headers by matched route.",
                ),
                &["route"],
            )
            .expect("static duration metric"),
            rate_limited: IntCounterVec::new(
                Opts::new(
                    "turnesta_http_rate_limited_total",
                    "HTTP 429 responses by limiting reason.",
                ),
                &["reason"],
            )
            .expect("static rate limit metric"),
            sse: IntGauge::new("turnesta_sse_connections", "Open queue event streams.")
                .expect("static SSE metric"),
        }
    }

    pub(crate) fn record(&self, route: &str, response: &Response, elapsed: Duration) {
        self.responses
            .with_label_values(&[route, response.status().as_str()])
            .inc();
        self.duration
            .with_label_values(&[route])
            .observe(elapsed.as_secs_f64());
        if response.status() == StatusCode::TOO_MANY_REQUESTS {
            let reason = response
                .extensions()
                .get::<LimitReason>()
                .map_or("other", |r| r.0);
            self.rate_limited.with_label_values(&[reason]).inc();
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct LimitReason(pub(crate) &'static str);

#[derive(Clone)]
enum Target {
    Api(Arc<ApiMetrics>),
    Worker,
}

pub(crate) fn api_router(pool: PgPool, api: Arc<ApiMetrics>) -> Router {
    Router::new()
        .route("/metrics", get(scrape))
        .with_state((pool, Target::Api(api)))
}

pub fn worker_router(pool: PgPool) -> Router {
    Router::new()
        .route("/metrics", get(scrape))
        .with_state((pool, Target::Worker))
}

async fn scrape(State((pool, target)): State<(PgPool, Target)>) -> Result<Response, StatusCode> {
    let mut families = Vec::new();
    match target {
        Target::Api(api) => {
            families.extend(api.responses.collect());
            families.extend(api.duration.collect());
            families.extend(api.rate_limited.collect());
            families.extend(api.sse.collect());
            // A labelled family has no samples until the first matching response.
            families.retain(|family| !family.get_metric().is_empty());
        }
        Target::Worker => {
            let age = tokio::time::timeout(
                Duration::from_secs(2),
                sqlx::query_scalar::<_, f64>("SELECT auth.oldest_unsent_email_age_seconds()")
                    .fetch_one(&pool),
            )
            .await
            .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?
            .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
            let oldest = Gauge::new(
                "turnesta_outbox_oldest_unsent_email_age_seconds",
                "Age of the oldest pending or leased invitation email, or zero for an empty outbox.",
            ).expect("static outbox metric");
            oldest.set(age);
            families.extend(oldest.collect());
        }
    }
    let connections = IntGaugeVec::new(
        Opts::new(
            "turnesta_db_pool_connections",
            "SQLx pool connections by state.",
        ),
        &["state"],
    )
    .expect("static pool metric");
    let size = i64::from(pool.size());
    let idle = (pool.num_idle() as i64).min(size);
    connections.with_label_values(&["idle"]).set(idle);
    connections.with_label_values(&["in_use"]).set(size - idle);
    families.extend(connections.collect());
    let capacity = IntGauge::new(
        "turnesta_db_pool_max_connections",
        "Configured SQLx pool capacity.",
    )
    .expect("static pool capacity metric");
    capacity.set(i64::from(pool.options().get_max_connections()));
    families.extend(capacity.collect());
    let encoder = TextEncoder::new();
    let body = encoder
        .encode_to_string(&families)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(([(CONTENT_TYPE, encoder.format_type())], body).into_response())
}
