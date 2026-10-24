//! The Director's HTTP API (`src/app.ts`, `src/routes/matchmaker.ts`, `src/p2p/index.ts`), on
//! loopback for the backend only: it has no authentication (Reforged binds 127.0.0.1 unless
//! BIND_HOST says otherwise).
//!
//! In Express's order: the three read-only GETs come before everything else, so they get no request
//! id and no body parsing; then the request id, the JSON and form parsers (100 kB, Express's
//! default), the request log, and the matchmaker routes. Paths match as Express matches them: any
//! case, one trailing slash, HEAD with GET. Anything else is Express's production 404 page.

use std::sync::Arc;
use std::time::Instant;

use axum::Router;
use axum::body::Body;
use axum::extract::{Request, State};
use axum::http::{HeaderValue, Method, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use serde_json::json;
use service_kit::body::{BodyConfig, ExpressBody, error_page, parse_bodies};
use service_kit::json::Json;

use crate::gameservers::Director;
use crate::hunt;
use crate::log;
use crate::matchmaker;

pub const REQUEST_ID_HEADER: &str = "x-mysticparadox-request-id";
const JSON_LIMIT: usize = 100 * 1024;

/// The route a path names, as Express compares it: case-insensitive, one trailing slash ignored.
fn route_of(path: &str) -> String {
    let path = path.strip_suffix('/').filter(|p| !p.is_empty()).unwrap_or(path);
    path.to_ascii_lowercase()
}

fn is_get(method: &Method) -> bool {
    method == Method::GET || method == Method::HEAD
}

fn strip_head(method: &Method, mut response: Response) -> Response {
    if method == Method::HEAD {
        *response.body_mut() = Body::empty();
    }
    response
}

/// A safe client-sent id (letters, digits, `._:-`, 8-96 characters, starting alphanumeric).
pub fn normalize_request_id(value: Option<&HeaderValue>) -> Option<String> {
    let text = service_kit::js::trim(value?.to_str().ok()?);
    let ok = (8..=96).contains(&text.len())
        && text.bytes().next().is_some_and(|b| b.is_ascii_alphanumeric())
        && text
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b':' | b'-'));
    ok.then(|| text.to_owned())
}

pub fn router(director: Arc<Director>) -> Router {
    let inner = Router::new()
        .fallback(routes)
        .layer(middleware::from_fn(request_log))
        .layer(middleware::from_fn_with_state(
            BodyConfig {
                limit: JSON_LIMIT,
                ..BodyConfig::default()
            },
            parse_bodies,
        ))
        .layer(middleware::from_fn(request_id))
        .with_state(director.clone());
    Router::new()
        .fallback(move |State(director): State<Arc<Director>>, request: Request| {
            let inner = inner.clone();
            async move {
                use tower::ServiceExt;
                let method = request.method().clone();
                if is_get(&method) {
                    let reply = match route_of(request.uri().path()).as_str() {
                        "/health/ready" => {
                            let ready = director.persistent_hubs_ready();
                            let status = if ready {
                                StatusCode::OK
                            } else {
                                StatusCode::SERVICE_UNAVAILABLE
                            };
                            Some((status, Json(json!({"ok": ready, "service": "deployserver"}))).into_response())
                        }
                        // 1.14.7 FENCING: each hub is null unless ready and its process still alive.
                        "/api/hub-status" => Some(Json(director.persistent_hub_status()).into_response()),
                        // Running game servers, for the backend's launcher status.
                        "/api/instances" => Some(Json(director.instances_for_status()).into_response()),
                        _ => None,
                    };
                    if let Some(reply) = reply {
                        return strip_head(&method, reply);
                    }
                }
                match inner.oneshot(request).await {
                    Ok(response) => strip_head(&method, response),
                    Err(never) => match never {},
                }
            }
        })
        .with_state(director)
}

async fn request_id(mut request: Request, next: Next) -> Response {
    let headers = request.headers();
    let id = normalize_request_id(headers.get(REQUEST_ID_HEADER))
        .or_else(|| normalize_request_id(headers.get("x-request-id")))
        .unwrap_or_else(crate::random::uuid_v4);
    request.extensions_mut().insert(RequestId(id.clone()));
    let mut response = log::REQUEST_ID.scope(id.clone(), next.run(request)).await;
    if let Ok(value) = HeaderValue::from_str(&id) {
        response.headers_mut().insert(REQUEST_ID_HEADER, value);
    }
    response
}

#[derive(Debug, Clone)]
struct RequestId(String);

async fn request_log(request: Request, next: Next) -> Response {
    let started = Instant::now();
    let id = request
        .extensions()
        .get::<RequestId>()
        .map_or_else(|| "missing".to_owned(), |r| r.0.clone());
    let method = request.method().clone();
    let url = request
        .uri()
        .path_and_query()
        .map_or_else(|| request.uri().path().to_owned(), |p| p.as_str().to_owned());
    log::info(&format!("[REQ] requestId={id} {method} {url}"));
    let response = next.run(request).await;
    log::info(&format!(
        "[RES] requestId={id} {method} {url} -> {} ({}ms)",
        response.status().as_u16(),
        started.elapsed().as_millis()
    ));
    response
}

async fn routes(State(director): State<Arc<Director>>, body: ExpressBody, request: Request) -> Response {
    let method = request.method().clone();
    let route = route_of(request.uri().path());
    if method == Method::POST && route == "/api/matchmaker/handle-matchmaking-for-player" {
        return match matchmaker::handle(&director, body.0.as_ref()).await {
            Ok(details) => Json(details).into_response(),
            Err(error) => {
                // The final safety net: the metagame treats a non-200 as a failed matchmaking.
                log::error(&format!("handle-matchmaking-for-player failed: {error}"));
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(json!({"error": "matchmaking_failed"})),
                )
                    .into_response()
            }
        };
    }
    // The player-hosted hunt module's resolver: no spawn, port, process or credential, only the
    // manifest the Metagame freezes before it picks a player host.
    if director.config.p2p_module && method == Method::POST && route == "/api/matchmaker/resolve-hunt-launch-manifest" {
        let manifest = match body.0.as_ref() {
            Some(body) => hunt::resolve_player_hosted_manifest(
                &director.resolver,
                body.get("HuntId"),
                body.get("ExpectedPlayers"),
            ),
            None => Err("Cannot read properties of undefined (reading 'HuntId')".to_owned()),
        };
        return match manifest {
            Ok(manifest) => Json(manifest).into_response(),
            Err(error) => {
                log::error(&format!("resolve-hunt-launch-manifest failed: {error}"));
                (StatusCode::BAD_REQUEST, Json(json!({"error": "hunt_manifest_invalid"}))).into_response()
            }
        };
    }
    error_page(StatusCode::NOT_FOUND)
}
