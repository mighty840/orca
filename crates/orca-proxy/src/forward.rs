//! Backend forwarding with retry logic and HTTPS redirect helpers.

use hyper::{Response, StatusCode};
use tracing::{debug, error};

use crate::RouteTarget;

/// Forward a request to a backend, retrying once on 502 with a different
/// backend if multiple exist.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn forward_with_retry(
    client: &reqwest::Client,
    matched: &[RouteTarget],
    base_idx: usize,
    method: &reqwest::Method,
    headers: &hyper::HeaderMap,
    body: &hyper::body::Bytes,
    path_and_query: &str,
    host: &str,
) -> Response<http_body_util::Full<hyper::body::Bytes>> {
    let max_attempts = if matched.len() > 1 { 2 } else { 1 };

    for attempt in 0..max_attempts {
        let idx = (base_idx + attempt) % matched.len();
        let target = &matched[idx];
        let uri = format!("http://{}{}", target.address, path_and_query);

        debug!("Proxying {host}{path_and_query} -> {uri} (attempt {attempt})");

        let mut forward_req = client.request(method.clone(), &uri);
        for (key, value) in headers {
            if key != "host" {
                forward_req = forward_req.header(key, value);
            }
        }
        forward_req = forward_req.body(body.clone());

        match forward_req.send().await {
            Ok(resp) if resp.status() == StatusCode::BAD_GATEWAY && attempt + 1 < max_attempts => {
                debug!("Got 502 from {}, retrying", target.address);
                continue;
            }
            Ok(resp) => {
                let status = resp.status();
                let resp_body = resp.bytes().await.unwrap_or_default();
                let mut response = Response::new(http_body_util::Full::new(resp_body));
                *response.status_mut() = status;
                return response;
            }
            Err(e) if attempt + 1 < max_attempts => {
                debug!("Backend error from {}: {e}, retrying", target.address);
                continue;
            }
            Err(e) => {
                error!("Proxy error to {}: {e}", target.address);
                return super::handler::error_response(
                    StatusCode::BAD_GATEWAY,
                    &format!("backend error: {e}"),
                );
            }
        }
    }

    super::handler::error_response(StatusCode::BAD_GATEWAY, "all backends failed")
}

/// Build a 301 redirect response to HTTPS.
pub(crate) fn redirect_to_https(
    host: &str,
    path: &str,
) -> Response<http_body_util::Full<hyper::body::Bytes>> {
    let location = format!("https://{host}{path}");
    let body = http_body_util::Full::new(hyper::body::Bytes::from(format!("Moved to {location}")));
    let mut resp = Response::new(body);
    *resp.status_mut() = StatusCode::MOVED_PERMANENTLY;
    resp.headers_mut().insert(
        hyper::header::LOCATION,
        location.parse().expect("valid location header"),
    );
    resp
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_redirect_to_https_returns_301() {
        let resp = redirect_to_https("example.com", "/some/path");
        assert_eq!(resp.status(), StatusCode::MOVED_PERMANENTLY);
    }

    #[test]
    fn test_redirect_to_https_location_header() {
        let resp = redirect_to_https("example.com", "/foo?bar=1");
        let location = resp
            .headers()
            .get(hyper::header::LOCATION)
            .expect("should have Location header")
            .to_str()
            .unwrap();
        assert_eq!(location, "https://example.com/foo?bar=1");
    }

    #[test]
    fn test_redirect_to_https_root_path() {
        let resp = redirect_to_https("myapp.dev", "/");
        let location = resp
            .headers()
            .get(hyper::header::LOCATION)
            .unwrap()
            .to_str()
            .unwrap();
        assert_eq!(location, "https://myapp.dev/");
        assert_eq!(resp.status(), StatusCode::MOVED_PERMANENTLY);
    }

    #[test]
    fn test_redirect_preserves_complex_path() {
        let resp = redirect_to_https("sub.example.com", "/a/b/c?x=1&y=2");
        let location = resp
            .headers()
            .get(hyper::header::LOCATION)
            .unwrap()
            .to_str()
            .unwrap();
        assert_eq!(location, "https://sub.example.com/a/b/c?x=1&y=2");
    }
}
