//! TLS connections to a public-mode server, pinned to the certificate fingerprint from the invite (row
//! L19): `src/main/pinned.ts`. The server's certificate is self-signed, so there is no CA to check;
//! the SHA-256 of the certificate it presents (DER) must be the invite's fingerprint, or the handshake
//! fails and nothing, not even a request line, is written. The handshake signature is still verified
//! with that certificate's key, so a copy of the certificate without its key does not pass.

use std::net::IpAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::CryptoProvider;
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{DigitallySignedStruct, SignatureScheme};
use sha2::{Digest, Sha256};
use tokio::net::TcpStream;
use tokio_rustls::TlsConnector;
use tokio_rustls::client::TlsStream;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PinnedTarget {
    pub host: String,
    pub port: u16,
    /// 64 lower-case hex
    pub fingerprint: String,
}

#[derive(Debug)]
pub enum ConnectError {
    /// The certificate is not the invite's; the fingerprint it had, if the server showed one
    Pin {
        actual: Option<String>,
    },
    Timeout,
    Io(std::io::Error),
}

impl std::fmt::Display for ConnectError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Pin { .. } => f.write_str("the server's certificate does not match the invite"),
            Self::Timeout => f.write_str("connection timed out"),
            Self::Io(error) => write!(f, "{error}"),
        }
    }
}

impl std::error::Error for ConnectError {}

pub fn certificate_fingerprint(der: &[u8]) -> String {
    Sha256::digest(der).iter().map(|byte| format!("{byte:02x}")).collect()
}

/// Accepts exactly the pinned certificate and records the one it saw.
#[derive(Debug)]
struct Pin {
    fingerprint: String,
    seen: Mutex<Option<String>>,
    provider: Arc<CryptoProvider>,
}

impl ServerCertVerifier for Pin {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        let actual = certificate_fingerprint(end_entity);
        let matches = actual == self.fingerprint;
        *self.seen.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(actual);
        if matches {
            Ok(ServerCertVerified::assertion())
        } else {
            Err(rustls::Error::InvalidCertificate(
                rustls::CertificateError::ApplicationVerificationFailure,
            ))
        }
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(message, cert, dss, &self.provider.signature_verification_algorithms)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(message, cert, dss, &self.provider.signature_verification_algorithms)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.provider.signature_verification_algorithms.supported_schemes()
    }
}

fn connector(pin: Arc<Pin>) -> Result<TlsConnector, ConnectError> {
    let mut config = rustls::ClientConfig::builder_with_provider(pin.provider.clone())
        .with_safe_default_protocol_versions()
        .map_err(|error| ConnectError::Io(std::io::Error::other(error)))?
        .dangerous()
        .with_custom_certificate_verifier(pin)
        .with_no_client_auth();
    config.alpn_protocols = vec![b"http/1.1".to_vec()];
    Ok(TlsConnector::from(Arc::new(config)))
}

