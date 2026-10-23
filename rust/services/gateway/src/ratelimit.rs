//! Token buckets per key, an IPv4 address or an IPv6 /64 (`src/ratelimit.ts`). A bucket holds up to
//! `burst` requests and refills at `per_minute`. Full buckets are forgotten, so memory only holds
//! recently busy peers.

use indexmap::IndexMap;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BucketSpec {
    pub burst: f64,
    pub per_minute: f64,
}

impl BucketSpec {
    pub const fn new(burst: u32, per_minute: f64) -> Self {
        Self {
            burst: burst as f64,
            per_minute,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Take {
    Ok,
    Refused { retry_after_seconds: u64 },
}

struct Bucket {
    tokens: f64,
    at: i64,
}

fn refilled(burst: f64, per_ms: f64, bucket: &Bucket, now: i64) -> f64 {
    burst.min(bucket.tokens + (now - bucket.at).max(0) as f64 * per_ms)
}

pub struct RateLimiter {
    spec: BucketSpec,
    per_ms: f64,
    max_keys: usize,
    /// In the order peers first came, as a JavaScript Map keeps them
    buckets: IndexMap<String, Bucket>,
}

impl RateLimiter {
    pub fn new(spec: BucketSpec) -> Self {
        Self::with_max_keys(spec, 100_000)
    }

    pub fn with_max_keys(spec: BucketSpec, max_keys: usize) -> Self {
        Self {
            spec,
            per_ms: spec.per_minute / 60_000.0,
            max_keys,
            buckets: IndexMap::new(),
        }
    }

    pub fn spec(&self) -> BucketSpec {
        self.spec
    }

    pub fn take(&mut self, key: &str, now: i64) -> Take {
        if !self.buckets.contains_key(key) && self.buckets.len() >= self.max_keys {
            self.sweep(now);
            if self.buckets.len() >= self.max_keys {
                // Still full: forget the oldest peer rather than grow without bound
                self.buckets.shift_remove_index(0);
            }
        }
        let (burst, per_ms) = (self.spec.burst, self.per_ms);
        let bucket = self
            .buckets
            .entry(key.to_owned())
            .and_modify(|bucket| {
                bucket.tokens = refilled(burst, per_ms, bucket, now);
                bucket.at = now;
            })
            .or_insert(Bucket { tokens: burst, at: now });
        if bucket.tokens >= 1.0 {
            bucket.tokens -= 1.0;
            return Take::Ok;
        }
        let missing = 1.0 - bucket.tokens;
        // (The small epsilon keeps floating-point dust from adding a second.)
        let seconds = if per_ms > 0.0 {
            (missing / per_ms / 1000.0 - 1e-6).ceil()
        } else {
            3600.0
        };
        Take::Refused {
            retry_after_seconds: seconds.max(1.0) as u64,
        }
    }

    /// Drops buckets that have refilled completely: they behave exactly like a new one.
    pub fn sweep(&mut self, now: i64) {
        let (burst, per_ms) = (self.spec.burst, self.per_ms);
        self.buckets
            .retain(|_, bucket| refilled(burst, per_ms, bucket, now) < burst);
    }

    pub fn len(&self) -> usize {
        self.buckets.len()
    }

    pub fn is_empty(&self) -> bool {
        self.buckets.is_empty()
    }
}

/// "<burst>,<per minute>", for example "300,180". Decimals are fine for slow refills ("5,0.2" is one
/// request every five minutes after a burst of five).
pub fn parse_bucket_spec(text: &str, name: &str) -> Result<BucketSpec, String> {
    let shape = || format!("{name} must look like \"<burst>,<per minute>\", for example \"300,180\"");
    let (burst, rate) = text.split_once(',').ok_or_else(shape)?;
    let (burst, rate) = (service_kit::js::trim(burst), service_kit::js::trim(rate));
    let digits = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
    let rate_ok = match rate.split_once('.') {
        Some((whole, fraction)) => digits(whole) && digits(fraction),
        None => digits(rate),
    };
    if !digits(burst) || !rate_ok {
        return Err(shape());
    }
    let burst: f64 = burst.parse().map_err(|_| shape())?;
    let per_minute: f64 = rate.parse().map_err(|_| shape())?;
    if !(1.0..=1_000_000.0).contains(&burst) || per_minute > 1_000_000.0 {
        return Err(format!(
            "{name}: burst must be 1 to 1000000 and the rate at most 1000000 per minute"
        ));
    }
    Ok(BucketSpec { burst, per_minute })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allows_the_burst_then_refills_at_the_rate() {
        let mut limiter = RateLimiter::new(BucketSpec::new(3, 60.0));
        let t0 = 1_000_000;
        for _ in 0..3 {
            assert_eq!(limiter.take("a", t0), Take::Ok);
        }
        assert_eq!(limiter.take("a", t0), Take::Refused { retry_after_seconds: 1 });
        // Another key has its own bucket
        assert_eq!(limiter.take("b", t0), Take::Ok);
        // One per second comes back
        assert_eq!(limiter.take("a", t0 + 1000), Take::Ok);
        assert!(matches!(limiter.take("a", t0 + 1000), Take::Refused { .. }));
        assert_eq!(limiter.take("a", t0 + 2500), Take::Ok);
    }

    #[test]
    fn slow_buckets_and_forgetting_full_ones() {
        let mut limiter = RateLimiter::new(BucketSpec::new(1, 0.2));
        assert_eq!(limiter.take("x", 0), Take::Ok);
        assert_eq!(
            limiter.take("x", 0),
            Take::Refused {
                retry_after_seconds: 300
            }
        );
        limiter.sweep(299_000);
        assert_eq!(limiter.len(), 1);
        limiter.sweep(300_100);
        assert_eq!(limiter.len(), 0);
    }

    #[test]
    fn never_holds_more_than_max_keys() {
        let mut limiter = RateLimiter::with_max_keys(BucketSpec::new(5, 1.0), 100);
        for index in 0..1000 {
            limiter.take(&format!("k{index}"), 0);
        }
        assert!(limiter.len() <= 100);
    }

    #[test]
    fn parses_burst_and_rate() {
        assert_eq!(parse_bucket_spec("300,180", "X"), Ok(BucketSpec::new(300, 180.0)));
        assert_eq!(parse_bucket_spec(" 5 , 0.2 ", "X"), Ok(BucketSpec::new(5, 0.2)));
        for bad in ["", "5", "0,1", "-1,2", "a,b", "5,1,2", "5,.2", "5,2.", "1e3,1"] {
            assert!(parse_bucket_spec(bad, "X").is_err(), "{bad}");
        }
    }
}
