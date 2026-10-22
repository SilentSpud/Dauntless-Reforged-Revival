//! Download slots (`src/limits.ts`). Each account may have a few files streaming at once (the
//! launcher downloads several in parallel), and the whole server has a ceiling so a crowd of
//! first-time downloads can't starve the host's upload for the players already in a hunt.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    AccountLimit,
    TotalLimit,
}

impl Refusal {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::AccountLimit => "account_limit",
            Self::TotalLimit => "total_limit",
        }
    }
}

#[derive(Debug, Default)]
struct Active {
    per_account: HashMap<String, usize>,
    total: usize,
}

#[derive(Debug)]
pub struct StreamLimiter {
    per_account: usize,
    total: usize,
    active: Arc<Mutex<Active>>,
}

/// A held slot; dropping it frees it.
#[derive(Debug)]
pub struct Slot {
    account: String,
    active: Arc<Mutex<Active>>,
}

impl Drop for Slot {
    fn drop(&mut self) {
        let mut active = self.active.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let now = active
            .per_account
            .get(&self.account)
            .copied()
            .unwrap_or(1)
            .saturating_sub(1);
        if now == 0 {
            active.per_account.remove(&self.account);
        } else {
            active.per_account.insert(self.account.clone(), now);
        }
        active.total = active.total.saturating_sub(1);
    }
}

impl StreamLimiter {
    pub fn new(per_account: usize, total: usize) -> Self {
        Self {
            per_account,
            total,
            active: Arc::default(),
        }
    }

    /// Streams open, for one account or the whole server.
    pub fn active(&self, account: Option<&str>) -> usize {
        let active = self.active.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        match account {
            None => active.total,
            Some(account) => active.per_account.get(account).copied().unwrap_or(0),
        }
    }

    pub fn try_acquire(&self, account: &str) -> Result<Slot, Refusal> {
        let mut active = self.active.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let current = active.per_account.get(account).copied().unwrap_or(0);
        if current >= self.per_account {
            return Err(Refusal::AccountLimit);
        }
        if active.total >= self.total {
            return Err(Refusal::TotalLimit);
        }
        active.per_account.insert(account.to_owned(), current + 1);
        active.total += 1;
        Ok(Slot {
            account: account.to_owned(),
            active: self.active.clone(),
        })
    }
}

#[cfg(test)]
mod tests {
    //! test/limits.test.ts
    use super::*;

    #[test]
    fn per_account_limit() {
        let limiter = StreamLimiter::new(6, 100);
        let mut held: Vec<Slot> = (0..6).map(|_| limiter.try_acquire("A").unwrap()).collect();
        assert_eq!(limiter.try_acquire("A").unwrap_err(), Refusal::AccountLimit);
        let _b = limiter.try_acquire("B").expect("another account is not affected");
        assert_eq!(limiter.active(Some("A")), 6);
        // A slot is released once, when it is dropped: Node's second release() has no counterpart
        drop(held.remove(0));
        assert_eq!(limiter.active(Some("A")), 5);
        let _again = limiter.try_acquire("A").unwrap();
        assert_eq!(limiter.active(None), 7);
    }

    #[test]
    fn server_wide_limit() {
        let limiter = StreamLimiter::new(6, 3);
        let _a = limiter.try_acquire("A").unwrap();
        let _b = limiter.try_acquire("B").unwrap();
        let c = limiter.try_acquire("C").unwrap();
        assert_eq!(limiter.try_acquire("D").unwrap_err(), Refusal::TotalLimit);
        drop(c);
        let _d = limiter.try_acquire("D").unwrap();
        assert_eq!(limiter.active(Some("C")), 0);
    }
}
