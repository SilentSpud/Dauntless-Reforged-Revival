#![allow(clippy::unwrap_used, clippy::expect_used)] // test helpers

use std::io::Write;
use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use service_kit::body::{BodyConfig, BodyError, ExpressBody, parse_bodies};
use service_kit::json::{Json, not_found};
use service_kit::routing::Router;
use tower::ServiceExt;

fn app() -> axum::Router {
    let routes = Router::new()
        .post("/echo", |ExpressBody(body): ExpressBody| async move { Json(body) })
        .post("/ignores-body", || async { "ok" })
        .post("/undaunted/api/register", || async { "ok" });
    let config = BodyConfig {
        // As the metagame's JSON_ERROR_ROUTES: lower case, trailing slashes removed.
        on_error: Arc::new(|parts, error| {
            let path = parts.uri.path().trim_end_matches('/').to_ascii_lowercase();
            (error == BodyError::Malformed && path == "/undaunted/api/register").then(|| {
                let reply = Json(serde_json::json!({"error": "bad_request"}));
                axum::response::IntoResponse::into_response((StatusCode::BAD_REQUEST, reply))
            })
        }),
        ..BodyConfig::default()
    };
    axum::Router::new()
        .fallback_service(routes.into_service((), not_found))
        .layer(axum::middleware::from_fn_with_state(config, parse_bodies))
}

async fn post(path: &str, content_type: &str, encoding: Option<&str>, body: Vec<u8>) -> (StatusCode, String) {
    let mut request = Request::post(path).header("content-type", content_type);
    if let Some(encoding) = encoding {
        request = request.header("content-encoding", encoding);
    }
    let response = app().oneshot(request.body(Body::from(body)).unwrap()).await.unwrap();
    let status = response.status();
    let body = response.into_body().collect().await.unwrap().to_bytes();
    (status, String::from_utf8(body.to_vec()).unwrap())
}

#[tokio::test]
async fn json_and_form_bodies_reach_the_handler() {
    assert_eq!(
        post(
            "/echo",
            "application/json; charset=utf-8",
            None,
            br#"{"b":1,"a":2}"#.to_vec()
        )
        .await,
        (StatusCode::OK, r#"{"b":1,"a":2}"#.into())
    );
    assert_eq!(
        post(
            "/echo",
            "application/x-www-form-urlencoded",
            None,
            b"a=1&b=x+y".to_vec()
        )
        .await
        .1,
        r#"{"a":"1","b":"x y"}"#
    );
    assert_eq!(
        post("/echo", "text/plain", None, b"hello".to_vec()).await.1,
        "null",
        "req.body is undefined"
    );
}

#[tokio::test]
async fn a_malformed_body_is_refused_before_any_route() {
    let (status, page) = post("/ignores-body", "application/json", None, b"{nope".to_vec()).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(page.contains("<pre>Bad Request</pre>"));
    assert_eq!(
        post("/undaunted/api/register/", "application/json", None, b"{nope".to_vec())
            .await
            .1,
        r#"{"error":"bad_request"}"#
    );
    assert_eq!(
        post("/echo", "application/json; charset=latin1", None, b"{}".to_vec())
            .await
            .0,
        StatusCode::UNSUPPORTED_MEDIA_TYPE
    );
}

#[tokio::test]
async fn gzip_bodies_are_inflated() {
    let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    gz.write_all(br#"{"z":true}"#).unwrap();
    assert_eq!(
        post("/echo", "application/json", Some("gzip"), gz.finish().unwrap())
            .await
            .1,
        r#"{"z":true}"#
    );
    assert_eq!(
        post("/echo", "application/json", Some("zstd"), b"{}".to_vec()).await.0,
        StatusCode::UNSUPPORTED_MEDIA_TYPE
    );
}
