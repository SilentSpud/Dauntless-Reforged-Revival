//! Request bodies as `express.json({limit: "50mb"})` and `express.urlencoded({extended: true})` parse
//! them in the Node services (MIGRATION_PLAN.md M8).
//!
//! Like Express's app-wide body parsers, [`parse_bodies`] runs before routing, for every request: a
//! malformed body is refused even on a route that never reads it. Handlers read the result with
//! [`ExpressBody`]: `Some(value)` for a JSON or form body, `None` for any other content type (where
//! Express leaves `req.body` undefined). What matches Express:
//!
//! - JSON (`application/json`): an empty body is `{}`; anything but an object or array at the top
//!   is refused (strict mode); the charset must be UTF-8.
//! - Forms (`application/x-www-form-urlencoded`): `+` is a space, a value that isn't valid
//!   percent-encoding stays as it is, a repeated field becomes an array, at most 1000 fields. Fields
//!   are flat: `a[b]=1` is the field `a[b]`, where `qs` would nest it.
//! - `Content-Encoding` gzip and deflate are inflated; the 50 MB limit applies to the inflated body.
//! - Errors are Express's production error page ([`error_page`]): 400 for a malformed body, 413 for
//!   one over the limit, 415 for an unsupported charset or encoding. A service can answer some of
//!   them its own way ([`BodyConfig::on_error`]).

use std::io::Read;
use std::sync::Arc;

use axum::body::Body;
use axum::extract::{FromRequestParts, Request, State};
use axum::http::header::{CONTENT_ENCODING, CONTENT_TYPE, HeaderValue};
use axum::http::request::Parts;
use axum::http::{HeaderMap, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use percent_encoding::percent_decode_str;
use serde_json::{Map, Value};

pub const DEFAULT_LIMIT: usize = 50 * 1024 * 1024;
const PARAMETER_LIMIT: usize = 1000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BodyError {
    /// Not valid JSON, or not an object or array at the top (`entity.parse.failed`).
    Malformed,
    TooLarge,
    TooManyParameters,
    UnsupportedCharset,
    UnsupportedEncoding,
    /// The connection failed while the body was read.
    Aborted,
}

impl BodyError {
    pub fn status(self) -> StatusCode {
        match self {
            Self::Malformed | Self::Aborted => StatusCode::BAD_REQUEST,
            Self::TooLarge | Self::TooManyParameters => StatusCode::PAYLOAD_TOO_LARGE,
            Self::UnsupportedCharset | Self::UnsupportedEncoding => StatusCode::UNSUPPORTED_MEDIA_TYPE,
        }
    }
}

impl IntoResponse for BodyError {
    fn into_response(self) -> Response {
        error_page(self.status())
    }
}

/// Express's error reply with `NODE_ENV=production` (`finalhandler`): a small HTML page naming the
/// status, with its security headers.
pub fn error_page(status: StatusCode) -> Response {
    let message = status.canonical_reason().unwrap_or("Error");
    let body = format!(
        "<!DOCTYPE html>\n<html lang=\"en\">\n<head>\n<meta charset=\"utf-8\">\n<title>Error</title>\n</head>\n<body>\n<pre>{message}</pre>\n</body>\n</html>\n"
    );
    let mut response = (status, body).into_response();
    let headers = response.headers_mut();
    headers.insert(
        "content-security-policy",
        HeaderValue::from_static("default-src 'none'"),
    );
    headers.insert("x-content-type-options", HeaderValue::from_static("nosniff"));
    headers.insert(CONTENT_TYPE, HeaderValue::from_static("text/html; charset=utf-8"));
    response
}

/// A service's own answer to a body error, or `None` for Express's error page.
pub type OnBodyError = dyn Fn(&Parts, BodyError) -> Option<Response> + Send + Sync;

/// How [`parse_bodies`] works for one service.
#[derive(Clone)]
pub struct BodyConfig {
    pub limit: usize,
    /// Answers a body error on some routes differently (the metagame's `JSON_ERROR_ROUTES`); `None`
    /// keeps Express's error page.
    pub on_error: Arc<OnBodyError>,
}

impl Default for BodyConfig {
    fn default() -> Self {
        Self {
            limit: DEFAULT_LIMIT,
            on_error: Arc::new(|_, _| None),
        }
    }
}

/// The parsed body, stored on the request by [`parse_bodies`].
#[derive(Debug, Clone)]
struct Parsed(Option<Value>);

/// The request's body as Express's parsers left it in `req.body`.
#[derive(Debug, Clone, PartialEq)]
pub struct ExpressBody(pub Option<Value>);

impl<S: Send + Sync> FromRequestParts<S> for ExpressBody {
    type Rejection = std::convert::Infallible;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        Ok(Self(
            parts.extensions.get::<Parsed>().and_then(|parsed| parsed.0.clone()),
        ))
    }
}

enum Kind {
    Json,
    Form,
}

fn kind(headers: &HeaderMap) -> Result<Option<Kind>, BodyError> {
    let Some(content_type) = headers.get(CONTENT_TYPE).and_then(|v| v.to_str().ok()) else {
        return Ok(None);
    };
    let mut parts = content_type.split(';');
    let essence = parts.next().unwrap_or("").trim().to_ascii_lowercase();
    let charset = parts
        .filter_map(|p| p.split_once('='))
        .find(|(name, _)| name.trim().eq_ignore_ascii_case("charset"))
        .map(|(_, value)| value.trim().trim_matches('"').to_ascii_lowercase());
    let kind = match essence.as_str() {
        "application/json" => Kind::Json,
        "application/x-www-form-urlencoded" => Kind::Form,
        _ => return Ok(None),
    };
    match charset.as_deref() {
        None | Some("utf-8") => Ok(Some(kind)),
        _ => Err(BodyError::UnsupportedCharset),
    }
}

