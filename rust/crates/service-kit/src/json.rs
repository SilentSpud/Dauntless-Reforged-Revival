//! JSON replies as Express's `res.json` sends them (MIGRATION_PLAN.md M7, SB1): the body is the text
//! `JSON.stringify` writes (`crate::jsjson`) and the type is `application/json; charset=utf-8` (axum's own
//! `Json` leaves the charset out). No ETag (D16) and no `X-Powered-By`.

use axum::http::StatusCode;
use axum::http::header::{CONTENT_TYPE, HeaderValue};
use axum::response::{IntoResponse, Response};
use serde::Serialize;

pub const JSON_UTF8: &str = "application/json; charset=utf-8";

pub struct Json<T>(pub T);

impl<T: Serialize> IntoResponse for Json<T> {
    fn into_response(self) -> Response {
        // Written as JSON.stringify writes it (numbers, escapes, key order): crate::jsjson.
        match serde_json::to_value(&self.0) {
            Ok(value) => (
                [(CONTENT_TYPE, HeaderValue::from_static(JSON_UTF8))],
                crate::jsjson::stringify(&value),
            )
                .into_response(),
            Err(error) => {
                tracing::error!(err = &error as &dyn std::error::Error, "Could not serialize a reply");
                StatusCode::INTERNAL_SERVER_ERROR.into_response()
            }
        }
    }
}

/// Serializes an `f64` as JavaScript's `JSON.stringify` writes a number (SB1): a whole number
/// without a fraction (`1`, not `1.0`), `null` for NaN and infinities, otherwise the shortest form
/// that reads back the same, which `serde_json` and JavaScript agree on. Use with
/// `#[serde(serialize_with = "service_kit::json::js_number")]`.
pub fn js_number<S: serde::Serializer>(value: &f64, serializer: S) -> Result<S::Ok, S::Error> {
    if !value.is_finite() {
        serializer.serialize_none()
    } else if value.fract() == 0.0 && value.abs() < 1e21 {
        // Whole numbers up to 1e21 print without a fraction or exponent in JavaScript.
        if value.abs() <= i64::MAX as f64 {
            serializer.serialize_i64(*value as i64)
        } else {
            serializer.serialize_i128(*value as i128)
        }
    } else {
        serializer.serialize_f64(*value)
    }
}

/// [`js_number`] for an optional number.
pub fn js_number_opt<S: serde::Serializer>(value: &Option<f64>, serializer: S) -> Result<S::Ok, S::Error> {
    match value {
        Some(value) => js_number(value, serializer),
        None => serializer.serialize_none(),
    }
}

/// The reply of a request no route answers: 404 with an empty body, like the Node services' catch-all.
pub fn not_found(_request: &axum::extract::Request) -> Response {
    StatusCode::NOT_FOUND.into_response()
}
