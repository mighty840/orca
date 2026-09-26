//! Timeouts for requests the proxy forwards to backends (#187).
//!
//! The client used reqwest's `read_timeout(120s)` believing it was an
//! inactivity timeout. It is only one for the response *body*. From dispatch
//! to response headers it is a single wall-clock timer that nothing resets,
//! so every request whose upload plus backend think time exceeded 120 s got
//! a 502, however fast data was flowing: a Harbor layer push, a large
//! Nextcloud upload, a slow LLM completion (`latency_ms=120001` in prod).
//!
//! Instead:
//! - while the request body uploads, fail only after [`UPLOAD_IDLE`] with no
//!   chunk (a stalled client or backend);
//! - once it has been sent, give the backend [`RESPONSE_WAIT`] to answer;
//! - stream the response body with [`RESPONSE_IDLE`] per chunk.
//!
//! TCP keepalive on the client catches a backend that vanished.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures_util::{Stream, StreamExt};
use hyper::body::Bytes;
use tokio::time::Instant;
use tracing::warn;

/// Longest gap between request-body chunks before the upload counts as stalled.
pub(crate) const UPLOAD_IDLE: Duration = Duration::from_secs(120);
/// How long the backend may think after the request was fully sent.
pub(crate) const RESPONSE_WAIT: Duration = Duration::from_secs(600);
/// Longest gap between response-body chunks.
pub(crate) const RESPONSE_IDLE: Duration = Duration::from_secs(120);
/// Log a still-pending request after this long. At 30 s, Gitea Actions'
/// long polls produced ~15k warnings a week and buried real stalls.
pub(crate) const SLOW_WARN: Duration = Duration::from_secs(120);

/// The HTTP client for backend requests.
pub(crate) fn client() -> reqwest::Client {
    reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_secs(10))
        // No read_timeout: see the module docs. A backend that disappears
        // without closing the connection is caught by keepalive instead.
        .tcp_keepalive(Duration::from_secs(30))
        .pool_idle_timeout(Duration::from_secs(90))
        .build()
        .expect("failed to build HTTP client")
}

/// Progress of one request's upload.
#[derive(Clone)]
pub(crate) struct Upload {
    last_chunk: Arc<Mutex<Instant>>,
    done: Arc<AtomicBool>,
}

impl Upload {
    /// An upload that has not started sending yet.
    pub(crate) fn started() -> Self {
        Self {
            last_chunk: Arc::new(Mutex::new(Instant::now())),
            done: Arc::new(AtomicBool::new(false)),
        }
    }

    /// A request with no body to upload.
    pub(crate) fn none() -> Self {
        let u = Self::started();
        u.done.store(true, Ordering::SeqCst);
        u
    }

    fn touch(&self) {
        *self.last_chunk.lock().unwrap_or_else(|e| e.into_inner()) = Instant::now();
    }

    fn finish(&self) {
        self.touch();
        self.done.store(true, Ordering::SeqCst);
    }

    /// Why the request should be given up now, if it should.
    fn expired(&self) -> Option<String> {
        let idle = self
            .last_chunk
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .elapsed();
        if self.done.load(Ordering::SeqCst) {
            (idle > RESPONSE_WAIT).then(|| {
                format!(
                    "backend sent no response within {}s of receiving the request",
                    RESPONSE_WAIT.as_secs()
                )
            })
        } else {
            (idle > UPLOAD_IDLE)
                .then(|| format!("upload stalled: no data for {}s", UPLOAD_IDLE.as_secs()))
        }
    }

    /// Wrap the request body so every chunk counts as progress.
    pub(crate) fn track<S, E>(
        &self,
        body: S,
    ) -> impl Stream<Item = Result<Bytes, E>> + Send + Sync + 'static
    where
        S: Stream<Item = Result<Bytes, E>> + Send + Sync + 'static,
        E: 'static,
    {
        let on_chunk = self.clone();
        let on_end = self.clone();
        body.inspect(move |_| on_chunk.touch())
            .chain(futures_util::stream::poll_fn(move |_| {
                on_end.finish();
                std::task::Poll::Ready(None)
            }))
    }
}

/// Why a forwarded request failed.
#[derive(Debug)]
pub(crate) enum SendError {
    Backend(reqwest::Error),
    Idle(String),
}

