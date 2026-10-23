//! The gateway's TLS certificate. An installed server keeps the certificate and key it has: their
//! SHA-256 fingerprint is the `fp` of every invite already sent, so the gateway only ever loads them
//! (`src/server.ts`). `make-cert` (`tools/make-cert.js`) makes a new pair for a new install.

use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use chrono::{DateTime, Utc};
use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use serde_json::json;
use sha2::{Digest, Sha256};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CertInfo {
    /// SHA-256 of the certificate's DER encoding, 64 lowercase hex: the "fp" of a v2 invite
    pub fingerprint: String,
    pub not_before: DateTime<Utc>,
    pub not_after: DateTime<Utc>,
    /// As Node's `X509Certificate.subjectAltName` writes it: "IP Address:203.0.113.7, DNS:a.example"
    pub subject_alt_name: String,
}

/// `Date.prototype.toISOString`.
pub fn iso(when: DateTime<Utc>) -> String {
    when.format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string()
}

fn san_entry(name: &x509_parser::extensions::GeneralName<'_>) -> Option<String> {
    use x509_parser::extensions::GeneralName;
    match name {
        GeneralName::DNSName(dns) => Some(format!("DNS:{dns}")),
        GeneralName::IPAddress(bytes) => match bytes.len() {
            4 => Some(format!(
                "IP Address:{}.{}.{}.{}",
                bytes[0], bytes[1], bytes[2], bytes[3]
            )),
            // OpenSSL's spelling: eight upper-case groups, nothing folded
            16 => Some(format!(
                "IP Address:{}",
                bytes
                    .chunks(2)
                    .map(|w| format!("{:X}", u16::from(w[0]) << 8 | u16::from(w[1])))
                    .collect::<Vec<_>>()
                    .join(":")
            )),
            _ => None,
        },
        GeneralName::RFC822Name(mail) => Some(format!("email:{mail}")),
        GeneralName::URI(uri) => Some(format!("URI:{uri}")),
        _ => None,
    }
}

/// `DescribeCertificate`.
pub fn describe(der: &[u8]) -> Result<CertInfo, String> {
    let (_, cert) = x509_parser::parse_x509_certificate(der).map_err(|e| format!("not a certificate: {e}"))?;
    let time = |t: x509_parser::time::ASN1Time| {
        DateTime::from_timestamp(t.timestamp(), 0).ok_or_else(|| "certificate date out of range".to_owned())
    };
    let names = cert
        .subject_alternative_name()
        .ok()
        .flatten()
        .map(|san| {
            san.value
                .general_names
                .iter()
                .filter_map(san_entry)
                .collect::<Vec<_>>()
                .join(", ")
        })
        .unwrap_or_default();
    Ok(CertInfo {
        fingerprint: Sha256::digest(der).iter().map(|b| format!("{b:02x}")).collect(),
        not_before: time(cert.validity().not_before)?,
        not_after: time(cert.validity().not_after)?,
        subject_alt_name: names,
    })
}

pub struct Loaded {
    pub chain: Vec<CertificateDer<'static>>,
    pub key: PrivateKeyDer<'static>,
    pub info: CertInfo,
}

/// Reads the PEM certificate (chain) and key, and makes sure the key belongs to the certificate.
pub fn load(cert_file: &Path, key_file: &Path) -> Result<Loaded, String> {
    let chain: Vec<CertificateDer<'static>> = CertificateDer::pem_file_iter(cert_file)
        .map_err(|e| e.to_string())?
        .collect::<Result<_, _>>()
        .map_err(|e| e.to_string())?;
    let Some(first) = chain.first() else {
        return Err("no certificate in the file".into());
    };
    let key = PrivateKeyDer::from_pem_file(key_file).map_err(|e| e.to_string())?;
    let provider = rustls::crypto::aws_lc_rs::default_provider();
    let signing = provider
        .key_provider
        .load_private_key(key.clone_key())
        .map_err(|e| e.to_string())?;
    rustls::sign::CertifiedKey::new(chain.clone(), signing)
        .keys_match()
        .map_err(|_| "the key does not belong to the certificate".to_owned())?;
    let info = describe(first)?;
    Ok(Loaded { chain, key, info })
}

