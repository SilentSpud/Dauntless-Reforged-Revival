//! Invites (`legacy/ReforgedHost/src/invite.ts`): a code from the backend (its local admin route)
//! wrapped in the link the launcher reads. The link format is `protocol`'s (section 6, RS1):
//!
//! - private: `reforged-revival://join?v=1&host=<Tailscale IP>&port=61000&code=..&name=..[&share=..]`
//! - public: `reforged-revival://join?v=2&mode=public&host=<IP or DNS>&port=443&fp=<cert SHA-256>&code=..&name=..`
//!
//! Also the gateway certificate's fingerprint (`setup`, `doctor`), the one a port serves (`doctor`)
//! and the small HTTP/1.1 client the admin routes and `doctor`'s checks use.

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream, ToSocketAddrs};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use protocol::invite::{FormatError, Invite, ServerMode as InviteMode, format_invite};
use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};

use crate::error::HostError;
use crate::hostconfig::{HostConfig, ServerMode, gateway_port};

pub use protocol::invite::INVITE_SCHEME;

/// What a link is made of, as `formatInvite` takes it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InviteParts {
    pub mode: ServerMode,
    pub host: String,
    pub port: u16,
    pub code: String,
    pub name: String,
    pub fp: Option<String>,
    pub share: Option<String>,
}

pub fn format_invite_parts(i: &InviteParts) -> Result<String, HostError> {
    let invite = Invite {
        mode: match i.mode {
            ServerMode::Private => InviteMode::Private,
            ServerMode::Public => InviteMode::Public,
        },
        host: i.host.clone(),
        port: i.port,
        code: i.code.clone(),
        name: i.name.clone(),
        share: i.share.clone(),
        fp: i.fp.clone(),
    };
    format_invite(&invite).map_err(|e| match e {
        FormatError::Fingerprint => HostError::message("public invites need the certificate fingerprint"),
    })
}

pub fn invite_for(c: &HostConfig, code: &str, fp: Option<&str>) -> Result<String, HostError> {
    format_invite_parts(&InviteParts {
        mode: c.mode,
        host: c.host.clone(),
        port: gateway_port(c),
        code: code.to_owned(),
        name: c.name.clone(),
        fp: if c.mode == ServerMode::Public {
            fp.map(str::to_owned)
        } else {
            None
        },
        share: if c.mode == ServerMode::Private {
            c.share_url.clone()
        } else {
            None
        },
    })
}

fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect()
}

/// The SHA-256 of the gateway certificate's DER bytes, as every invite pins it (`fp`).
pub fn certificate_fingerprint(cert_file: &Path) -> Result<String, HostError> {
    let pem = std::fs::read(cert_file).map_err(|e| HostError::io(cert_file, e))?;
    let cert = CertificateDer::from_pem_slice(&pem)
        .map_err(|e| HostError::message(format!("{}: not a PEM certificate ({e})", cert_file.display())))?;
    Ok(sha256_hex(&cert))
}

/// Takes any certificate: `doctor` only reads which one a port presents, it sends nothing.
#[derive(Debug)]
struct AcceptAny(Arc<rustls::crypto::CryptoProvider>);

impl rustls::client::danger::ServerCertVerifier for AcceptAny {
    fn verify_server_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        Ok(rustls::client::danger::ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
    }

    fn verify_tls13_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        self.0.signature_verification_algorithms.supported_schemes()
    }
}

fn connect(host: &str, port: u16, timeout: Duration) -> std::io::Result<TcpStream> {
    let addresses: Vec<SocketAddr> = (host, port).to_socket_addrs()?.collect();
    let mut last = std::io::Error::new(std::io::ErrorKind::NotFound, format!("no address for {host}"));
    for address in addresses {
        match TcpStream::connect_timeout(&address, timeout) {
            Ok(stream) => {
                stream.set_read_timeout(Some(timeout))?;
                stream.set_write_timeout(Some(timeout))?;
                return Ok(stream);
            }
            Err(e) => last = e,
        }
    }
    Err(last)
}

/// The SHA-256 of the certificate whatever listens on host:port presents (the launcher pins it), or
/// `None` when nothing answers with TLS.
pub fn served_fingerprint(port: u16, host: &str, timeout: Duration) -> Option<String> {
    let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
    let config = rustls::ClientConfig::builder_with_provider(provider.clone())
        .with_safe_default_protocol_versions()
        .ok()?
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(AcceptAny(provider)))
        .with_no_client_auth();
    // Node's tls.connect sends no SNI for an IP address either.
    let name = ServerName::try_from(host.to_owned()).ok()?;
    let mut tls = rustls::ClientConnection::new(Arc::new(config), name).ok()?;
    let mut socket = connect(host, port, timeout).ok()?;
    while tls.is_handshaking() {
        tls.complete_io(&mut socket).ok()?;
    }
    tls.peer_certificates()
        .and_then(|chain| chain.first())
        .map(|cert| sha256_hex(cert))
}

/// An HTTP/1.1 answer: the status and the body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct HttpReply {
    pub status: u16,
    pub body: Vec<u8>,
}

