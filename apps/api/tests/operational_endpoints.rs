use std::time::Duration;
use turnesta_api::auth::delivery::DeliveryKey;

use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode},
};
use serde_json::Value;
use sqlx::postgres::PgPoolOptions;
use tower::ServiceExt;
use turnesta_api::auth::AuthConfig;
use turnesta_api::http::{AppState, router};

fn offline_state() -> AppState {
    let pool = PgPoolOptions::new()
        .acquire_timeout(Duration::from_millis(30))
        .connect_lazy("postgres://user:password@127.0.0.1:1/turnesta")
        .expect("valid test URL");
    AppState::new(
        pool,
        Duration::from_millis(50),
        DeliveryKey::from_hex(&"17".repeat(32)).unwrap(),
    )
}

#[test]
fn api_and_worker_reject_invalid_delivery_keys_before_connecting() {
    for args in [
        &[][..],
        &["serve"][..],
        &["worker"][..],
        &["worker", "--once"][..],
    ] {
        for value in [None, Some(""), Some("too-short"), Some(&"xy".repeat(32))] {
            let mut cmd = std::process::Command::new(env!("CARGO_BIN_EXE_turnesta-api"));
            cmd.args(args)
                .env("DATABASE_URL", "postgres://localhost:1/unused")
                .env("TURNESTA_TIPS_MODE", "disabled")
                .env_remove("TURNESTA_DELIVERY_KEY");
            if let Some(value) = value {
                cmd.env("TURNESTA_DELIVERY_KEY", value);
            }
            let output = cmd.output().unwrap();
            assert_eq!(output.status.code(), Some(1));
            let stderr = String::from_utf8(output.stderr).unwrap();
            assert!(stderr.contains("TURNESTA_DELIVERY_KEY must contain a 32-byte key"));
            if let Some(value) = value.filter(|s| !s.is_empty()) {
                assert!(!stderr.contains(value));
                assert!(!String::from_utf8_lossy(&output.stdout).contains(value));
            }
        }
    }
}

async fn json(response: axum::response::Response) -> Value {
    let bytes = to_bytes(response.into_body(), 64 * 1024)
        .await
        .expect("bounded response body");
    serde_json::from_slice(&bytes).expect("JSON response")
}

#[tokio::test]
async fn worker_scrape_fails_when_outbox_age_is_unknown() {
    let pool = PgPoolOptions::new()
        .connect_lazy("postgres://localhost/unused")
        .unwrap();
    pool.close().await;
    let res = turnesta_api::metrics::worker_router(pool)
        .oneshot(Request::get("/metrics").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::SERVICE_UNAVAILABLE);
}

