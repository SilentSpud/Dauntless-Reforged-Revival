//! What a player's launcher sends the content server (MIGRATION_PLAN.md RS2, RX3;
//! docs/security/content.md): the request path, which picks the route and the runtime file, and the
//! headers it reads, the account key and If-None-Match, repeated lines included. None of it may
//! panic; a path that names a runtime file names exactly one of the two, so nothing else on disk is
//! reachable; a key that passes the shape check is one printable token, so it cannot add a header to
//! the backend request.
#![no_main]

use content::input::{KEY_HEADER, Route, header_text, if_none_match_hits, is_plausible_key, is_runtime_file_name};
use http::{HeaderMap, HeaderValue};
use libfuzzer_sys::arbitrary::{self, Arbitrary};
use libfuzzer_sys::fuzz_target;

#[derive(Debug, Arbitrary)]
struct Request<'a> {
    target: &'a str,
    keys: Vec<&'a [u8]>,
    if_none_match: Vec<&'a [u8]>,
    etag: &'a str,
}

fuzz_target!(|request: Request<'_>| {
    // The path up to "?", as the server cuts it
    let path_only = request.target.split('?').next().unwrap_or("");
    match Route::of(path_only) {
        Route::RuntimeFile(name) => {
            if is_runtime_file_name(name) {
                assert!(name == "MysticParadox.dll" || name == "winmm.dll", "{name}");
                assert!(path_only.ends_with(name) && !name.contains(['/', '\\', '%']));
            }
        }
        Route::BrandingImage(name) => assert!(path_only.ends_with(name)),
        Route::Unknown | Route::RuntimeManifest | Route::Branding | Route::News => {}
    }

    // Headers as hyper hands them over: only values it accepts
    let mut headers = HeaderMap::new();
    for key in &request.keys {
        if let Ok(value) = HeaderValue::from_bytes(key) {
            headers.append(KEY_HEADER, value);
        }
    }
    for tag in &request.if_none_match {
        if let Ok(value) = HeaderValue::from_bytes(tag) {
            headers.append("if-none-match", value);
        }
    }
    match header_text(&headers, KEY_HEADER) {
        Some(key) if is_plausible_key(&key) => {
            assert!((1..=256).contains(&key.len()));
            assert!(key.bytes().all(|b| (0x21..=0x7e).contains(&b)), "{key:?}");
            // The value the backend request carries is one valid header value
            assert!(HeaderValue::from_str(&key).is_ok());
        }
        // Refused without asking the backend
        Some(_) => {}
        None => assert!(headers.get(KEY_HEADER).is_none()),
    }
    let inm = header_text(&headers, "if-none-match");
    let hit = if_none_match_hits(inm.as_deref(), request.etag);
    // "*" always hits, and a tag never hits an ETag it does not contain
    if inm.as_deref().is_some_and(|h| service_kit::js::trim(h) == "*") {
        assert!(hit);
    }
    if hit && !request.etag.is_empty() {
        assert!(
            inm.as_deref()
                .is_some_and(|h| h.contains(request.etag) || service_kit::js::trim(h) == "*")
        );
    }
});
