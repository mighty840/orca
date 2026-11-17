//! A read limit inside `orca mcp` (#80): the master's audit log and
//! `[api_rate_limit]` cover changes only, so a model looping `get_logs`
//! would otherwise hit the master without bound.
//!
//! A token bucket over every API call the server makes, whether for a tool,
//! a resource or a prompt's fetches: a burst of `per_minute`, then one every
//! `60 / per_minute` seconds. Over it a call fails at once, without reaching
//! the master.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use tokio::sync::Mutex;
use tokio::time::Instant;

use crate::backend::{ApiBackend, BackendError};

/// The default reads per minute.
pub const READS_PER_MINUTE: u32 = 60;

/// An [`ApiBackend`] that refuses calls over `per_minute`.
pub struct Limited {
    inner: Arc<dyn ApiBackend>,
    per_minute: u32,
    bucket: Mutex<(f64, Instant)>,
}

impl Limited {
    pub fn new(inner: Arc<dyn ApiBackend>, per_minute: u32) -> Self {
        let per_minute = per_minute.max(1);
        Self {
            inner,
            per_minute,
            bucket: Mutex::new((f64::from(per_minute), Instant::now())),
        }
    }

    /// Take one call, or say how long until one is available.
    async fn take(&self) -> Result<(), Duration> {
        let capacity = f64::from(self.per_minute);
        let per_sec = capacity / 60.0;
        let mut bucket = self.bucket.lock().await;
        let now = Instant::now();
        let (tokens, at) = &mut *bucket;
        *tokens = (*tokens + now.duration_since(*at).as_secs_f64() * per_sec).min(capacity);
        *at = now;
        if *tokens >= 1.0 {
            *tokens -= 1.0;
            Ok(())
        } else {
            Err(Duration::from_secs_f64((1.0 - *tokens) / per_sec))
        }
    }
}

#[async_trait]
impl ApiBackend for Limited {
    async fn get(&self, path_and_query: &str) -> Result<String, BackendError> {
        if let Err(wait) = self.take().await {
            return Err(BackendError::Limited {
                per_minute: self.per_minute,
                retry_secs: wait.as_secs().max(1),
            });
        }
        self.inner.get(path_and_query).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Ok;

    #[async_trait]
    impl ApiBackend for Ok {
        async fn get(&self, _: &str) -> Result<String, BackendError> {
            std::result::Result::Ok("{}".into())
        }
    }

    #[tokio::test(start_paused = true)]
    async fn a_burst_of_the_limit_then_one_per_interval() {
        let limited = Limited::new(Arc::new(Ok), 60);
        for _ in 0..60 {
            assert!(limited.get("/x").await.is_ok());
        }
        match limited.get("/x").await {
            Err(BackendError::Limited {
                per_minute: 60,
                retry_secs: 1,
            }) => {}
            other => panic!("{other:?}"),
        }

        tokio::time::advance(Duration::from_secs(1)).await;
        assert!(limited.get("/x").await.is_ok(), "one back after a second");
        assert!(limited.get("/x").await.is_err());
    }
}
