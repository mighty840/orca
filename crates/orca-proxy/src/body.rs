//! Body type used across the proxy: type-erased so handlers can return
//! either a fully-buffered body (for short error/redirect responses) or a
//! streaming body (for large upstream responses like Docker registry blob
//! pulls or pushes).
//!
//! Why this exists: the old proxy used `Full<Bytes>` everywhere, which
//! forced every upstream response to be fully read into memory before
//! forwarding to the client. For a 100 MB blob pull that meant ~100 MB
//! per request held in the proxy task, plus 100 MB of added latency
//! while the proxy buffered. Under burst (CI registry push, parallel
//! blob uploads) the per-task memory + extra latency consumed the accept
//! loop's headroom enough that *new* TLS handshakes from concurrent
//! clients (`docker login`, browser TLS) timed out — a soft DoS by
//! upstream slowness.
//!
//! Using `BoxBody<Bytes, BoxError>` lets us:
//! - keep simple `full_body(bytes)` for short responses (errors,
//!   redirects, ACME challenge), and
//! - stream upstream responses directly with `stream_body(stream)`
//!   without an intermediate buffer.

use std::convert::Infallible;
use std::error::Error as StdError;

use futures_util::TryStreamExt;
use http_body_util::combinators::BoxBody;
use http_body_util::{BodyExt, Full, StreamBody};
use hyper::body::{Bytes, Frame};

/// Errors carried by the body stream. Boxed so we can unify reqwest errors
/// (response streaming) with the Infallible from buffered responses.
pub type BoxError = Box<dyn StdError + Send + Sync>;

/// Body type used for every response the proxy emits. Type-erased so a
/// single handler return type covers buffered and streaming variants.
pub type ProxyBody = BoxBody<Bytes, BoxError>;

/// Wrap fully-buffered bytes as a `ProxyBody`. Use for error responses,
/// redirects, ACME challenge responses, and anything else short.
pub fn full_body(bytes: Bytes) -> ProxyBody {
    // Full's error is Infallible; lift it into BoxError for type uniformity.
    Full::new(bytes)
        .map_err(|never: Infallible| match never {})
        .boxed()
}

/// Convenience: empty body.
pub fn empty_body() -> ProxyBody {
    full_body(Bytes::new())
}

/// Wrap a `Stream<Item = Result<Bytes, E>>` as a `ProxyBody`, streaming
/// frames straight to the client without buffering.
///
/// Use for upstream response bodies (`reqwest::Response::bytes_stream`)
/// so the proxy doesn't have to read the full body into memory before
/// the client sees the first byte.
pub fn stream_body<S, E>(stream: S) -> ProxyBody
where
    S: futures_util::Stream<Item = Result<Bytes, E>> + Send + Sync + 'static,
    E: StdError + Send + Sync + 'static,
{
    let frames = stream
        .map_ok(Frame::data)
        .map_err(|e| Box::new(e) as BoxError);
    StreamBody::new(frames).boxed()
}

#[cfg(test)]
mod tests {
    use super::*;
    use http_body_util::BodyExt;

    #[tokio::test]
    async fn full_body_collects_back_to_bytes() {
        let body = full_body(Bytes::from_static(b"hello world"));
        let collected = body.collect().await.unwrap().to_bytes();
        assert_eq!(&collected[..], b"hello world");
    }

    #[tokio::test]
    async fn empty_body_yields_zero_bytes() {
        let body = empty_body();
        let collected = body.collect().await.unwrap().to_bytes();
        assert!(collected.is_empty());
    }

    #[tokio::test]
    async fn stream_body_preserves_chunk_order() {
        let chunks = vec![
            Ok::<_, std::io::Error>(Bytes::from_static(b"first ")),
            Ok(Bytes::from_static(b"second ")),
            Ok(Bytes::from_static(b"third")),
        ];
        let stream = futures_util::stream::iter(chunks);
        let body = stream_body(stream);
        let collected = body.collect().await.unwrap().to_bytes();
        assert_eq!(&collected[..], b"first second third");
    }

    #[tokio::test]
    async fn stream_body_propagates_errors() {
        let chunks = vec![
            Ok::<_, std::io::Error>(Bytes::from_static(b"ok ")),
            Err(std::io::Error::other("boom")),
        ];
        let stream = futures_util::stream::iter(chunks);
        let body = stream_body(stream);
        // Collecting should surface the error rather than truncating silently.
        let result = body.collect().await;
        assert!(result.is_err(), "stream error must propagate, not swallow");
    }

    #[tokio::test]
    async fn stream_body_large_payload_does_not_buffer_eagerly() {
        // 16 chunks * 1 MiB. Confirms the body shape can carry large
        // payloads frame-by-frame.
        let chunk = Bytes::from(vec![0xAB; 1024 * 1024]);
        let chunks: Vec<Result<Bytes, std::io::Error>> =
            (0..16).map(|_| Ok(chunk.clone())).collect();
        let stream = futures_util::stream::iter(chunks);
        let body = stream_body(stream);
        let collected = body.collect().await.unwrap().to_bytes();
        assert_eq!(collected.len(), 16 * 1024 * 1024);
    }
}