#[tokio::test]
async fn idle_api_can_be_scraped_before_the_first_request() {
    let res = offline_state()
        .metrics_router()
        .oneshot(Request::get("/metrics").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
}

#[tokio::test]
async fn metrics_are_internal_and_aggregate_only_route_templates_and_limit_reasons() {
    let state = offline_state().with_client_ip_header(Some("x-real-ip".parse().unwrap()));
    let metrics = state.metrics_router();
    let app = router(state);
    for (path, status) in [
        ("/metrics", StatusCode::NOT_FOUND),
        ("/private-path?token=secret", StatusCode::NOT_FOUND),
        ("/api/public/queues/first/events", StatusCode::BAD_REQUEST),
        ("/api/public/queues/second/events", StatusCode::BAD_REQUEST),
        ("/book/secret", StatusCode::TOO_MANY_REQUESTS),
    ] {
        let res = app
            .clone()
            .oneshot(Request::get(path).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(res.status(), status, "{path}");
    }
    let res = metrics
        .oneshot(Request::get("/metrics").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    assert_eq!(res.headers()["content-type"], "text/plain; version=0.0.4");
    let text =
        String::from_utf8(to_bytes(res.into_body(), 64 * 1024).await.unwrap().to_vec()).unwrap();
    for sample in [
        r#"turnesta_http_responses_total{route="/api/public/queues/{slug}/events",status="400"} 2"#,
        r#"turnesta_http_responses_total{route="<unmatched>",status="404"} 2"#,
        r#"turnesta_http_request_duration_seconds_count{route="/api/public/queues/{slug}/events"} 2"#,
        r#"turnesta_http_rate_limited_total{reason="public_client_admission"} 1"#,
        r#"turnesta_db_pool_connections{state="in_use"} 0"#,
        "turnesta_sse_connections 0",
    ] {
        assert!(text.contains(sample), "{sample} missing from {text}");
    }
    for private in [
        "first",
        "second/events",
        "private-path",
        "secret",
        "oldest_unsent_email",
    ] {
        assert!(
            !text.contains(private),
            "unexpected label or metric: {private}"
        );
    }
}

#[tokio::test]
async fn liveness_does_not_depend_on_postgres() {
    let response = router(offline_state())
        .oneshot(Request::get("/health").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()["cache-control"], "no-store");
    let request_id = response.headers()["x-request-id"].to_str().unwrap();
    assert!(uuid::Uuid::parse_str(request_id).is_ok());
    assert_eq!(json(response).await["status"], "alive");
}

#[tokio::test]
async fn missing_database_returns_unavailable_instead_of_false_readiness() {
    let response = router(offline_state())
        .oneshot(Request::get("/health/ready").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(json(response).await["status"], "not_ready");
}

#[tokio::test]
async fn api_contract_contains_operational_routes() {
    let response = router(offline_state().with_expose_openapi(true))
        .oneshot(Request::get("/openapi.json").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = to_bytes(response.into_body(), 256 * 1024).await.unwrap();
    let document: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert!(document["paths"]["/health"]["get"].is_object());
    assert!(document["paths"]["/health/ready"]["get"]["responses"]["503"].is_object());
}

#[tokio::test]
async fn openapi_is_local_or_explicitly_enabled() {
    for (origin, exposed, expected) in [
        ("http://localhost:8080", false, StatusCode::OK),
        ("http://[::1]:8080", false, StatusCode::OK),
        ("https://127.0.0.1", false, StatusCode::OK),
        ("https://turnesta.example", false, StatusCode::NOT_FOUND),
        ("https://localhost.example", false, StatusCode::NOT_FOUND),
        ("https://turnesta.example", true, StatusCode::OK),
    ] {
        let state = offline_state()
            .with_auth(AuthConfig::parse(origin, 3600).unwrap())
            .with_expose_openapi(exposed);
        let response = router(state)
            .oneshot(Request::get("/openapi.json").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), expected, "{origin} expose={exposed}");
    }
}

#[tokio::test]
async fn unknown_routes_and_wrong_methods_return_json_errors() {
    for (request, expected, code) in [
        (Request::get("/missing"), StatusCode::NOT_FOUND, "not_found"),
        (
            Request::post("/health"),
            StatusCode::METHOD_NOT_ALLOWED,
            "method_not_allowed",
        ),
    ] {
        let response = router(offline_state())
            .oneshot(request.body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), expected);
        assert_eq!(json(response).await["code"], code);
    }
}

#[tokio::test]
async fn server_generates_its_own_request_id() {
    let response = router(offline_state())
        .oneshot(
            Request::get("/health")
                .header("x-request-id", "client-provided-id")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_ne!(response.headers()["x-request-id"], "client-provided-id");
}

#[tokio::test]
async fn identity_headers_do_not_replace_a_session() {
    let response = router(offline_state())
        .oneshot(
            Request::get("/api/organizations")
                .header("x-user-id", uuid::Uuid::new_v4().to_string())
                .header("x-tenant-id", uuid::Uuid::new_v4().to_string())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(json(response).await["code"], "unauthorized");
}

#[tokio::test]
async fn login_rejects_missing_or_foreign_origin_before_accessing_database() {
    for origin in [None, Some("https://foreign.example")] {
        let mut request = Request::post("/auth/login").header("content-type", "application/json");
        if let Some(origin) = origin {
            request = request.header("origin", origin);
        }
        let response = router(offline_state())
            .oneshot(request.body(Body::from(r#"{"code":"invalid"}"#)).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }
}

#[tokio::test]
async fn login_page_and_scripts_have_browser_security_headers() {
    for path in [
        "/login",
        "/assets/login.js",
        "/assets/login.css",
        "/assets/password-login.js",
        "/assets/password-login.css",
    ] {
        let response = router(offline_state())
            .oneshot(Request::get(path).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()["referrer-policy"], "no-referrer");
        assert_eq!(response.headers()["x-content-type-options"], "nosniff");
        assert!(
            response.headers()["content-security-policy"]
                .to_str()
                .unwrap()
                .contains("frame-ancestors 'none'")
        );
    }
}

#[tokio::test]
async fn cabinet_deep_links_return_html_but_missing_assets_stay_404() {
    let directory =
        std::env::temp_dir().join(format!("turnesta-web-test-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(directory.join("assets")).unwrap();
    std::fs::write(
        directory.join("index.html"),
        "<!doctype html><title>Cabinet fixture</title>",
    )
    .unwrap();
    std::fs::write(directory.join("assets/app-test.js"), "export {};").unwrap();
    std::fs::write(
        directory.join("favicon.svg"),
        "<svg xmlns=\"http://www.w3.org/2000/svg\"/>",
    )
    .unwrap();
    let app = router(offline_state().with_web_dist(directory));
    let response = app
        .clone()
        .oneshot(
            Request::get("/app/organizations/example/locations")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()["cache-control"], "no-store");
    assert!(
        response.headers()["content-type"]
            .to_str()
            .unwrap()
            .starts_with("text/html")
    );
    let response = app
        .clone()
        .oneshot(
            Request::get("/app/assets/app-test.js")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers()["cache-control"],
        "public, max-age=31536000, immutable"
    );
    let response = app
        .clone()
        .oneshot(
            Request::get("/app/favicon.svg")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()["content-type"], "image/svg+xml");
    let response = app
        .oneshot(
            Request::get("/app/assets/missing.js")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    assert_eq!(response.headers()["cache-control"], "no-store");
}
