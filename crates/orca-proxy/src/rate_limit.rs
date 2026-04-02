//! Simple per-IP token bucket rate limiter.

use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::{Arc, Mutex};
use std::time::Instant;

/// Maximum tokens (requests) per IP per second.
const MAX_TOKENS: u32 = 100;

/// Per-IP token bucket rate limiter.
///
/// Uses `std::sync::Mutex` for fast in-memory operations.
#[derive(Clone, Default)]
pub struct RateLimiter {
    buckets: Arc<Mutex<HashMap<IpAddr, (u32, Instant)>>>,
}

impl RateLimiter {
    /// Create a new rate limiter.
    pub fn new() -> Self {
        Self {
            buckets: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Check if a request from the given IP is allowed.
    ///
    /// Returns `true` if the request is within the rate limit, `false` if it
    /// should be rejected with 429 Too Many Requests.
    pub fn check(&self, ip: IpAddr) -> bool {
        let mut buckets = self.buckets.lock().expect("rate limiter lock poisoned");
        let now = Instant::now();

        let entry = buckets.entry(ip).or_insert((MAX_TOKENS, now));

        // Refill tokens based on elapsed time
        let elapsed = now.duration_since(entry.1);
        let refill = (elapsed.as_secs_f64() * MAX_TOKENS as f64) as u32;
        if refill > 0 {
            entry.0 = (entry.0 + refill).min(MAX_TOKENS);
            entry.1 = now;
        }

        // Try to consume a token
        if entry.0 > 0 {
            entry.0 -= 1;
            true
        } else {
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Ipv4Addr;

    #[test]
    fn allows_requests_under_limit() {
        let limiter = RateLimiter::new();
        let ip = IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1));
        for _ in 0..MAX_TOKENS {
            assert!(limiter.check(ip));
        }
    }

    #[test]
    fn rejects_requests_over_limit() {
        let limiter = RateLimiter::new();
        let ip = IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1));
        for _ in 0..MAX_TOKENS {
            limiter.check(ip);
        }
        assert!(!limiter.check(ip));
    }

    #[test]
    fn separate_buckets_per_ip() {
        let limiter = RateLimiter::new();
        let ip1 = IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1));
        let ip2 = IpAddr::V4(Ipv4Addr::new(10, 0, 0, 2));
        for _ in 0..MAX_TOKENS {
            limiter.check(ip1);
        }
        assert!(!limiter.check(ip1));
        assert!(limiter.check(ip2));
    }

    #[test]
    fn test_rate_limiter_allows_under_limit() {
        let limiter = RateLimiter::new();
        let ip = IpAddr::V4(Ipv4Addr::new(192, 168, 1, 1));
        for i in 0..50 {
            assert!(limiter.check(ip), "request {i} should be allowed");
        }
    }

    #[test]
    fn test_rate_limiter_blocks_over_limit() {
        let limiter = RateLimiter::new();
        let ip = IpAddr::V4(Ipv4Addr::new(192, 168, 1, 2));
        // Exhaust all 100 tokens
        for _ in 0..MAX_TOKENS {
            limiter.check(ip);
        }
        // The 101st request should be blocked
        assert!(
            !limiter.check(ip),
            "request 101 should be blocked after exhausting tokens"
        );
    }

    #[test]
    fn test_rate_limiter_allows_different_ips() {
        let limiter = RateLimiter::new();
        let ip1 = IpAddr::V4(Ipv4Addr::new(10, 1, 0, 1));
        let ip2 = IpAddr::V4(Ipv4Addr::new(10, 1, 0, 2));
        // 100 requests from each IP should all be allowed
        for i in 0..MAX_TOKENS {
            assert!(limiter.check(ip1), "ip1 request {i} should be allowed");
            assert!(limiter.check(ip2), "ip2 request {i} should be allowed");
        }
    }

    #[test]
    fn test_rate_limiter_refills_after_time() {
        let limiter = RateLimiter::new();
        let ip = IpAddr::V4(Ipv4Addr::new(192, 168, 1, 3));
        // Exhaust all tokens
        for _ in 0..MAX_TOKENS {
            limiter.check(ip);
        }
        assert!(!limiter.check(ip), "should be blocked after exhaustion");

        // Wait for refill (1 second should refill ~100 tokens)
        std::thread::sleep(std::time::Duration::from_secs(1));

        assert!(
            limiter.check(ip),
            "should be allowed again after token refill"
        );
    }
}
