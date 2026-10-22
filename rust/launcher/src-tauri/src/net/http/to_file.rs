//! `requestToFile` (Reforged's `src/main/http.ts` 133–203): one GET streamed to a file, with the key
//! headers, an idle timeout and a size cap, for the runtime's two files (RD22: DR's `net/downloader.rs` is
//! not brought in). The same rules as `request()`: only the invite's host, pinned TLS in public mode,
//! plain HTTP only where `request()` allows it, no redirects. A file of its own so RA7 fills it while RA5
//! changes the rest of `net/http.rs`.

use std::path::Path;
use std::time::Duration;

use bytes::Bytes;
use http_body_util::{BodyExt, Empty};
use hyper::Request;
use hyper::header::HeaderValue;
use hyper_util::rt::TokioIo;
use tokio::io::{AsyncRead, AsyncWrite, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio_util::sync::CancellationToken;

use super::{Endpoint, HttpError, check_endpoint, host_header, pinned_target};
use crate::net::pinned::{ConnectError, pinned_connect};

pub struct FileRequestOptions<'a> {
    pub headers: Vec<(&'static str, String)>,
    /// No data for this long ends the request ("download stalled"); reset on the answer and every chunk
    pub idle_timeout: Duration,
    pub max_bytes: u64,
    pub cancel: Option<CancellationToken>,
    /// Hears the length of every chunk written
    pub on_bytes: Option<&'a mut (dyn FnMut(u64) + Send)>,
}

#[derive(Debug)]
pub enum FileRequestError {
    Http(HttpError),
    /// Writing the file failed (disk full, access denied, …)
    Disk(std::io::Error),
}

impl std::fmt::Display for FileRequestError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Http(error) => error.fmt(f),
            Self::Disk(error) => error.fmt(f),
        }
    }
}

impl std::error::Error for FileRequestError {}

impl From<HttpError> for FileRequestError {
    fn from(error: HttpError) -> Self {
        Self::Http(error)
    }
}

/// Streams `path_and_query` on `endpoint` into `target` and answers the HTTP status. The file holds the
/// body only for a 200; any other status writes nothing. A failure part way removes what was written.
pub async fn request_to_file(
    endpoint: &Endpoint,
    path_and_query: &str,
    target: &Path,
    options: FileRequestOptions<'_>,
) -> Result<u16, FileRequestError> {
    check_endpoint(endpoint)?;
    if !path_and_query.starts_with('/') {
        return Err(HttpError::BadUrl("path must start with /").into());
    }
    let mut builder = Request::builder().method(hyper::Method::GET).uri(path_and_query);
    let headers = builder.headers_mut().ok_or(HttpError::BadUrl("invalid request"))?;
    let value = |text: &str| HeaderValue::from_str(text).map_err(|_| HttpError::BadUrl("invalid header"));
    headers.insert("accept", HeaderValue::from_static("application/octet-stream"));
    headers.insert("host", value(&host_header(endpoint))?);
    for (name, text) in &options.headers {
        headers.insert(*name, value(text)?);
    }
    let request = builder
        .body(Empty::<Bytes>::new())
        .map_err(|_| HttpError::BadUrl("invalid request"))?;

    let cancel = options.cancel.clone().unwrap_or_default();
    if cancel.is_cancelled() {
        return Err(HttpError::Aborted.into());
    }
    let idle = options.idle_timeout;
    let mut wrote = false;
    let work = async {
        match pinned_target(endpoint) {
            Some(pinned) => {
                let stream = pinned_connect(&pinned, idle).await.map_err(connect_error)?;
                exchange(stream, request, target, options, &mut wrote).await
            }
            None => {
                let stream = tokio::time::timeout(idle, TcpStream::connect((endpoint.host.as_str(), endpoint.port)))
                    .await
                    .map_err(|_| HttpError::Timeout)?
                    .map_err(|error| HttpError::Network(error.to_string()))?;
                let _ = stream.set_nodelay(true);
                exchange(stream, request, target, options, &mut wrote).await
            }
        }
    };
    let result = tokio::select! {
        result = work => result,
        () = cancel.cancelled() => Err(HttpError::Aborted.into()),
    };
    if result.is_err() && wrote {
        let _ = tokio::fs::remove_file(target).await;
    }
    result
}

fn connect_error(error: ConnectError) -> HttpError {
    match error {
        ConnectError::Pin { actual } => HttpError::Pin { actual },
        ConnectError::Timeout => HttpError::Timeout,
        ConnectError::Io(error) => HttpError::Network(error.to_string()),
    }
}

