//! The gateway's request policy (MIGRATION_PLAN.md RS3, RX3; docs/security/gateway.md): the first
//! thing a public request meets. A method, target and headers from the network must give a decision,
//! never a panic, and nothing proxied may reach a host-only part of the backend: no host-only header,
//! no host-only prefix, nothing under /reforged but the four public routes, whatever the spelling or
//! the /__origin wrapping.
#![no_main]

use arbitrary::Arbitrary;
use gateway::policy::{
    Decision, HOST_ONLY_HEADERS, HOST_ONLY_PREFIXES, PUBLIC_REFORGED_API, Upstream, canonical_path, check_path,
    classify, has_bearer, split_target, unwrap_origin,
};
use http::{HeaderMap, HeaderName, HeaderValue};
use libfuzzer_sys::fuzz_target;

#[derive(Arbitrary, Debug)]
struct Request<'a> {
    method: &'a str,
    target: &'a str,
    headers: Vec<(&'a [u8], &'a [u8])>,
    upgrade: bool,
}

fn under(path: &str, prefix: &str) -> bool {
    path == prefix || path.starts_with(&format!("{prefix}/"))
}

fuzz_target!(|request: Request<'_>| {
    let mut headers = HeaderMap::new();
    for (name, value) in request.headers.iter().take(32) {
        if let (Ok(name), Ok(value)) = (HeaderName::from_bytes(name), HeaderValue::from_bytes(value)) {
            headers.append(name, value);
        }
    }
    let decision = classify(request.method, request.target, &headers, request.upgrade);
    let _ = has_bearer(&headers);
    let Some((path, _query)) = split_target(request.target) else {
        assert!(matches!(decision, Decision::Reject { status: 400, .. }));
        return;
    };
    let _ = check_path(path);
    let unwrapped = unwrap_origin(path);
    if let Some(unwrapped) = &unwrapped {
        assert!(unwrapped.path.starts_with('/'), "{unwrapped:?}");
        if let Some(host) = &unwrapped.origin {
            assert!(
                !host.is_empty() && host.len() <= 253 && !host.contains(".."),
                "{host:?}"
            );
        }
    }
    let Decision::Proxy { upstream, .. } = decision else {
        return;
    };
    // Proxied: the path was clean, unwrapped once, and canonical
    assert!(check_path(path));
    let unwrapped = unwrapped.expect("a proxied path unwraps");
    let canonical = canonical_path(&unwrapped.path).expect("a proxied path decodes");
    assert!(canonical.starts_with('/'), "{canonical:?}");
    for name in HOST_ONLY_HEADERS {
        assert!(!headers.contains_key(name), "{name} went through");
    }
    for prefix in HOST_ONLY_PREFIXES {
        assert!(!under(&canonical, prefix), "{canonical} went through");
    }
    if under(&canonical, "/reforged") {
        let name = canonical.strip_prefix("/reforged/api/").unwrap_or("");
        let verb = request.method.to_uppercase();
        assert!(
            PUBLIC_REFORGED_API
                .iter()
                .any(|(route, methods)| *route == name && methods.contains(&verb.as_str())),
            "{verb} {canonical} went through"
        );
        assert_eq!(upstream, Upstream::Backend);
    }
    if upstream == Upstream::Content {
        assert!(unwrapped.origin.is_none() && under(&canonical, "/content"));
    }
});
