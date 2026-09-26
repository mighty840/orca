//! Backoff for failing ACME orders, shared by every caller (#188).
//!
//! The reconcile path requested certificates without any backoff, on every
//! reconcile pass (watchdog 30 s, declarative loop 60 s), so a domain whose
//! DNS no longer points here opened a Let's Encrypt order each time. Only the
//! renewal task backed off, with its own state. Eight dead domains cycling
//! like that can exhaust the *account-wide* limit of 300 orders per 3 hours
//! and block renewals of the domains that matter. The backoff now lives in
//! the `AcmeManager`, so all callers share one budget.

use std::collections::HashMap;
use std::time::Duration;

use tokio::time::Instant;

/// Cooldowns after 1, 2, 3 and 4+ consecutive failures of one domain.
const DELAYS: [Duration; 4] = [
    Duration::from_secs(5 * 60),
    Duration::from_secs(15 * 60),
    Duration::from_secs(60 * 60),
    Duration::from_secs(6 * 60 * 60),
];
/// After Let's Encrypt says `rateLimited`, no orders at all for this long.
const RATE_LIMIT_PAUSE: Duration = Duration::from_secs(60 * 60);

/// A certificate request was skipped because of a recent failure. Callers
/// that run on every reconcile pass should log it quietly.
#[derive(Debug)]
pub struct AcmeCooldown(pub String);

impl std::fmt::Display for AcmeCooldown {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for AcmeCooldown {}

#[derive(Default)]
pub(crate) struct Backoff {
    domains: HashMap<String, (u32, Instant)>,
    paused_until: Option<Instant>,
}

impl Backoff {
    /// Whether an order for `domain` may be opened now.
    pub(crate) fn check(&self, domain: &str) -> Result<(), AcmeCooldown> {
        let now = Instant::now();
        if let Some(until) = self.paused_until
            && now < until
        {
            return Err(AcmeCooldown(format!(
                "ACME paused for {}s more after Let's Encrypt rate-limited us",
                (until - now).as_secs()
            )));
        }
        if let Some((failures, next)) = self.domains.get(domain)
            && now < *next
        {
            return Err(AcmeCooldown(format!(
                "{domain}: last {failures} certificate order(s) failed; next try in {}s",
                (*next - now).as_secs()
            )));
        }
        Ok(())
    }

    pub(crate) fn failed(&mut self, domain: &str, error: &str) {
        let now = Instant::now();
        let failures = self.domains.get(domain).map_or(0, |(n, _)| *n) + 1;
        let delay = DELAYS[(failures as usize - 1).min(DELAYS.len() - 1)];
        self.domains
            .insert(domain.to_string(), (failures, now + delay));
        if error.contains("rateLimited") {
            self.paused_until = Some(now + RATE_LIMIT_PAUSE);
            tracing::error!(
                domain,
                "Let's Encrypt rate limit hit: pausing all certificate orders for {} min",
                RATE_LIMIT_PAUSE.as_secs() / 60
            );
        } else {
            tracing::warn!(
                domain,
                failures,
                retry_in_secs = delay.as_secs(),
                "certificate order failed; backing off"
            );
        }
    }

    pub(crate) fn succeeded(&mut self, domain: &str) {
        self.domains.remove(domain);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test(start_paused = true)]
    async fn a_failing_domain_backs_off_more_each_time() {
        let mut b = Backoff::default();
        assert!(b.check("dead.example").is_ok());

        b.failed("dead.example", "Order not ready after challenges: Invalid");
        assert!(b.check("dead.example").is_err(), "no new order right away");
        assert!(b.check("other.example").is_ok(), "other domains unaffected");

        tokio::time::advance(DELAYS[0] + Duration::from_secs(1)).await;
        assert!(b.check("dead.example").is_ok());
        b.failed("dead.example", "Invalid");
        tokio::time::advance(DELAYS[0] + Duration::from_secs(1)).await;
        assert!(
            b.check("dead.example").is_err(),
            "second cooldown is longer"
        );

        b.succeeded("dead.example");
        assert!(b.check("dead.example").is_ok());
    }

    #[tokio::test(start_paused = true)]
    async fn a_rate_limit_pauses_every_domain() {
        let mut b = Backoff::default();
        b.failed(
            "a.example",
            "urn:ietf:params:acme:error:rateLimited: too many new orders",
        );
        assert!(b.check("cloud.example").is_err());
        tokio::time::advance(RATE_LIMIT_PAUSE + Duration::from_secs(1)).await;
        assert!(b.check("cloud.example").is_ok());
    }
}
