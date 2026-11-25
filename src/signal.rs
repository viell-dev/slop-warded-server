//! What the operating system asks of the process.

use crate::Tls;

/// Completes when the process is asked to stop.
///
/// On Unix that is Ctrl+C (`SIGINT`) or `SIGTERM`. On Windows it is Ctrl+C, Ctrl+Break, closing
/// the console, or system shutdown.
///
/// # Panics
///
/// When the operating system refuses to report signals, because the server could then only be
/// stopped by killing it.
pub async fn shutdown() {
    const EXPECTATION: &str = "the process can listen for shutdown signals";

    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};

        let mut interrupt = signal(SignalKind::interrupt()).expect(EXPECTATION);
        let mut terminate = signal(SignalKind::terminate()).expect(EXPECTATION);
        tokio::select! {
            _ = interrupt.recv() => {}
            _ = terminate.recv() => {}
        }
    }
    #[cfg(windows)]
    {
        use tokio::signal::windows;

        let mut interrupt = windows::ctrl_c().expect(EXPECTATION);
        let mut break_key = windows::ctrl_break().expect(EXPECTATION);
        let mut close = windows::ctrl_close().expect(EXPECTATION);
        let mut system = windows::ctrl_shutdown().expect(EXPECTATION);
        tokio::select! {
            _ = interrupt.recv() => {}
            _ = break_key.recv() => {}
            _ = close.recv() => {}
            _ = system.recv() => {}
        }
    }
}

/// Reload the certificate files whenever the process receives `SIGHUP`.
///
/// [`Server::serve`](crate::Server::serve) does this. Spawn it beside
/// [`BoundServer::serve_until`](crate::BoundServer::serve_until) to have it there too; without it,
/// `SIGHUP` ends the process. It never completes, and does nothing on Windows.
pub async fn reload_on_hangup(tls: Tls) {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};

        match signal(SignalKind::hangup()) {
            Ok(mut hangup) => {
                while hangup.recv().await.is_some() {
                    reload(tls.clone()).await;
                }
            }
            Err(error) => tracing::warn!(%error, "SIGHUP will not reload the certificate"),
        }
    }
    #[cfg(not(unix))]
    let _ = tls;
    std::future::pending().await
}

#[cfg(unix)]
async fn reload(tls: Tls) {
    if !tls.reloads() {
        tracing::info!("there are no certificate files to reload");
        return;
    }
    match tokio::task::spawn_blocking(move || tls.reload()).await {
        Ok(Ok(())) => tracing::info!("reloaded the certificate"),
        Ok(Err(error)) => tracing::error!(?error, "kept the previous certificate"),
        Err(error) => tracing::error!(%error, "kept the previous certificate"),
    }
}
