//! In-memory, per-key token-bucket rate limiting.
//!
//! Keys are typically client IP addresses. They live **only** in this in-memory
//! map and are never logged or persisted — they exist purely to throttle abusive
//! callers, in keeping with the service's no-IP-storage privacy model.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Instant;

use crate::config::RateLimitRule;

struct Bucket {
    tokens: f64,
    last: Instant,
}

/// A keyed token-bucket limiter. Cheap to clone-share behind an `Arc`.
pub struct RateLimiter {
    capacity: f64,
    refill_per_sec: f64,
    buckets: Mutex<HashMap<String, Bucket>>,
}

impl RateLimiter {
    pub fn new(per_minute: u32, burst: u32) -> Self {
        Self {
            capacity: burst.max(1) as f64,
            refill_per_sec: (per_minute.max(1) as f64) / 60.0,
            buckets: Mutex::new(HashMap::new()),
        }
    }

    pub fn from_rule(rule: &RateLimitRule) -> Self {
        Self::new(rule.per_minute, rule.burst)
    }

    /// Return true if a request for `key` is allowed, consuming one token.
    pub fn check(&self, key: &str) -> bool {
        let now = Instant::now();
        let mut buckets = self.buckets.lock().unwrap_or_else(|e| e.into_inner());
        let bucket = buckets.entry(key.to_owned()).or_insert(Bucket {
            tokens: self.capacity,
            last: now,
        });
        let elapsed = now.duration_since(bucket.last).as_secs_f64();
        bucket.tokens = (bucket.tokens + elapsed * self.refill_per_sec).min(self.capacity);
        bucket.last = now;
        if bucket.tokens >= 1.0 {
            bucket.tokens -= 1.0;
            true
        } else {
            false
        }
    }

    /// Drop fully-refilled (idle) buckets to bound memory. Call periodically.
    pub fn cleanup(&self) {
        let now = Instant::now();
        let mut buckets = self.buckets.lock().unwrap_or_else(|e| e.into_inner());
        buckets.retain(|_, bucket| {
            let elapsed = now.duration_since(bucket.last).as_secs_f64();
            let tokens = (bucket.tokens + elapsed * self.refill_per_sec).min(self.capacity);
            tokens < self.capacity
        });
    }

    #[cfg(test)]
    fn tracked_keys(&self) -> usize {
        self.buckets.lock().unwrap().len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allows_up_to_burst_then_denies() {
        // 60/min = 1/sec refill, burst 2.
        let limiter = RateLimiter::new(60, 2);
        assert!(limiter.check("1.2.3.4"));
        assert!(limiter.check("1.2.3.4"));
        assert!(!limiter.check("1.2.3.4"), "third immediate request is throttled");
        // A different key has its own bucket.
        assert!(limiter.check("5.6.7.8"));
    }

    #[test]
    fn cleanup_drops_idle_keys() {
        let limiter = RateLimiter::new(6000, 5);
        assert!(limiter.check("k"));
        // The key refills almost immediately at 100/sec, so cleanup reclaims it.
        std::thread::sleep(std::time::Duration::from_millis(60));
        limiter.cleanup();
        assert_eq!(limiter.tracked_keys(), 0);
    }
}