/// One request to `host:port` with `Connection: close`, as Node's `http.request` with `agent: false`.
pub(crate) fn http_request(
    host: &str,
    port: u16,
    method: &str,
    path: &str,
    headers: &[(&str, &str)],
    body: Option<&[u8]>,
    timeout: Duration,
) -> std::io::Result<HttpReply> {
    let mut stream = connect(host, port, timeout)?;
    let mut head = format!("{method} {path} HTTP/1.1\r\nHost: {host}:{port}\r\nConnection: close\r\n");
    for (name, value) in headers {
        head += &format!("{name}: {value}\r\n");
    }
    if let Some(body) = body {
        head += &format!("Content-Length: {}\r\n", body.len());
    }
    head += "\r\n";
    stream.write_all(head.as_bytes())?;
    if let Some(body) = body {
        stream.write_all(body)?;
    }
    let mut raw = Vec::new();
    stream.read_to_end(&mut raw)?;
    parse_reply(&raw).ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidData, "not an HTTP answer"))
}

fn parse_reply(raw: &[u8]) -> Option<HttpReply> {
    let end = raw.windows(4).position(|w| w == b"\r\n\r\n")?;
    let head = std::str::from_utf8(&raw[..end]).ok()?;
    let mut lines = head.split("\r\n");
    let status = lines.next()?.split(' ').nth(1)?.parse().ok()?;
    let chunked = lines.any(|line| {
        let lower = line.to_ascii_lowercase();
        lower.starts_with("transfer-encoding:") && lower.contains("chunked")
    });
    let rest = &raw[end + 4..];
    let body = if chunked { unchunk(rest)? } else { rest.to_vec() };
    Some(HttpReply { status, body })
}

fn unchunk(mut rest: &[u8]) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    loop {
        let line_end = rest.windows(2).position(|w| w == b"\r\n")?;
        let size_text = std::str::from_utf8(&rest[..line_end]).ok()?;
        let size = usize::from_str_radix(size_text.split(';').next()?.trim(), 16).ok()?;
        rest = &rest[line_end + 2..];
        if size == 0 {
            return Some(out);
        }
        out.extend_from_slice(rest.get(..size)?);
        rest = rest.get(size + 2..)?;
    }
}

const ADMIN_TIMEOUT: Duration = Duration::from_secs(10);

fn backend_request(
    method: &str,
    path: &str,
    admin_key: &str,
    body: Option<&Value>,
    port: u16,
) -> std::io::Result<(u16, Value)> {
    let data = body.map(|b| service_kit::jsjson::stringify(b).into_bytes());
    let mut headers = vec![("x-reforged-admin-key", admin_key)];
    if data.is_some() {
        headers.push(("content-type", "application/json"));
    }
    let reply = http_request(
        "127.0.0.1",
        port,
        method,
        path,
        &headers,
        data.as_deref(),
        ADMIN_TIMEOUT,
    )?;
    let json = std::str::from_utf8(&reply.body)
        .ok()
        .and_then(|text| service_kit::jsjson::parse(text).ok())
        .unwrap_or(Value::Null);
    Ok((reply.status, json))
}

/// Who the code is for and how often it can be used.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CodeOptions {
    pub uses: f64,
    pub infinite: bool,
    pub note: Option<String>,
}

/// Asks the running backend for a new invite code (admin routes answer only on this machine).
pub fn create_invite_code(admin_key: &str, options: &CodeOptions, port: u16) -> Result<String, HostError> {
    let mut body = Map::new();
    body.insert("uses".into(), service_kit::jsjson::to_js(&json!(options.uses)));
    body.insert("infinite".into(), options.infinite.into());
    if let Some(note) = options.note.as_deref().filter(|n| !n.is_empty()) {
        body.insert("name".into(), note.into());
    }
    let (status, json) = backend_request(
        "POST",
        "/reforged/api/CreateInvite",
        admin_key,
        Some(&Value::Object(body)),
        port,
    )
    .map_err(|e| {
        HostError::message(format!(
            "the backend is not running on 127.0.0.1:{port} (start the server with reforged-host start first): {e}"
        ))
    })?;
    match json.get("code").and_then(Value::as_str) {
        Some(code) if status == 200 => Ok(code.to_owned()),
        _ if status == 403 => Err(HostError::message(
            "the backend refused the admin key (run reforged-host setup again so its env file and secrets.env agree)",
        )),
        _ => Err(HostError::message(format!("the backend answered {status}"))),
    }
}

pub fn list_invite_codes(admin_key: &str, port: u16) -> Result<Value, HostError> {
    let (status, json) = backend_request("GET", "/reforged/api/InviteCodes", admin_key, None, port)
        .map_err(|e| HostError::message(e.to_string()))?;
    if status != 200 {
        return Err(HostError::message(format!("the backend answered {status}")));
    }
    Ok(json)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;

    #[test]
    fn reads_a_chunked_answer() {
        let raw = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n4\r\n{\"a\"\r\n3\r\n:1}\r\n0\r\n\r\n";
        assert_eq!(
            parse_reply(raw),
            Some(HttpReply {
                status: 200,
                body: b"{\"a\":1}".to_vec()
            })
        );
        assert_eq!(parse_reply(b"garbage"), None);
    }

    #[test]
    fn nothing_listening_has_no_fingerprint() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);
        assert_eq!(served_fingerprint(port, "127.0.0.1", Duration::from_millis(500)), None);
    }
}