/// Middleware: parses JSON and form bodies before routing (`axum::middleware::from_fn_with_state`).
pub async fn parse_bodies(State(config): State<BodyConfig>, request: Request, next: Next) -> Response {
    let (mut parts, body) = request.into_parts();
    match read(&parts.headers, body, config.limit).await {
        Ok((parsed, body)) => {
            parts.extensions.insert(Parsed(parsed));
            next.run(Request::from_parts(parts, body)).await
        }
        Err(error) => (config.on_error)(&parts, error).unwrap_or_else(|| error.into_response()),
    }
}

async fn read(headers: &HeaderMap, body: Body, limit: usize) -> Result<(Option<Value>, Body), BodyError> {
    let Some(kind) = kind(headers)? else {
        return Ok((None, body));
    };
    let raw = axum::body::to_bytes(body, limit).await.map_err(|e| {
        if std::error::Error::source(&e).is_some_and(|s| s.is::<http_body_util::LengthLimitError>()) {
            BodyError::TooLarge
        } else {
            BodyError::Aborted
        }
    })?;
    let bytes = inflate(headers, &raw, limit)?;
    let text = std::str::from_utf8(&bytes).map_err(|_| BodyError::Malformed)?;
    let value = match kind {
        Kind::Json => parse_json(text)?,
        Kind::Form => parse_form(text)?,
    };
    Ok((Some(value), Body::from(raw)))
}

fn inflate(headers: &HeaderMap, raw: &[u8], limit: usize) -> Result<Vec<u8>, BodyError> {
    let encoding = headers
        .get(CONTENT_ENCODING)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("identity")
        .trim()
        .to_ascii_lowercase();
    let reader: Box<dyn Read + '_> = match encoding.as_str() {
        "identity" => return Ok(raw.to_vec()),
        "gzip" => Box::new(flate2::read::GzDecoder::new(raw)),
        "deflate" => Box::new(flate2::read::ZlibDecoder::new(raw)),
        _ => return Err(BodyError::UnsupportedEncoding),
    };
    let mut out = Vec::new();
    reader
        .take(limit as u64 + 1)
        .read_to_end(&mut out)
        .map_err(|_| BodyError::Malformed)?;
    if out.len() > limit {
        return Err(BodyError::TooLarge);
    }
    Ok(out)
}

/// `JSON.parse` in body-parser's strict mode.
pub fn parse_json(text: &str) -> Result<Value, BodyError> {
    if text.is_empty() {
        return Ok(Value::Object(Map::new()));
    }
    match text.trim_start_matches([' ', '\t', '\n', '\r']).chars().next() {
        Some('{' | '[') => serde_json::from_str(text).map_err(|_| BodyError::Malformed),
        _ => Err(BodyError::Malformed),
    }
}

/// A form body as flat fields, decoded like `qs`.
pub fn parse_form(text: &str) -> Result<Value, BodyError> {
    let pairs: Vec<&str> = text.split('&').filter(|p| !p.is_empty()).collect();
    if pairs.len() > PARAMETER_LIMIT {
        return Err(BodyError::TooManyParameters);
    }
    let mut fields = Map::new();
    for pair in pairs {
        let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
        let key = decode(key);
        if key.is_empty() {
            continue;
        }
        let value = Value::String(decode(value));
        match fields.get_mut(&key) {
            None => {
                fields.insert(key, value);
            }
            Some(Value::Array(values)) => values.push(value),
            Some(first) => *first = Value::Array(vec![first.take(), value]),
        }
    }
    Ok(Value::Object(fields))
}

fn decode(text: &str) -> String {
    let spaced = text.replace('+', " ");
    match percent_decode_str(&spaced).decode_utf8() {
        Ok(decoded) => decoded.into_owned(),
        Err(_) => spaced,
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn json_is_strict_like_body_parser() {
        assert_eq!(parse_json(""), Ok(json!({})));
        assert_eq!(parse_json(" \n{\"a\":1}"), Ok(json!({"a": 1})));
        assert_eq!(parse_json("[1,2]"), Ok(json!([1, 2])));
        assert_eq!(parse_json("\"text\""), Err(BodyError::Malformed));
        assert_eq!(parse_json("42"), Err(BodyError::Malformed));
        assert_eq!(parse_json("{\"a\":"), Err(BodyError::Malformed));
    }

    #[test]
    fn duplicate_json_keys_keep_the_last_value_in_the_first_place() {
        let parsed = parse_json(r#"{"a":1,"b":2,"a":3}"#).unwrap();
        assert_eq!(serde_json::to_string(&parsed).unwrap(), r#"{"a":3,"b":2}"#);
    }

    #[test]
    fn forms_decode_like_qs() {
        let form = parse_form("grant_type=exchange_code&code=a%2Bb+c&x=%zz&x=2&empty&=skipped").unwrap();
        assert_eq!(
            form,
            json!({"grant_type": "exchange_code", "code": "a+b c", "x": ["%zz", "2"], "empty": ""})
        );
        let too_many = vec!["a=1"; 1001].join("&");
        assert_eq!(parse_form(&too_many), Err(BodyError::TooManyParameters));
    }

    #[test]
    fn the_error_page_is_expresss() {
        let response = error_page(StatusCode::BAD_REQUEST);
        assert_eq!(response.headers()[CONTENT_TYPE], "text/html; charset=utf-8");
        assert_eq!(response.headers()["content-security-policy"], "default-src 'none'");
    }
}