/// TLS 1.2 and 1.3, HTTP/1.1 only.
pub fn server_config(loaded: &Loaded) -> Result<Arc<rustls::ServerConfig>, String> {
    let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
    let mut config = rustls::ServerConfig::builder_with_provider(provider)
        .with_protocol_versions(&[&rustls::version::TLS13, &rustls::version::TLS12])
        .map_err(|e| e.to_string())?
        .with_no_client_auth()
        .with_single_cert(loaded.chain.clone(), loaded.key.clone_key())
        .map_err(|e| e.to_string())?;
    config.alpn_protocols = vec![b"http/1.1".to_vec()];
    Ok(Arc::new(config))
}

// ---- make-cert ----

const VALID_YEARS: i32 = 10;

#[derive(Debug, Default, PartialEq, Eq)]
pub struct MakeCertArgs {
    pub hosts: Vec<String>,
    pub out: Option<PathBuf>,
    pub cert: Option<PathBuf>,
    pub key: Option<PathBuf>,
    pub force: bool,
    pub json: bool,
    pub fingerprint: Option<PathBuf>,
}

const USAGE: &str = "usage: gateway make-cert --host <public IP or DNS name> [--host ...] (--out <dir> | --cert <file> --key <file>) [--force] [--json]\n       gateway make-cert --fingerprint <cert.pem> [--json]\n\n  gateway make-cert --host 203.0.113.7 [--host play.example.org] --out C:\\ReforgedServer\\tls\n  gateway make-cert --fingerprint C:\\ReforgedServer\\tls\\gateway-cert.pem";

pub fn parse_make_cert_args(args: &[String]) -> Result<MakeCertArgs, String> {
    let mut out = MakeCertArgs::default();
    let mut index = 0;
    while index < args.len() {
        let arg = args[index].as_str();
        let mut next = || {
            index += 1;
            match args.get(index) {
                Some(value) if !value.starts_with("--") => Ok(value.clone()),
                _ => Err(format!("{arg} needs a value")),
            }
        };
        match arg {
            "--host" => out.hosts.push(next()?),
            "--out" => out.out = Some(next()?.into()),
            "--cert" => out.cert = Some(next()?.into()),
            "--key" => out.key = Some(next()?.into()),
            "--fingerprint" => out.fingerprint = Some(next()?.into()),
            "--force" => out.force = true,
            "--json" => out.json = true,
            "--help" | "-h" => return Err(String::new()),
            other => return Err(format!("unknown argument {}", json!(other))),
        }
        index += 1;
    }
    Ok(out)
}

/// A DNS name the way a certificate may carry it: letters, digits and hyphens in dot-separated
/// labels, no wildcard, no trailing dot.
pub fn is_dns_name(name: &str) -> bool {
    if name.len() > 253 || !name.bytes().any(|b| b.is_ascii_alphabetic()) {
        return false;
    }
    name.split('.').all(|label| {
        let bytes = label.as_bytes();
        (1..=63).contains(&bytes.len())
            && bytes.iter().all(|b| b.is_ascii_alphanumeric() || *b == b'-')
            && bytes[0] != b'-'
            && bytes[bytes.len() - 1] != b'-'
    })
}

/// SubjectAltName entries: IP addresses and DNS names, each once.
pub fn alt_names(hosts: &[String]) -> Result<Vec<rcgen::SanType>, String> {
    if hosts.is_empty() {
        return Err("give at least one --host (the server's public IP address or DNS name)".into());
    }
    let mut seen = std::collections::HashSet::new();
    let mut names = Vec::new();
    for raw in hosts {
        let host = raw.trim().to_lowercase();
        if !seen.insert(host.clone()) {
            continue;
        }
        if host.contains('%') && host.split('%').next().is_some_and(|h| h.parse::<IpAddr>().is_ok()) {
            return Err(format!("{raw}: no zone ids"));
        }
        if let Ok(ip) = host.parse::<IpAddr>() {
            names.push(rcgen::SanType::IpAddress(ip));
        } else if is_dns_name(&host) {
            let name = host.clone().try_into().map_err(|_| format!("{raw}: not a DNS name"))?;
            names.push(rcgen::SanType::DnsName(name));
        } else {
            return Err(format!("{} is neither an IP address nor a DNS name", json!(raw)));
        }
    }
    Ok(names)
}