async fn exchange<S>(
    stream: S,
    request: Request<Empty<Bytes>>,
    target: &Path,
    mut options: FileRequestOptions<'_>,
    wrote: &mut bool,
) -> Result<u16, FileRequestError>
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let idle = options.idle_timeout;
    let stalled = || HttpError::Timeout;
    let (mut sender, connection) =
        tokio::time::timeout(idle, hyper::client::conn::http1::handshake(TokioIo::new(stream)))
            .await
            .map_err(|_| stalled())?
            .map_err(|error| HttpError::Network(error.to_string()))?;
    let driver = tokio::spawn(connection);
    let result = async {
        let response = tokio::time::timeout(idle, sender.send_request(request))
            .await
            .map_err(|_| stalled())?
            .map_err(|error| HttpError::Network(error.to_string()))?;
        let status = response.status().as_u16();
        // Any other answer is dropped with its connection: nothing is written
        if status != 200 {
            return Ok(status);
        }
        let declared = response
            .headers()
            .get("content-length")
            .and_then(|value| value.to_str().ok())
            .and_then(|text| text.trim().parse::<u64>().ok());
        if declared.is_some_and(|length| length > options.max_bytes) {
            return Err(HttpError::TooLarge.into());
        }
        let mut file = tokio::fs::File::create(target).await.map_err(FileRequestError::Disk)?;
        *wrote = true;
        let mut body = response.into_body();
        let mut size = 0u64;
        loop {
            let Some(frame) = tokio::time::timeout(idle, body.frame()).await.map_err(|_| stalled())? else {
                break;
            };
            let frame = frame.map_err(|error| HttpError::Network(error.to_string()))?;
            let Some(chunk) = frame.data_ref() else { continue };
            size += chunk.len() as u64;
            if size > options.max_bytes {
                return Err(HttpError::TooLarge.into());
            }
            file.write_all(chunk).await.map_err(FileRequestError::Disk)?;
            if let Some(on_bytes) = options.on_bytes.as_mut() {
                on_bytes(chunk.len() as u64);
            }
        }
        file.flush().await.map_err(FileRequestError::Disk)?;
        Ok(200)
    }
    .await;
    driver.abort();
    result
}

#[cfg(test)]
mod tests {
    use std::convert::Infallible;

    use super::*;
    use crate::net::pinned::certificate_fingerprint;
    use crate::net::pinned::tests::{acceptor, certificate};
    use http_body_util::Full;
    use hyper::Response;
    use hyper::service::service_fn;
    use tokio::io::AsyncReadExt;
    use tokio::net::TcpListener;

    const DATA: &[u8] = b"runtime bytes";

    fn answer(request: Request<hyper::body::Incoming>) -> Response<Full<Bytes>> {
        let accept = request.headers().get("accept").cloned();
        let key = request.headers().get("x-test-key").cloned();
        match request.uri().path() {
            "/file" if accept.as_ref().is_some_and(|v| v == "application/octet-stream") && key.is_some() => {
                Response::new(Full::new(Bytes::from_static(DATA)))
            }
            "/file" => Response::builder()
                .status(401)
                .body(Full::default())
                .unwrap_or_default(),
            _ => Response::builder()
                .status(404)
                .body(Full::default())
                .unwrap_or_default(),
        }
    }