impl SendError {
    /// A timeout is a `504 Gateway Timeout`; anything else a `502` (#192).
    pub(crate) fn status(&self) -> hyper::StatusCode {
        match self {
            Self::Idle(_) => hyper::StatusCode::GATEWAY_TIMEOUT,
            Self::Backend(e) if e.is_timeout() => hyper::StatusCode::GATEWAY_TIMEOUT,
            Self::Backend(_) => hyper::StatusCode::BAD_GATEWAY,
        }
    }

    /// What the client is told. Never the upstream error itself, which
    /// carries the backend's internal `127.0.0.1:port` (#192).
    pub(crate) fn client_message(&self) -> &'static str {
        if self.status() == hyper::StatusCode::GATEWAY_TIMEOUT {
            "upstream timed out"
        } else {
            "upstream unavailable"
        }
    }

    /// Log the failure with its cause. reqwest's `Display` is only "error
    /// sending request for url (...)": a container being recreated, a
    /// timeout and a TLS error all logged the same line (#192).
    pub(crate) fn log(&self, backend: &str, path: &str) {
        let (kind, cause) = match self {
            Self::Idle(why) => ("timeout", why.clone()),
            Self::Backend(e) => {
                let kind = if e.is_timeout() {
                    "timeout"
                } else if e.is_connect() {
                    "connect"
                } else if e.is_body() {
                    "body"
                } else if e.is_request() {
                    "request"
                } else {
                    "other"
                };
                (kind, error_chain(e))
            }
        };
        tracing::error!(
            backend = %backend,
            path = %path,
            kind,
            status = self.status().as_u16(),
            "proxy error: {cause}"
        );
    }
}

/// `e` and every `source()` below it, joined with ": ".
fn error_chain(e: &dyn std::error::Error) -> String {
    let mut out = e.to_string();
    let mut next = e.source();
    while let Some(cause) = next {
        out.push_str(": ");
        out.push_str(&cause.to_string());
        next = cause.source();
    }
    out
}

impl std::fmt::Display for SendError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Backend(e) => write!(f, "{e}"),
            Self::Idle(why) => f.write_str(why),
        }
    }
}

/// Send `request`, failing only when [`Upload`] says it stalled, and logging
/// once while it is still pending after [`SLOW_WARN`].
pub(crate) async fn send<F>(
    request: F,
    upload: &Upload,
    backend: &str,
    path: &str,
) -> Result<reqwest::Response, SendError>
where
    F: std::future::Future<Output = Result<reqwest::Response, reqwest::Error>>,
{
    tokio::pin!(request);
    let started = Instant::now();
    let mut warned = false;
    let mut tick = tokio::time::interval(Duration::from_secs(1));
    loop {
        tokio::select! {
            result = &mut request => return result.map_err(SendError::Backend),
            _ = tick.tick() => {
                if let Some(why) = upload.expired() {
                    return Err(SendError::Idle(why));
                }
                if !warned && started.elapsed() >= SLOW_WARN {
                    warned = true;
                    warn!(
                        backend = %backend,
                        path = %path,
                        threshold_secs = SLOW_WARN.as_secs(),
                        "slow backend: no response yet and still waiting"
                    );
                }
            }
        }
    }
}

/// The response body with [`RESPONSE_IDLE`] per chunk.
pub(crate) fn response_body<S, E>(
    body: S,
) -> impl Stream<Item = Result<Bytes, std::io::Error>> + Send + Sync + 'static
where
    S: Stream<Item = Result<Bytes, E>> + Send + Sync + Unpin + 'static,
    E: std::fmt::Display + 'static,
{
    futures_util::stream::unfold(Some(body), |state| async move {
        let mut body = state?;
        match tokio::time::timeout(RESPONSE_IDLE, body.next()).await {
            Ok(Some(Ok(chunk))) => Some((Ok(chunk), Some(body))),
            Ok(Some(Err(e))) => Some((Err(std::io::Error::other(e.to_string())), None)),
            Ok(None) => None,
            Err(_) => Some((
                Err(std::io::Error::new(
                    std::io::ErrorKind::TimedOut,
                    format!("backend sent no data for {}s", RESPONSE_IDLE.as_secs()),
                )),
                None,
            )),
        }
    })
}

#[cfg(test)]
#[path = "backend_timeouts_tests.rs"]
mod tests;
