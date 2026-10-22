//! Outgoing GETs (Node's `fetch` with `redirect: "error"` and a timeout): the backend's account
//! lookup and the GitHub news file. HTTP or HTTPS (rustls with the webpki roots); a redirect is an
//! error, as `redirect: "error"` made it.

use std::sync::OnceLock;
use std::time::Duration;

use bytes::Bytes;
use http_body_util::{BodyExt, Empty, Limited};
use hyper::{Request, StatusCode};
use hyper_util::client::legacy::Client;
use hyper_util::client::legacy::connect::HttpConnector;
use hyper_util::rt::TokioExecutor;

type Https = hyper_rustls::HttpsConnector<HttpConnector>;

fn client() -> &'static Client<Https, Empty<Bytes>> {
    static CLIENT: OnceLock<Client<Https, Empty<Bytes>>> = OnceLock::new();
    CLIENT.get_or_init(|| {
        // aws-lc-rs by name: with another provider compiled in (serenity brings ring), rustls has no
        // default to fall back on
        let builder = hyper_rustls::HttpsConnectorBuilder::new();
        let builder = match builder.with_provider_and_webpki_roots(rustls::crypto::aws_lc_rs::default_provider()) {
            Ok(builder) => builder,
            Err(_) => hyper_rustls::HttpsConnectorBuilder::new().with_webpki_roots(),
        };
        let connector = builder.https_or_http().enable_http1().build();
        Client::builder(TokioExecutor::new()).build(connector)
    })
}

/// A reply: its status, its headers and (up to `max_body` bytes) its body.
#[derive(Debug)]
pub struct Reply {
    pub status: StatusCode,
    pub headers: hyper::HeaderMap,
    pub body: Result<Bytes, String>,
}

/// GET `url` with `headers`, everything within `timeout`. Errors read like fetch's.
pub async fn get(url: &str, headers: &[(&str, &str)], timeout: Duration, max_body: usize) -> Result<Reply, String> {
    let work = async {
        let mut request = Request::get(url);
        for (name, value) in headers {
            request = request.header(*name, *value);
        }
        let request = request.body(Empty::<Bytes>::new()).map_err(|e| e.to_string())?;
        let response = client()
            .request(request)
            .await
            .map_err(|e| format!("fetch failed: {e}"))?;
        // What fetch follows (and `redirect: "error"` refuses); a 304 is an answer
        if matches!(response.status().as_u16(), 301 | 302 | 303 | 307 | 308) {
            return Err("fetch failed: unexpected redirect".to_owned());
        }
        let (parts, body) = response.into_parts();
        let body = Limited::new(body, max_body)
            .collect()
            .await
            .map(|b| b.to_bytes())
            .map_err(|e| e.to_string());
        Ok(Reply {
            status: parts.status,
            headers: parts.headers,
            body,
        })
    };
    match tokio::time::timeout(timeout, work).await {
        Ok(result) => result,
        Err(_) => Err("The operation was aborted due to timeout".to_owned()),
    }
}
