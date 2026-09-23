//! Where the control-plane API listens (`cluster.api_bind`).
//!
//! The API used to bind `0.0.0.0` unconditionally, which put it, and the agent
//! channel on it, on every network the host is attached to, including the
//! public internet. `api_bind` lets an operator list the addresses instead,
//! typically loopback plus a private or mesh address.

use std::collections::HashSet;
use std::future::Future;
use std::net::{IpAddr, SocketAddr};

use axum::Router;
use tokio::net::TcpListener;

/// The socket addresses to bind for `bind` on `port`.
///
/// Rejects lists that cannot all be bound at once, with a message that names
/// the setting: an empty list, a duplicate, or a wildcard alongside other
/// addresses. A wildcard already covers every interface, so a second bind on
/// the same port would fail later with a bare "address in use".
pub(crate) fn listen_addrs(bind: &[IpAddr], port: u16) -> Result<Vec<SocketAddr>, String> {
    if bind.is_empty() {
        return Err("cluster.api_bind lists no addresses, so the API would listen nowhere".into());
    }
    let mut seen = HashSet::new();
    if let Some(dup) = bind.iter().find(|ip| !seen.insert(**ip)) {
        return Err(format!("cluster.api_bind lists {dup} more than once"));
    }
    if bind.len() > 1
        && let Some(wild) = bind.iter().find(|ip| ip.is_unspecified())
    {
        return Err(format!(
            "cluster.api_bind combines the wildcard {wild} with other addresses; {wild} \
             already covers every interface, so list it alone or list specific addresses"
        ));
    }
    Ok(bind.iter().map(|ip| SocketAddr::new(*ip, port)).collect())
}

/// Whether the `orca` CLI and TUI on this host, which connect to `127.0.0.1`
/// by default, can reach an API bound to `bind`.
///
/// `::` counts: with Linux's default dual-stack sockets it accepts IPv4 too.
/// `::1` does not: it is IPv6-only and the CLI dials `127.0.0.1`.
pub(crate) fn reachable_from_local_cli(bind: &[IpAddr]) -> bool {
    bind.iter().any(|ip| match ip {
        IpAddr::V4(v4) => v4.is_unspecified() || v4.is_loopback(),
        IpAddr::V6(v6) => v6.is_unspecified(),
    })
}

/// Serve `app` on every listener until `shutdown` resolves, then drain them all.
///
/// One signal fans out to many servers through a `watch` channel. Unlike
/// `Notify`, it keeps its value, so a server that starts waiting after the
/// signal already fired still sees it and stops. Returns the first server
/// error, if any.
pub(crate) async fn serve_all<F>(
    app: Router,
    listeners: Vec<TcpListener>,
    shutdown: F,
) -> anyhow::Result<()>
where
    F: Future<Output = ()> + Send + 'static,
{
    let (stop_tx, stop_rx) = tokio::sync::watch::channel(false);
    tokio::spawn(async move {
        shutdown.await;
        let _ = stop_tx.send(true);
    });

    let mut servers = tokio::task::JoinSet::new();
    for listener in listeners {
        let app = app.clone();
        let mut stop = stop_rx.clone();
        servers.spawn(async move {
            axum::serve(
                listener,
                app.into_make_service_with_connect_info::<SocketAddr>(),
            )
            .with_graceful_shutdown(async move {
                // A dropped sender without a signal (the signal future
                // panicked) must not read as "shut down": keep serving.
                if stop.wait_for(|stopped| *stopped).await.is_err() {
                    std::future::pending::<()>().await;
                }
            })
            .await
        });
    }
    while let Some(joined) = servers.join_next().await {
        joined??;
    }
    Ok(())
}

#[cfg(test)]
#[path = "api_listen_tests.rs"]
mod tests;