/// A self-signed RSA-2048, SHA-256 certificate valid for ten years (a day of slack before now for
/// clocks that run behind). Returns the certificate and PKCS#8 key as PEM.
pub fn make_certificate(names: Vec<rcgen::SanType>) -> Result<(String, String), String> {
    use rcgen::{
        CertificateParams, DnType, ExtendedKeyUsagePurpose, IsCa, KeyPair, KeyUsagePurpose, PKCS_RSA_SHA256, SanType,
        SerialNumber,
    };
    let key = KeyPair::generate_for(&PKCS_RSA_SHA256).map_err(|e| e.to_string())?;
    let mut params = CertificateParams::default();
    // 16 random bytes, positive
    let mut serial = [0u8; 16];
    rustls::crypto::aws_lc_rs::default_provider()
        .secure_random
        .fill(&mut serial)
        .map_err(|_| "no random numbers".to_owned())?;
    serial[0] = (serial[0] & 0x7f) | 0x01;
    params.serial_number = Some(SerialNumber::from_slice(&serial));
    let now = Utc::now().timestamp() - 24 * 3600;
    let not_before = time::OffsetDateTime::from_unix_timestamp(now).map_err(|e| e.to_string())?;
    let not_after = not_before
        .replace_year(not_before.year() + VALID_YEARS)
        .map_err(|e| e.to_string())?;
    params.not_before = not_before;
    params.not_after = not_after;
    let first = match &names[0] {
        SanType::IpAddress(ip) => ip.to_string(),
        SanType::DnsName(name) => name.to_string(),
        _ => String::new(),
    };
    params.distinguished_name.push(DnType::CommonName, first);
    params
        .distinguished_name
        .push(DnType::OrganizationName, "Dauntless Reforged Revival server");
    params.is_ca = IsCa::ExplicitNoCa;
    params.key_usages = vec![KeyUsagePurpose::DigitalSignature, KeyUsagePurpose::KeyEncipherment];
    params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
    params.subject_alt_names = names;
    let cert = params.self_signed(&key).map_err(|e| e.to_string())?;
    Ok((cert.pem(), key.serialize_pem()))
}

/// `gateway make-cert ...`; the process exit code.
pub fn make_cert_main(args: &[String]) -> i32 {
    let usage = |message: &str| {
        if !message.is_empty() {
            eprintln!("make-cert: {message}");
        }
        eprintln!("{USAGE}");
        2
    };
    let args = match parse_make_cert_args(args) {
        Ok(args) => args,
        Err(message) => return usage(&message),
    };
    let absolute = |p: &Path| std::path::absolute(p).unwrap_or_else(|_| p.to_path_buf());
    let report = |info: &CertInfo| {
        let mut fields = serde_json::Map::new();
        fields.insert("fingerprint".into(), json!(info.fingerprint));
        fields.insert("notBefore".into(), json!(iso(info.not_before)));
        fields.insert("notAfter".into(), json!(iso(info.not_after)));
        fields.insert("names".into(), json!(info.subject_alt_name));
        fields
    };
    if let Some(file) = &args.fingerprint {
        let info = std::fs::read(file).map_err(|e| e.to_string()).and_then(|pem| {
            let der = CertificateDer::from_pem_slice(&pem).map_err(|e| e.to_string())?;
            describe(&der)
        });
        let info = match info {
            Ok(info) => info,
            Err(error) => return usage(&format!("cannot read {}: {error}", file.display())),
        };
        if args.json {
            let mut out = serde_json::Map::new();
            out.insert("cert".into(), json!(absolute(file).display().to_string()));
            out.extend(report(&info));
            println!("{}", serde_json::Value::Object(out));
        } else {
            println!(
                "Names:       {}\nValid:       {} to {}\nFingerprint: {}",
                info.subject_alt_name,
                iso(info.not_before),
                iso(info.not_after),
                info.fingerprint
            );
        }
        return 0;
    }
    let names = match alt_names(&args.hosts) {
        Ok(names) => names,
        Err(message) => return usage(&message),
    };
    let (cert_file, key_file) = match (&args.out, &args.cert, &args.key) {
        (Some(_), Some(_), _) | (Some(_), _, Some(_)) => return usage("use either --out or --cert/--key"),
        (Some(out), None, None) => (
            absolute(&out.join("gateway-cert.pem")),
            absolute(&out.join("gateway-key.pem")),
        ),
        (None, Some(cert), Some(key)) => (absolute(cert), absolute(key)),
        _ => return usage("say where to write: --out <dir>, or --cert <file> and --key <file>"),
    };
    if !args.force {
        for file in [&cert_file, &key_file] {
            if file.exists() {
                return usage(&format!(
                    "{} exists. A new certificate changes the fingerprint and breaks every invite already sent; add --force if that is what you want",
                    file.display()
                ));
            }
        }
    }
    let written = make_certificate(names).and_then(|(cert_pem, key_pem)| {
        for file in [&cert_file, &key_file] {
            if let Some(dir) = file.parent() {
                std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
            }
        }
        write_private(&key_file, key_pem.as_bytes())?;
        std::fs::write(&cert_file, cert_pem.as_bytes()).map_err(|e| e.to_string())?;
        let der = CertificateDer::from_pem_slice(cert_pem.as_bytes()).map_err(|e| e.to_string())?;
        describe(&der)
    });
    let info = match written {
        Ok(info) => info,
        Err(error) => {
            eprintln!("make-cert: {error}");
            return 1;
        }
    };
    if args.json {
        let mut out = serde_json::Map::new();
        out.insert("cert".into(), json!(cert_file.display().to_string()));
        out.insert("key".into(), json!(key_file.display().to_string()));
        out.extend(report(&info));
        println!("{}", serde_json::Value::Object(out));
        return 0;
    }
    println!(
        "Certificate: {}\nKey:         {}  (keep private)\nNames:       {}\nValid:       {} to {}\nFingerprint: {}\n\nInvites for this server carry: fp={}",
        cert_file.display(),
        key_file.display(),
        info.subject_alt_name,
        iso(info.not_before),
        iso(info.not_after),
        info.fingerprint,
        info.fingerprint
    );
    0
}

