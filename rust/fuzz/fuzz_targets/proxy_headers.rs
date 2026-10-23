//! The shared proxy rules (MIGRATION_PLAN.md X5, X9), used by the gateway and the launcher's relay: what is
//! forwarded never carries a hop-by-hop header or one the `Connection` header names, whatever headers
//! arrive.
#![no_main]

use http::{HeaderMap, HeaderName, HeaderValue};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|headers: Vec<(&[u8], &[u8])>| {
    let mut map = HeaderMap::new();
    for (name, value) in headers.iter().take(64) {
        if let (Ok(name), Ok(value)) = (HeaderName::from_bytes(name), HeaderValue::from_bytes(value)) {
            map.append(name, value);
        }
    }
    let named = proxy::connection_tokens(&map);
    let forwarded = proxy::end_to_end(&map, |_| false);
    for name in forwarded.keys() {
        let name = name.as_str();
        assert!(!proxy::is_hop_by_hop(name), "{name} is hop-by-hop");
        assert!(!named.contains(name), "{name} is named by Connection");
    }
});
