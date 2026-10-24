//! The HTTP side: `POST /graphql` for the bot, `GET /health` for `reforged-host start`'s ready probe.
//! Every GraphQL request is checked before its body is read: a loopback peer, no proxy headers, the
//! API key (`input::check_headers`).

use std::sync::Arc;

use axum::Router;
use axum::body::Body;
use axum::extract::{ConnectInfo, Request, State};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use serde_json::json;
use service_kit::serve::ConnectionInfo;

use crate::input::{self, Refusal};
use crate::log;
use crate::schema::AdminSchema;

#[derive(Clone)]
pub struct AppState {
    pub schema: AdminSchema,
    pub api_key: Arc<str>,
}

fn json_reply(status: StatusCode, body: serde_json::Value) -> Response {
    (
        status,
        [
            (header::CONTENT_TYPE, "application/json; charset=utf-8"),
            (header::CACHE_CONTROL, "no-store"),
        ],
        body.to_string(),
    )
        .into_response()
}

fn refuse(refusal: Refusal) -> Response {
    let status = StatusCode::from_u16(refusal.status()).unwrap_or(StatusCode::BAD_REQUEST);
    json_reply(
        status,
        json!({"errors": [{"message": refusal.code().to_lowercase().replace('_', " "), "extensions": {"code": refusal.code()}}]}),
    )
}

async fn graphql(
    State(state): State<AppState>,
    ConnectInfo(info): ConnectInfo<ConnectionInfo>,
    request: Request,
) -> Response {
    // The listener is on loopback already; a peer elsewhere means a bind setting went wrong
    if !info.peer.ip().is_loopback() {
        return refuse(Refusal::Forwarded);
    }
    let (parts, body) = request.into_parts();
    let operator = match input::check_headers(&parts.headers, &state.api_key) {
        Ok(operator) => operator,
        Err(refusal) => {
            log::warn("admin API request refused", json!({"code": refusal.code()}));
            return refuse(refusal);
        }
    };
    if !input::is_json(&parts.headers) {
        return refuse(Refusal::NotJson);
    }
    let Ok(bytes) = axum::body::to_bytes(body, input::MAX_BODY).await else {
        return refuse(Refusal::BadBody);
    };
    let request = match input::parse_body(&bytes) {
        Ok(request) => request,
        Err(refusal) => return refuse(refusal),
    };
    let response = state.schema.execute(request.data(operator)).await;
    match serde_json::to_value(&response) {
        Ok(body) => json_reply(StatusCode::OK, body),
        Err(_) => json_reply(
            StatusCode::INTERNAL_SERVER_ERROR,
            json!({"errors": [{"message": "internal error"}]}),
        ),
    }
}

async fn health() -> Response {
    json_reply(StatusCode::OK, json!({"ok": true}))
}

async fn not_found() -> Response {
    (StatusCode::NOT_FOUND, Body::empty()).into_response()
}

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/graphql", post(graphql))
        .route("/health", get(health))
        .fallback(not_found)
        .with_state(state)
}
