#![allow(clippy::unwrap_used, clippy::expect_used)] // test helpers

use axum::body::Body;
use axum::extract::State;
use axum::http::{Request, StatusCode, header};
use http_body_util::BodyExt;
use service_kit::json::{JSON_UTF8, Json, not_found};
use service_kit::routing::{Params, Router};
use tower::ServiceExt;

async fn by_id(params: Params) -> String {
    format!("by id {}", params.get("id").unwrap())
}

#[derive(Clone)]
struct Greeting(&'static str);

fn app() -> axum::Router {
    // Express order: the parameter route is registered first, so it wins over the literal one.
    let routes = Router::new()
        .get("/items/:id", |params: Params| async move {
            format!("by id {}", params.get("id").unwrap())
        })
        .get("/items/special", || async { "special" })
        .get("/greeting", |State(greeting): State<Greeting>| async move {
            Json(serde_json::json!({"text": greeting.0}))
        })
        .post("/items/:id", by_id_post);
    axum::Router::new().fallback_service(routes.into_service(Greeting("Clear Skies"), not_found))
}

async fn by_id_post(params: Params) -> String {
    by_id(params).await.replace("by id", "posted")
}

async fn call(method: &str, path: &str) -> (StatusCode, Option<String>, String) {
    let request = Request::builder().method(method).uri(path).body(Body::empty()).unwrap();
    let response = app().oneshot(request).await.unwrap();
    let status = response.status();
    let content_type = response
        .headers()
        .get(header::CONTENT_TYPE)
        .map(|v| v.to_str().unwrap().to_owned());
    let body = response.into_body().collect().await.unwrap().to_bytes();
    (status, content_type, String::from_utf8(body.to_vec()).unwrap())
}

#[tokio::test]
async fn the_first_registered_route_wins_case_insensitively() {
    assert_eq!(call("GET", "/ITEMS/Special").await.2, "by id Special");
    assert_eq!(call("GET", "/items/A%2FB/").await.2, "by id A/B");
    assert_eq!(call("POST", "/Items/7").await.2, "posted 7");
}

#[tokio::test]
async fn head_is_answered_by_the_get_route_without_a_body() {
    let (status, _, body) = call("HEAD", "/items/1").await;
    assert_eq!((status, body.as_str()), (StatusCode::OK, ""));
}

#[tokio::test]
async fn unmatched_requests_get_an_empty_404() {
    assert_eq!(
        call("GET", "/nothing").await,
        (StatusCode::NOT_FOUND, None, String::new())
    );
    assert_eq!(
        call("DELETE", "/items/1").await.0,
        StatusCode::NOT_FOUND,
        "no route for the method"
    );
}

#[tokio::test]
async fn a_badly_encoded_parameter_is_a_400() {
    assert_eq!(call("GET", "/items/%E2%28").await.0, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn json_replies_name_their_charset() {
    let (status, content_type, body) = call("GET", "/greeting").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(content_type.as_deref(), Some(JSON_UTF8));
    assert_eq!(body, r#"{"text":"Clear Skies"}"#);
}
