//! The account check for downloads (`src/auth.ts`). Every file request carries the account key
//! (`x-undaunted-user-api-key`), and the metagame says whose it is (GET /undaunted/api/GetUserInfo).
//! Answers are cached for a few minutes, keyed by the SHA-256 of the key: the key itself is never
//! stored, cached or logged here.

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures_util::FutureExt;
use futures_util::future::{BoxFuture, Shared};
use serde_json::Value;
use sha2::{Digest, Sha256};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountInfo {
    pub user_id: String,
    pub username: String,
    pub is_admin: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LookupResult {
    Ok(AccountInfo),
    Invalid,
    Unavailable(String),
}

pub type Lookup = Arc<dyn Fn(String) -> BoxFuture<'static, LookupResult> + Send + Sync>;
pub type Clock = Arc<dyn Fn() -> i64 + Send + Sync>;

pub fn hash_key(key: &str) -> String {
    Sha256::digest(key.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// Printable ASCII without spaces, at most 256 characters. Anything else can't be a key and is
/// refused without asking the metagame (and can't be smuggled into the header sent to it).
pub fn is_plausible_key(key: &str) -> bool {
    (1..=256).contains(&key.len()) && key.bytes().all(|b| (0x21..=0x7e).contains(&b))
}

#[derive(Default)]
struct Entries {
    /// Key hash -> the answer, when it expires, and its place in `order`
    by_hash: HashMap<String, (LookupResult, i64, u64)>,
    /// Oldest first
    order: BTreeMap<u64, String>,
    next: u64,
    in_flight: HashMap<String, Shared<BoxFuture<'static, LookupResult>>>,
}

pub struct AuthCache {
    lookup: Lookup,
    ttl_ms: i64,
    negative_ttl_ms: i64,
    max_entries: usize,
    now: Clock,
    entries: Arc<Mutex<Entries>>,
}

impl AuthCache {
    /// Good keys are cached `ttl`, refused ones 30 s (a bad key can't hammer the metagame), at most
    /// 2000 entries, oldest dropped first.
    pub fn new(lookup: Lookup, ttl: Duration) -> Self {
        Self::with(lookup, ttl, Duration::from_secs(30), 2000, Arc::new(crate::now_ms))
    }

    pub fn with(lookup: Lookup, ttl: Duration, negative_ttl: Duration, max_entries: usize, now: Clock) -> Self {
        Self {
            lookup,
            ttl_ms: ttl.as_millis() as i64,
            negative_ttl_ms: negative_ttl.as_millis() as i64,
            max_entries,
            now,
            entries: Arc::default(),
        }
    }

    pub fn len(&self) -> usize {
        self.entries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .by_hash
            .len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Whether this key hash is cached (tests; takes the hash, never the key).
    pub fn has_hash(&self, hash: &str) -> bool {
        self.entries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .by_hash
            .contains_key(hash)
    }

    pub async fn check(&self, key: &str) -> LookupResult {
        if !is_plausible_key(key) {
            return LookupResult::Invalid;
        }
        let hash = hash_key(key);
        let now = (self.now)();
        let pending = {
            let mut entries = self.entries.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            if let Some((result, expires, seq)) = entries.by_hash.get(&hash).cloned() {
                if expires > now {
                    return result;
                }
                entries.by_hash.remove(&hash);
                entries.order.remove(&seq);
            }
            // Several downloads start at once with the same key: ask the metagame once
            match entries.in_flight.get(&hash) {
                Some(pending) => pending.clone(),
                None => {
                    let lookup = (self.lookup)(key.to_owned());
                    let shared = std::panic::AssertUnwindSafe(lookup)
                        .catch_unwind()
                        .map(|r| r.unwrap_or_else(|_| LookupResult::Unavailable("lookup failed".into())))
                        .boxed()
                        .shared();
                    entries.in_flight.insert(hash.clone(), shared.clone());
                    shared
                }
            }
        };
        let result = pending.await;
        let mut entries = self.entries.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        if entries.in_flight.remove(&hash).is_some() && !matches!(result, LookupResult::Unavailable(_)) {
            // A metagame that is down or confused is not the account's fault: that is not cached
            let ttl = if matches!(result, LookupResult::Ok(_)) {
                self.ttl_ms
            } else {
                self.negative_ttl_ms
            };
            entries.next += 1;
            let seq = entries.next;
            if let Some((_, _, old)) = entries
                .by_hash
                .insert(hash.clone(), (result.clone(), (self.now)() + ttl, seq))
            {
                entries.order.remove(&old);
            }
            entries.order.insert(seq, hash);
            while entries.by_hash.len() > self.max_entries {
                let Some((_, oldest)) = entries.order.pop_first() else {
                    break;
                };
                entries.by_hash.remove(&oldest);
            }
        }
        result
    }
}

/// The real lookup: GET <METAGAME_URL>/undaunted/api/GetUserInfo with the key in its header. 200 with
/// a UserId is an account, 401/403 a refused key, anything else means the metagame can't answer now.
pub fn metagame_lookup(metagame_url: &str, timeout: Duration) -> Lookup {
    let url = format!("{}/undaunted/api/GetUserInfo", metagame_url.trim_end_matches('/'));
    Arc::new(move |key: String| {
        let url = url.clone();
        async move {
            let headers = [
                ("x-undaunted-user-api-key", key.as_str()),
                ("accept", "application/json"),
            ];
            let reply = match crate::client::get(&url, &headers, timeout, 1024 * 1024).await {
                Ok(reply) => reply,
                Err(error) => return LookupResult::Unavailable(format!("metagame unreachable: {error}")),
            };
            match reply.status.as_u16() {
                401 | 403 => return LookupResult::Invalid,
                200 => {}
                other => return LookupResult::Unavailable(format!("metagame answered {other}")),
            }
            let Ok(body) = reply.body else {
                return LookupResult::Unavailable("metagame answered with bad JSON".into());
            };
            let Ok(info) = serde_json::from_slice::<Value>(&body) else {
                return LookupResult::Unavailable("metagame answered with bad JSON".into());
            };
            let Some(user_id) = info.get("UserId").and_then(Value::as_str).filter(|u| !u.is_empty()) else {
                return LookupResult::Unavailable("metagame answer has no UserId".into());
            };
            LookupResult::Ok(AccountInfo {
                user_id: user_id.to_owned(),
                username: info.get("Username").and_then(Value::as_str).unwrap_or("").to_owned(),
                is_admin: info.get("IsAdmin") == Some(&Value::Bool(true)),
            })
        }
        .boxed()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicI64, AtomicUsize, Ordering};

    fn counting(answer: fn(&str) -> LookupResult) -> (Lookup, Arc<AtomicUsize>) {
        let calls = Arc::new(AtomicUsize::new(0));
        let counted = calls.clone();
        let lookup: Lookup = Arc::new(move |key: String| {
            counted.fetch_add(1, Ordering::SeqCst);
            let result = answer(&key);
            async move {
                tokio::time::sleep(Duration::from_millis(20)).await;
                result
            }
            .boxed()
        });
        (lookup, calls)
    }

    fn ok(key: &str) -> LookupResult {
        match key {
            "UUK_good" => LookupResult::Ok(AccountInfo {
                user_id: "UID-A".into(),
                username: "alpha".into(),
                is_admin: false,
            }),
            "UUK_down" => LookupResult::Unavailable("down".into()),
            _ => LookupResult::Invalid,
        }
    }

    #[tokio::test]
    async fn answers_are_cached_by_hash_and_expire() {
        let now = Arc::new(AtomicI64::new(0));
        let clock = now.clone();
        let (lookup, calls) = counting(ok);
        let cache = AuthCache::with(
            lookup,
            Duration::from_secs(300),
            Duration::from_secs(30),
            2000,
            Arc::new(move || clock.load(Ordering::SeqCst)),
        );
        assert!(matches!(cache.check("UUK_good").await, LookupResult::Ok(_)));
        assert!(matches!(cache.check("UUK_good").await, LookupResult::Ok(_)));
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert!(cache.has_hash(&hash_key("UUK_good")) && !cache.has_hash("UUK_good"));
        now.store(300_001, Ordering::SeqCst);
        cache.check("UUK_good").await;
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        // Refused keys briefly, outages not at all
        cache.check("UUK_bad").await;
        cache.check("UUK_bad").await;
        assert_eq!(calls.load(Ordering::SeqCst), 3);
        cache.check("UUK_down").await;
        cache.check("UUK_down").await;
        assert_eq!(calls.load(Ordering::SeqCst), 5);
        // Implausible keys never reach the lookup
        for bad in ["", "has space", &"x".repeat(257), "caf\u{e9}"] {
            assert_eq!(cache.check(bad).await, LookupResult::Invalid);
        }
        assert_eq!(calls.load(Ordering::SeqCst), 5);
    }

    #[tokio::test]
    async fn concurrent_checks_share_one_lookup_and_the_cache_stays_bounded() {
        let (lookup, calls) = counting(ok);
        let cache = Arc::new(AuthCache::with(
            lookup,
            Duration::from_secs(300),
            Duration::from_secs(30),
            3,
            Arc::new(|| 0),
        ));
        let checks: Vec<_> = (0..5)
            .map(|_| {
                let c = cache.clone();
                tokio::spawn(async move { c.check("UUK_good").await })
            })
            .collect();
        for check in checks {
            assert!(matches!(check.await.unwrap(), LookupResult::Ok(_)));
        }
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        for i in 0..10 {
            cache.check(&format!("UUK_other{i}")).await;
        }
        assert_eq!(cache.len(), 3);
    }
}
