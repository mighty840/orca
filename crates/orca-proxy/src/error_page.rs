//! Branded static HTML page served when the proxy can't route a request:
//! an unknown host (404) or a known host with no healthy backend (503).
//! Replaces the bare plain-text body with a docs-themed page that links to
//! the repo and docs. The page is fully self-contained (inline CSS + logo,
//! no external assets) so it renders even on a degraded / offline box.

use hyper::{Response, StatusCode, header};

use crate::body::{ProxyBody, full_body};

const REPO_URL: &str = "https://github.com/mighty840/orca";
const DOCS_URL: &str = "https://mighty840.github.io/orca";
const TEMPLATE: &str = include_str!("error_page.html");

/// Render the branded error page for a no-route / no-backend response.
///
/// `host` is the (untrusted) request `Host` header and is HTML-escaped before
/// being interpolated, so a hostile `Host` can't inject markup.
pub(crate) fn branded_error(status: StatusCode, host: &str) -> Response<ProxyBody> {
    let (title, message) = match status {
        StatusCode::SERVICE_UNAVAILABLE => (
            "Service unavailable",
            "This host is served by Orca, but none of its backends are reachable right now.",
        ),
        _ => (
            "Not found",
            "No service is configured for this host on this Orca cluster.",
        ),
    };

    // Substitute trusted tokens first; HOST (escaped, attacker-controllable)
    // last so its contents can never trigger a further replacement.
    let html = TEMPLATE
        .replace("{{STATUS}}", status.as_str())
        .replace("{{TITLE}}", title)
        .replace("{{MESSAGE}}", message)
        .replace("{{DOCS}}", DOCS_URL)
        .replace("{{REPO}}", REPO_URL)
        .replace("{{HOST}}", &escape(host));

    let mut resp = Response::new(full_body(hyper::body::Bytes::from(html)));
    *resp.status_mut() = status;
    resp.headers_mut().insert(
        header::CONTENT_TYPE,
        header::HeaderValue::from_static("text/html; charset=utf-8"),
    );
    resp
}

/// Minimal HTML-escape for text interpolated into the page body / attributes.
fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#x27;"),
            other => out.push(other),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use http_body_util::BodyExt;

    async fn body_string(resp: Response<ProxyBody>) -> String {
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        String::from_utf8(bytes.to_vec()).unwrap()
    }

    #[tokio::test]
    async fn not_found_page_reflects_host_and_links() {
        let resp = branded_error(StatusCode::NOT_FOUND, "dashboard.swarmhaul.defited.com");
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
        assert_eq!(
            resp.headers().get(header::CONTENT_TYPE).unwrap(),
            "text/html; charset=utf-8"
        );
        let html = body_string(resp).await;
        assert!(html.contains("dashboard.swarmhaul.defited.com"));
        assert!(html.contains("Not found"));
        assert!(html.contains(REPO_URL));
        assert!(html.contains(DOCS_URL));
        // No unsubstituted tokens left behind.
        assert!(!html.contains("{{"));
    }

    #[tokio::test]
    async fn service_unavailable_uses_503_copy() {
        let resp = branded_error(StatusCode::SERVICE_UNAVAILABLE, "api.example.com");
        assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
        let html = body_string(resp).await;
        assert!(html.contains("Service unavailable"));
        assert!(html.contains("503"));
    }

    #[tokio::test]
    async fn host_is_html_escaped() {
        // A hostile Host header must not break out into markup.
        let resp = branded_error(StatusCode::NOT_FOUND, "x\"><script>alert(1)</script>");
        let html = body_string(resp).await;
        assert!(!html.contains("<script>alert(1)</script>"));
        assert!(html.contains("&lt;script&gt;"));
    }
}