/// A TLS connection whose certificate is the pinned one. SNI only for names: an IP address is never
/// sent as a server name.
pub async fn pinned_connect(target: &PinnedTarget, timeout: Duration) -> Result<TlsStream<TcpStream>, ConnectError> {
    let pin = Arc::new(Pin {
        fingerprint: target.fingerprint.clone(),
        seen: Mutex::new(None),
        provider: Arc::new(rustls::crypto::aws_lc_rs::default_provider()),
    });
    let connector = connector(pin.clone())?;
    let name = match target.host.parse::<IpAddr>() {
        Ok(ip) => ServerName::IpAddress(ip.into()),
        Err(_) => {
            ServerName::try_from(target.host.clone()).map_err(|error| ConnectError::Io(std::io::Error::other(error)))?
        }
    };
    let connect = async {
        let tcp = TcpStream::connect((target.host.as_str(), target.port))
            .await
            .map_err(ConnectError::Io)?;
        let _ = tcp.set_nodelay(true);
        connector.connect(name, tcp).await.map_err(|error| {
            let seen = pin.seen.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).clone();
            match &seen {
                Some(actual) if *actual != target.fingerprint => ConnectError::Pin { actual: seen },
                _ => ConnectError::Io(error),
            }
        })
    };
    tokio::time::timeout(timeout, connect)
        .await
        .map_err(|_| ConnectError::Timeout)?
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;
    use tokio_rustls::TlsAcceptor;

    /// A self-signed certificate for 127.0.0.1 and localhost: (certificate DER, key DER).
    pub fn certificate() -> (CertificateDer<'static>, rustls::pki_types::PrivateKeyDer<'static>) {
        let cert = rcgen::generate_simple_self_signed(vec!["127.0.0.1".into(), "localhost".into()]).unwrap();
        let key = rustls::pki_types::PrivateKeyDer::Pkcs8(cert.signing_key.serialize_der().into());
        (cert.cert.der().clone(), key)
    }

    pub fn acceptor(cert: CertificateDer<'static>, key: rustls::pki_types::PrivateKeyDer<'static>) -> TlsAcceptor {
        let config =
            rustls::ServerConfig::builder_with_provider(Arc::new(rustls::crypto::aws_lc_rs::default_provider()))
                .with_safe_default_protocol_versions()
                .unwrap()
                .with_no_client_auth()
                .with_single_cert(vec![cert], key)
                .unwrap();
        TlsAcceptor::from(Arc::new(config))
    }

    /// A TLS server that writes "hello" and records the first bytes it received.
    async fn server(acceptor: TlsAcceptor) -> (u16, tokio::sync::oneshot::Receiver<Vec<u8>>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let (tx, rx) = tokio::sync::oneshot::channel();
        tokio::spawn(async move {
            let (tcp, _) = listener.accept().await.unwrap();
            let mut received = Vec::new();
            if let Ok(mut tls) = acceptor.accept(tcp).await {
                let _ = tls.write_all(b"hello").await;
                let mut buffer = [0; 64];
                if let Ok(n) = tls.read(&mut buffer).await {
                    received.extend_from_slice(&buffer[..n]);
                }
            }
            let _ = tx.send(received);
        });
        (port, rx)
    }

    #[tokio::test]
    async fn the_right_certificate_connects() {
        let (cert, key) = certificate();
        let fingerprint = certificate_fingerprint(&cert);
        let (port, received) = server(acceptor(cert, key)).await;
        let target = PinnedTarget {
            host: "127.0.0.1".into(),
            port,
            fingerprint,
        };
        let mut tls = pinned_connect(&target, Duration::from_secs(5)).await.unwrap();
        let mut hello = [0; 5];
        tls.read_exact(&mut hello).await.unwrap();
        assert_eq!(&hello, b"hello");
        tls.write_all(b"GET").await.unwrap();
        tls.flush().await.unwrap();
        assert_eq!(received.await.unwrap(), b"GET");
    }

    #[tokio::test]
    async fn a_wrong_or_rotated_certificate_is_refused_before_anything_is_sent() {
        let (cert, key) = certificate();
        let actual = certificate_fingerprint(&cert);
        let (old, _) = certificate();
        let (port, received) = server(acceptor(cert, key)).await;
        let target = PinnedTarget {
            host: "localhost".into(),
            port,
            fingerprint: certificate_fingerprint(&old),
        };
        match pinned_connect(&target, Duration::from_secs(5)).await {
            Err(ConnectError::Pin { actual: seen }) => assert_eq!(seen.as_deref(), Some(actual.as_str())),
            other => panic!("expected a pin error, got {other:?}"),
        }
        assert!(received.await.unwrap_or_default().is_empty());
    }

    #[tokio::test]
    async fn no_answer_times_out() {
        // A listener that never finishes the handshake
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let target = PinnedTarget {
            host: "127.0.0.1".into(),
            port,
            fingerprint: "0".repeat(64),
        };
        let result = pinned_connect(&target, Duration::from_millis(200)).await;
        assert!(matches!(result, Err(ConnectError::Timeout)), "{result:?}");
        drop(listener);
    }
}
