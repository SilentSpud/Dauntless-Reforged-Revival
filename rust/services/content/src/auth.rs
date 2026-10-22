//! The account check for downloads (`src/auth.ts`). Every runtime file request carries the account
//! key (`x-reforged-user-api-key`), and the backend says whose it is (GET /reforged/api/GetUserInfo).
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

pub use crate::input::is_plausible_key;

pub type Lookup = Arc<dyn Fn(String) -> BoxFuture<'static, LookupResult> + Send + Sync>;
pub type Clock = Arc<dyn Fn() -> i64 + Send + Sync>;

pub fn hash_key(key: &str) -> String {
    Sha256::digest(key.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
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
    /// Good keys are cached `ttl`, refused ones 30 s (a bad key can't hammer the backend), at most
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
            // Several downloads start at once with the same key: ask the backend once
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
            // A backend that is down or confused is not the account's fault: that is not cached
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

/// The real lookup (`BackendLookup`): GET <BACKEND_URL>/reforged/api/GetUserInfo with the key in its
/// header. 200 with a UserId is an account, 401/403 a refused key, anything else means the backend
/// can't answer now.
pub fn backend_lookup(backend_url: &str, timeout: Duration) -> Lookup {
    let url = format!("{}/reforged/api/GetUserInfo", backend_url.trim_end_matches('/'));
    Arc::new(move |key: String| {
        let url = url.clone();
        async move {
            let headers = [(crate::app::KEY_HEADER, key.as_str()), ("accept", "application/json")];
            let reply = match crate::client::get(&url, &headers, timeout, 1024 * 1024).await {
                Ok(reply) => reply,
                Err(error) => return LookupResult::Unavailable(format!("backend unreachable: {error}")),
            };
            match reply.status.as_u16() {
                401 | 403 => return LookupResult::Invalid,
                200 => {}
                other => return LookupResult::Unavailable(format!("backend answered {other}")),
            }
            let Ok(body) = reply.body else {
                return LookupResult::Unavailable("backend answered with bad JSON".into());
            };
            let Ok(info) = serde_json::from_slice::<Value>(&body) else {
                return LookupResult::Unavailable("backend answered with bad JSON".into());
            };
            let Some(user_id) = info.get("UserId").and_then(Value::as_str).filter(|u| !u.is_empty()) else {
                return LookupResult::Unavailable("backend answer has no UserId".into());
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
    //! test/auth.test.ts
    use super::*;
    use std::sync::atomic::{AtomicBool, AtomicI64, AtomicUsize, Ordering};

    fn good() -> String {
        format!("RRK_{}", "ab".repeat(24))
    }

    fn account() -> AccountInfo {
        AccountInfo {
            user_id: "UID-1".into(),
            username: "tester".into(),
            is_admin: false,
        }
    }

    fn counting(answer: impl Fn(&str) -> LookupResult + Send + Sync + 'static) -> (Lookup, Arc<AtomicUsize>) {
        let calls = Arc::new(AtomicUsize::new(0));
        let counted = calls.clone();
        let lookup: Lookup = Arc::new(move |key: String| {
            counted.fetch_add(1, Ordering::SeqCst);
            let result = answer(&key);
            async move { result }.boxed()
        });
        (lookup, calls)
    }

    fn clock(now: &Arc<AtomicI64>) -> Clock {
        let now = now.clone();
        Arc::new(move || now.load(Ordering::SeqCst))
    }

    fn cache(lookup: Lookup) -> AuthCache {
        AuthCache::new(lookup, Duration::from_secs(300))
    }

    #[tokio::test]
    async fn a_good_key_is_looked_up_once_and_then_served_from_the_cache() {
        let (lookup, calls) = counting(|key| {
            if key == good() {
                LookupResult::Ok(account())
            } else {
                LookupResult::Invalid
            }
        });
        let cache = cache(lookup);
        assert_eq!(cache.check(&good()).await, LookupResult::Ok(account()));
        assert_eq!(cache.check(&good()).await, LookupResult::Ok(account()));
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn the_cache_is_keyed_by_the_key_s_hash_and_never_holds_the_key() {
        let (lookup, _) = counting(|_| LookupResult::Ok(account()));
        let cache = cache(lookup);
        cache.check(&good()).await;
        assert!(cache.has_hash(&hash_key(&good())));
        assert!(!cache.has_hash(&good()));
        let entries = cache.entries.lock().unwrap();
        assert!(!format!("{:?} {:?}", entries.by_hash, entries.order).contains(&good()));
        assert_eq!(hash_key(&good()).len(), 64);
        assert_ne!(hash_key(&good()), good());
    }

    #[tokio::test]
    async fn entries_expire_after_the_ttl() {
        let now = Arc::new(AtomicI64::new(1_000_000));
        let (lookup, calls) = counting(|_| LookupResult::Ok(account()));
        let cache = AuthCache::with(
            lookup,
            Duration::from_secs(300),
            Duration::from_secs(30),
            2000,
            clock(&now),
        );
        cache.check(&good()).await;
        now.fetch_add(5 * 60 * 1000 - 1, Ordering::SeqCst);
        cache.check(&good()).await;
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        now.fetch_add(2, Ordering::SeqCst);
        cache.check(&good()).await;
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn refused_keys_are_cached_briefly_backend_outages_not_at_all() {
        let now = Arc::new(AtomicI64::new(0));
        let down = Arc::new(AtomicBool::new(true));
        let is_down = down.clone();
        let (lookup, calls) = counting(move |key| {
            if is_down.load(Ordering::SeqCst) {
                LookupResult::Unavailable("down".into())
            } else if key == good() {
                LookupResult::Ok(account())
            } else {
                LookupResult::Invalid
            }
        });
        let cache = AuthCache::with(
            lookup,
            Duration::from_secs(300),
            Duration::from_secs(30),
            2000,
            clock(&now),
        );
        assert!(matches!(cache.check(&good()).await, LookupResult::Unavailable(_)));
        down.store(false, Ordering::SeqCst);
        assert!(matches!(cache.check(&good()).await, LookupResult::Ok(_)));
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        assert_eq!(cache.check("RRK_wrong").await, LookupResult::Invalid);
        assert_eq!(cache.check("RRK_wrong").await, LookupResult::Invalid);
        assert_eq!(calls.load(Ordering::SeqCst), 3);
        now.fetch_add(30_001, Ordering::SeqCst);
        assert_eq!(cache.check("RRK_wrong").await, LookupResult::Invalid);
        assert_eq!(calls.load(Ordering::SeqCst), 4);
    }

    #[tokio::test]
    async fn implausible_keys_are_refused_without_a_lookup() {
        let (lookup, calls) = counting(|_| LookupResult::Ok(account()));
        let cache = cache(lookup);
        // The header is a string or absent here: Node's undefined and 42 have no Rust counterpart
        let long = "x".repeat(257);
        for bad in ["", "has space", "line\nbreak", long.as_str(), "\u{e4}"] {
            assert_eq!(cache.check(bad).await, LookupResult::Invalid, "{bad}");
            assert!(!is_plausible_key(bad));
        }
        assert_eq!(calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn concurrent_checks_of_one_key_share_a_single_lookup() {
        let gate = Arc::new(tokio::sync::Semaphore::new(0));
        let calls = Arc::new(AtomicUsize::new(0));
        let (opened, counted) = (gate.clone(), calls.clone());
        let lookup: Lookup = Arc::new(move |_| {
            counted.fetch_add(1, Ordering::SeqCst);
            let opened = opened.clone();
            async move {
                let _ = opened.acquire().await;
                LookupResult::Ok(account())
            }
            .boxed()
        });
        let cache = Arc::new(cache(lookup));
        let checks: Vec<_> = (0..3)
            .map(|_| {
                let cache = cache.clone();
                tokio::spawn(async move { cache.check(&good()).await })
            })
            .collect();
        tokio::time::sleep(Duration::from_millis(50)).await;
        gate.add_permits(10);
        for check in checks {
            assert!(matches!(check.await.unwrap(), LookupResult::Ok(_)));
        }
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn a_lookup_that_panics_counts_as_unavailable() {
        let lookup: Lookup = Arc::new(|_| async { panic!("boom") }.boxed());
        let cache = cache(lookup);
        assert!(matches!(cache.check(&good()).await, LookupResult::Unavailable(_)));
        assert_eq!(cache.len(), 0);
    }

    #[tokio::test]
    async fn the_cache_stays_bounded() {
        let (lookup, _) = counting(|_| LookupResult::Invalid);
        let cache = AuthCache::with(
            lookup,
            Duration::from_secs(300),
            Duration::from_secs(30),
            10,
            Arc::new(crate::now_ms),
        );
        for i in 0..50 {
            cache.check(&format!("RRK_{i}")).await;
        }
        assert_eq!(cache.len(), 10);
        assert!(cache.has_hash(&hash_key("RRK_49")));
        assert!(!cache.has_hash(&hash_key("RRK_0")));
    }

    #[tokio::test]
    async fn backend_lookup_maps_the_backend_s_answers() {
        let mode = Arc::new(std::sync::Mutex::new("ok"));
        let seen = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
        let (state_mode, state_seen) = (mode.clone(), seen.clone());
        let handler = move |request: axum::extract::Request| {
            let mode = *state_mode.lock().unwrap();
            state_seen.lock().unwrap().push(request.uri().to_string());
            let key = request
                .headers()
                .get(crate::app::KEY_HEADER)
                .map(|v| v.as_bytes().to_vec());
            async move {
                use axum::http::StatusCode;
                if key.as_deref() != Some(good().as_bytes()) {
                    return (StatusCode::UNAUTHORIZED, String::new());
                }
                match mode {
                    "500" => (StatusCode::INTERNAL_SERVER_ERROR, String::new()),
                    "junk" => (StatusCode::OK, "{nope".into()),
                    _ => (
                        StatusCode::OK,
                        r#"{"UserId":"UID-1","Username":"tester","IsAdmin":false}"#.into(),
                    ),
                }
            }
        };
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move { axum::serve(listener, axum::Router::new().fallback(handler)).await });

        let lookup = backend_lookup(&format!("http://127.0.0.1:{port}/"), Duration::from_secs(2));
        assert_eq!(lookup(good()).await, LookupResult::Ok(account()));
        assert_eq!(lookup("RRK_nope".into()).await, LookupResult::Invalid);
        *mode.lock().unwrap() = "junk";
        assert!(matches!(lookup(good()).await, LookupResult::Unavailable(_)));
        *mode.lock().unwrap() = "500";
        assert!(matches!(lookup(good()).await, LookupResult::Unavailable(_)));
        assert!(
            seen.lock()
                .unwrap()
                .iter()
                .all(|url| url == "/reforged/api/GetUserInfo")
        );

        // Nothing listens on a port just freed
        let free = std::net::TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        let nowhere = backend_lookup(&format!("http://127.0.0.1:{free}"), Duration::from_secs(1));
        assert!(matches!(nowhere(good()).await, LookupResult::Unavailable(_)));
    }
}