/// Mode 0600 where the file system has modes.
fn write_private(file: &Path, data: &[u8]) -> Result<(), String> {
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    use std::io::Write;
    let mut handle = options.open(file).map_err(|e| e.to_string())?;
    handle.write_all(data).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_new_certificate_loads_and_describes_itself() {
        let names = alt_names(&[
            "127.0.0.1".into(),
            "Play.Example.org".into(),
            "2001:db8::1".into(),
            "127.0.0.1".into(),
        ])
        .unwrap();
        let (cert_pem, key_pem) = make_certificate(names).unwrap();
        let dir = tempfile::tempdir().unwrap();
        let (cert_file, key_file) = (dir.path().join("c.pem"), dir.path().join("k.pem"));
        std::fs::write(&cert_file, &cert_pem).unwrap();
        std::fs::write(&key_file, &key_pem).unwrap();
        let loaded = load(&cert_file, &key_file).unwrap();
        assert_eq!(loaded.info.fingerprint.len(), 64);
        assert_eq!(
            loaded.info.subject_alt_name,
            "IP Address:127.0.0.1, DNS:play.example.org, IP Address:2001:DB8:0:0:0:0:0:1"
        );
        let now = Utc::now();
        assert!(loaded.info.not_before < now && loaded.info.not_after > now + chrono::Duration::days(3600));
        assert!(key_pem.contains("BEGIN PRIVATE KEY"));
        server_config(&loaded).unwrap();

        // Another key: refused
        let (_, other_key) = make_certificate(alt_names(&["127.0.0.1".into()]).unwrap()).unwrap();
        std::fs::write(&key_file, other_key).unwrap();
        assert!(load(&cert_file, &key_file).err().unwrap().contains("does not belong"));
    }

    #[test]
    fn host_names() {
        assert!(is_dns_name("play.example.org"));
        assert!(!is_dns_name("*.example.org"));
        assert!(!is_dns_name("example.org."));
        assert!(!is_dns_name("-a.example"));
        assert!(!is_dns_name("1234"));
        assert!(alt_names(&[]).is_err());
        assert!(alt_names(&["fe80::1%eth0".into()]).unwrap_err().contains("zone"));
        assert!(alt_names(&["not a host".into()]).is_err());
    }

    #[test]
    fn arguments() {
        let args: Vec<String> = ["--host", "a.example", "--out", "dir", "--force"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let parsed = parse_make_cert_args(&args).unwrap();
        assert_eq!(parsed.hosts, ["a.example"]);
        assert!(parsed.force && parsed.out.is_some());
        assert!(parse_make_cert_args(&["--host".into()]).is_err());
        assert!(parse_make_cert_args(&["--host".into(), "--force".into()]).is_err());
        assert!(parse_make_cert_args(&["--what".into()]).is_err());
    }
}
