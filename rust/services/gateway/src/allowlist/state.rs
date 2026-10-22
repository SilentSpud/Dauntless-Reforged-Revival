//! The live set (`src/allowlist/state.ts`): addresses of players seen logging in or sending
//! heartbeats, each kept for the TTL after it was last reported. Callers validate addresses first
//! ([`crate::ip::check_allowlist_ip`]).

use std::collections::HashMap;

use serde_json::Value;

use crate::ip::{ParsedIp, parse_ip};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refresh {
    Added,
    Refreshed,
    Full,
}

pub struct AllowlistState {
    ttl_ms: i64,
    max_entries: usize,
    entries: HashMap<String, (ParsedIp, i64)>,
}

impl AllowlistState {
    pub fn new(ttl_ms: i64, max_entries: usize) -> Self {
        Self {
            ttl_ms,
            max_entries,
            entries: HashMap::new(),
        }
    }

    pub fn refresh(&mut self, ip: &ParsedIp, now: i64) -> Refresh {
        if let Some((_, expires_at)) = self.entries.get_mut(&ip.canonical) {
            *expires_at = now + self.ttl_ms;
            return Refresh::Refreshed;
        }
        if self.entries.len() >= self.max_entries {
            return Refresh::Full;
        }
        self.entries
            .insert(ip.canonical.clone(), (ip.clone(), now + self.ttl_ms));
        Refresh::Added
    }

    /// Removes and returns the addresses whose time is up.
    pub fn expire(&mut self, now: i64) -> Vec<String> {
        let mut gone: Vec<String> = self
            .entries
            .iter()
            .filter(|(_, (_, expires_at))| *expires_at <= now)
            .map(|(key, _)| key.clone())
            .collect();
        gone.sort();
        for key in &gone {
            self.entries.remove(key);
        }
        gone
    }

    pub fn next_expiry(&self) -> Option<i64> {
        self.entries.values().map(|(_, at)| *at).min()
    }

    /// Sorted IPv4 first, then IPv6, each by numeric value: the same set always gives the same rule.
    pub fn list(&self) -> Vec<String> {
        let mut entries: Vec<&ParsedIp> = self.entries.values().map(|(ip, _)| ip).collect();
        entries.sort_by(|a, b| a.version.cmp(&b.version).then_with(|| a.bytes.cmp(&b.bytes)));
        entries.into_iter().map(|ip| ip.canonical.clone()).collect()
    }

    pub fn snapshot(&self) -> Vec<(String, i64)> {
        self.list()
            .into_iter()
            .map(|ip| {
                let at = self.entries[&ip].1;
                (ip, at)
            })
            .collect()
    }

    /// Restores a saved snapshot; entries that are gone, malformed or not allowed are skipped.
    pub fn load(&mut self, saved: &Value, now: i64, accept: impl Fn(&ParsedIp) -> bool) -> usize {
        let Some(items) = saved.as_array() else {
            return 0;
        };
        let mut loaded = 0;
        for item in items {
            let Some(ip) = item.get("ip").and_then(Value::as_str).and_then(parse_ip) else {
                continue;
            };
            let Some(expires_at) = item.get("expiresAt").and_then(Value::as_f64) else {
                continue;
            };
            if !accept(&ip) || !expires_at.is_finite() || expires_at <= now as f64 {
                continue;
            }
            if self.entries.len() >= self.max_entries {
                break;
            }
            // Never longer than one TTL from now, whatever the file says
            let at = (expires_at as i64).min(now + self.ttl_ms);
            self.entries.insert(ip.canonical.clone(), (ip, at));
            loaded += 1;
        }
        loaded
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::ip::IpClass;

    fn ip(text: &str) -> ParsedIp {
        parse_ip(text).unwrap()
    }

    #[test]
    fn keeps_an_address_for_the_ttl_after_its_last_report() {
        let mut state = AllowlistState::new(600_000, 10);
        assert_eq!(state.refresh(&ip("203.0.113.7"), 0), Refresh::Added);
        assert_eq!(state.refresh(&ip("203.0.113.7"), 1000), Refresh::Refreshed);
        assert!(state.expire(600_999).is_empty());
        assert_eq!(state.expire(601_000), ["203.0.113.7"]);
        assert!(state.is_empty());
    }

    #[test]
    fn lists_ipv4_before_ipv6_in_numeric_order() {
        let mut state = AllowlistState::new(1000, 10);
        for text in ["2001:db8::2", "10.0.0.1", "9.0.0.1", "2001:db8::1", "203.0.113.7"] {
            state.refresh(&ip(text), 0);
        }
        assert_eq!(
            state.list(),
            ["9.0.0.1", "10.0.0.1", "203.0.113.7", "2001:db8::1", "2001:db8::2"]
        );
    }

    #[test]
    fn refuses_new_addresses_when_full() {
        let mut state = AllowlistState::new(1000, 2);
        assert_eq!(state.refresh(&ip("203.0.113.1"), 0), Refresh::Added);
        assert_eq!(state.refresh(&ip("203.0.113.2"), 0), Refresh::Added);
        assert_eq!(state.refresh(&ip("203.0.113.3"), 0), Refresh::Full);
        assert_eq!(state.refresh(&ip("203.0.113.1"), 0), Refresh::Refreshed);
    }

    #[test]
    fn restores_a_saved_set_skipping_stale_malformed_and_refused_entries() {
        let mut state = AllowlistState::new(600_000, 10);
        let saved = json!([
            { "ip": "203.0.113.7", "expiresAt": 5_000_000 },
            { "ip": "198.51.100.2", "expiresAt": 999 },
            { "ip": "10.0.0.1", "expiresAt": 5_000 },
            { "ip": "0.0.0.0/0", "expiresAt": 5_000 },
            { "ip": 7, "expiresAt": 5_000 },
            null,
        ]);
        assert_eq!(state.load(&saved, 1000, |p| p.class == IpClass::Public), 1);
        assert_eq!(state.snapshot(), [("203.0.113.7".to_owned(), 601_000)]);
        assert_eq!(state.load(&json!("nonsense"), 0, |_| true), 0);
    }
}
