//! What the operating system asks of the process.

use crate::Tls;
use tokio_util::task::AbortOnDropHandle;

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

/// Reload certificate files on `SIGHUP`, for as long as the returned handle lives.
#[cfg(unix)]
pub(crate) fn reload_on_hangup(tls: &Tls) -> Option<AbortOnDropHandle<()>> {
    use tokio::signal::unix::{SignalKind, signal};

    if !tls.reloads() {
        return None;
    }
    let mut hangup = match signal(SignalKind::hangup()) {
        Ok(hangup) => hangup,
        Err(error) => {
            tracing::warn!(%error, "SIGHUP will not reload the certificate");
            return None;
        }
    };
    let tls = tls.clone();
    Some(AbortOnDropHandle::new(tokio::spawn(async move {
        while hangup.recv().await.is_some() {
            let tls = tls.clone();
            match tokio::task::spawn_blocking(move || tls.reload()).await {
                Ok(Ok(())) => tracing::info!("reloaded the certificate"),
                Ok(Err(error)) => tracing::error!(?error, "kept the previous certificate"),
                Err(error) => tracing::error!(%error, "kept the previous certificate"),
            }
        }
    })))
}

#[cfg(not(unix))]
pub(crate) fn reload_on_hangup(_: &Tls) -> Option<AbortOnDropHandle<()>> {
    None
}
