#![allow(clippy::unwrap_used, clippy::expect_used)] // test helpers

use std::sync::Arc;
use std::time::Duration;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use service_kit::json::not_found;
use service_kit::ratelimit::{Limited, Limiter, enforce};
use service_kit::routing::Router;
use tower::ServiceExt;

fn app(limiter: Arc<Limiter>) -> axum::Router {
    let limited = Limited::new(limiter, |request| {
        request
            .headers()
            .get("x-client")
            .map(|v| v.to_str().unwrap().to_owned())
    });
    let routes = Router::new()
        .post("/login/:outcome", |p: service_kit::routing::Params| async move {
            if p.get("outcome") == Some("ok") {
                StatusCode::OK
            } else {
                StatusCode::UNAUTHORIZED
            }
        })
        .layer(axum::middleware::from_fn_with_state(limited, enforce));
    axum::Router::new().fallback_service(routes.into_service((), not_found))
}

async fn login(app: &axum::Router, outcome: &str) -> axum::response::Response {
    let request = Request::post(format!("/login/{outcome}"))
        .header("x-client", "203.0.113.9")
        .body(Body::empty())
        .unwrap();
    app.clone().oneshot(request).await.unwrap()
}

#[tokio::test]
async fn failed_requests_count_and_the_limit_answers_429() {
    let reply = serde_json::json!({"error": "rate_limited", "message": "Too many requests; try again later."});
    let app = app(Arc::new(
        Limiter::new(2, Duration::from_secs(600), reply).skip_successful(),
    ));

    for _ in 0..5 {
        assert_eq!(
            login(&app, "ok").await.status(),
            StatusCode::OK,
            "successes give their hit back"
        );
    }
    assert_eq!(login(&app, "bad").await.status(), StatusCode::UNAUTHORIZED);
    let second = login(&app, "bad").await;
    assert_eq!(second.headers()["ratelimit"], "\"2-in-10min\"; r=0; t=600");
    let limited = login(&app, "bad").await;
    assert_eq!(limited.status(), StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(limited.headers()["retry-after"], "600");
    assert!(
        limited.headers()["ratelimit-policy"]
            .to_str()
            .unwrap()
            .starts_with("\"2-in-10min\"; q=2; w=600; pk=:")
    );
    let body = limited.into_body().collect().await.unwrap().to_bytes();
    assert_eq!(
        &body[..],
        br#"{"error":"rate_limited","message":"Too many requests; try again later."}"#
    );
}
