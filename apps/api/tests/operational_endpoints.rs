use std::time::Duration;

use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode},
};
use serde_json::Value;
use sqlx::postgres::PgPoolOptions;
use tower::ServiceExt;
use turnesta_api::http::{AppState, router};

fn offline_state() -> AppState {
    let pool = PgPoolOptions::new()
        .acquire_timeout(Duration::from_millis(30))
        .connect_lazy("postgres://user:password@127.0.0.1:1/turnesta")
        .expect("valid test URL");
    AppState::new(pool, Duration::from_millis(50))
}

async fn json(response: axum::response::Response) -> Value {
    let bytes = to_bytes(response.into_body(), 64 * 1024)
        .await
        .expect("bounded response body");
    serde_json::from_slice(&bytes).expect("JSON response")
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
    let response = router(offline_state())
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