    async fn serve(tls: Option<tokio_rustls::TlsAcceptor>) -> u16 {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            loop {
                let Ok((tcp, _)) = listener.accept().await else { return };
                let tls = tls.clone();
                tokio::spawn(async move {
                    let service = service_fn(|request| async move { Ok::<_, Infallible>(answer(request)) });
                    let http = hyper::server::conn::http1::Builder::new();
                    match tls {
                        Some(tls) => {
                            if let Ok(stream) = tls.accept(tcp).await {
                                let _ = http.serve_connection(TokioIo::new(stream), service).await;
                            }
                        }
                        None => {
                            let _ = http.serve_connection(TokioIo::new(tcp), service).await;
                        }
                    }
                });
            }
        });
        port
    }

    fn options<'a>() -> FileRequestOptions<'a> {
        FileRequestOptions {
            headers: vec![("x-test-key", "k".into())],
            idle_timeout: Duration::from_secs(5),
            max_bytes: 1024,
            cancel: None,
            on_bytes: None,
        }
    }

    fn local(port: u16) -> Endpoint {
        Endpoint {
            host: "127.0.0.1".into(),
            port,
            pin: None,
        }
    }

    #[tokio::test]
    async fn a_200_is_written_and_counted() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("out.dll");
        let endpoint = local(serve(None).await);
        let mut seen = 0u64;
        let mut count = |n: u64| seen += n;
        let status = request_to_file(
            &endpoint,
            "/file",
            &target,
            FileRequestOptions {
                on_bytes: Some(&mut count),
                ..options()
            },
        )
        .await
        .unwrap();
        assert_eq!(status, 200);
        assert_eq!(std::fs::read(&target).unwrap(), DATA);
        assert_eq!(seen, DATA.len() as u64);
    }

    #[tokio::test]
    async fn another_status_writes_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("out.dll");
        let endpoint = local(serve(None).await);
        assert_eq!(
            request_to_file(&endpoint, "/nope", &target, options()).await.unwrap(),
            404
        );
        let keyless = FileRequestOptions {
            headers: Vec::new(),
            ..options()
        };
        assert_eq!(
            request_to_file(&endpoint, "/file", &target, keyless).await.unwrap(),
            401
        );
        assert!(!target.exists());
    }

    #[tokio::test]
    async fn too_large_by_header_never_opens_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("out.dll");
        let endpoint = local(serve(None).await);
        let small = FileRequestOptions {
            max_bytes: 4,
            ..options()
        };
        let error = request_to_file(&endpoint, "/file", &target, small).await.unwrap_err();
        assert!(matches!(error, FileRequestError::Http(HttpError::TooLarge)));
        assert!(!target.exists());
    }

    #[tokio::test]
    async fn pinned_tls_and_a_wrong_pin() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("out.dll");
        let (cert, key) = certificate();
        let fingerprint = certificate_fingerprint(&cert);
        let port = serve(Some(acceptor(cert, key))).await;
        let endpoint = Endpoint {
            host: "127.0.0.1".into(),
            port,
            pin: Some(fingerprint.clone()),
        };
        assert_eq!(
            request_to_file(&endpoint, "/file", &target, options()).await.unwrap(),
            200
        );
        assert_eq!(std::fs::read(&target).unwrap(), DATA);
        std::fs::remove_file(&target).unwrap();
        let wrong = Endpoint {
            pin: Some("0".repeat(64)),
            ..endpoint
        };
        let error = request_to_file(&wrong, "/file", &target, options()).await.unwrap_err();
        assert!(matches!(error, FileRequestError::Http(HttpError::Pin { actual: Some(seen) }) if seen == fingerprint));
        assert!(!target.exists());
    }

    #[tokio::test]
    async fn plain_http_only_where_request_allows_it() {
        let target = Path::new("never-written.dll");
        let public = Endpoint {
            host: "203.0.113.9".into(),
            port: 443,
            pin: None,
        };
        let error = request_to_file(&public, "/file", target, options()).await.unwrap_err();
        assert!(matches!(error, FileRequestError::Http(HttpError::BadUrl(_))));
        let error = request_to_file(&local(1), "file", target, options()).await.unwrap_err();
        assert!(matches!(error, FileRequestError::Http(HttpError::BadUrl(_))));
    }

    /// A server that sends the head and part of the body, then goes quiet or hangs up.
    async fn half_server(hang_up: bool) -> u16 {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            while let Ok((mut tcp, _)) = listener.accept().await {
                tokio::spawn(async move {
                    let mut buffer = [0u8; 2048];
                    let _ = tcp.read(&mut buffer).await;
                    let _ = tcp
                        .write_all(b"HTTP/1.1 200 OK\r\ncontent-length: 100\r\n\r\nhalf of it")
                        .await;
                    if hang_up {
                        return;
                    }
                    tokio::time::sleep(Duration::from_secs(30)).await;
                });
            }
        });
        port
    }

    #[tokio::test]
    async fn a_stalled_or_cut_download_leaves_no_file() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("out.dll");
        let quick = FileRequestOptions {
            idle_timeout: Duration::from_millis(300),
            ..options()
        };
        let stalled = local(half_server(false).await);
        let error = request_to_file(&stalled, "/file", &target, quick).await.unwrap_err();
        assert!(matches!(error, FileRequestError::Http(HttpError::Timeout)), "{error}");
        assert!(!target.exists());
        let cut = local(half_server(true).await);
        let error = request_to_file(&cut, "/file", &target, options()).await.unwrap_err();
        assert!(
            matches!(error, FileRequestError::Http(HttpError::Network(_))),
            "{error}"
        );
        assert!(!target.exists());
    }

    #[tokio::test]
    async fn a_cancelled_download_stops() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("out.dll");
        let cancel = CancellationToken::new();
        let stalled = local(half_server(false).await);
        let stop = cancel.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(200)).await;
            stop.cancel();
        });
        let cancelled = FileRequestOptions {
            cancel: Some(cancel),
            ..options()
        };
        let error = request_to_file(&stalled, "/file", &target, cancelled)
            .await
            .unwrap_err();
        assert!(matches!(error, FileRequestError::Http(HttpError::Aborted)));
        assert!(!target.exists());
    }
}
