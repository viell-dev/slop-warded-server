//! What the operating system asks of the process.

use crate::Tls;

/// Completes when the process is asked to stop.
///
/// On Unix that is Ctrl+C (`SIGINT`) or `SIGTERM`. On Windows it is Ctrl+C, Ctrl+Break, closing
/// the console, or system shutdown.
///
/// Any number of tasks can wait for it, also beside [`Server::serve`](crate::Server::serve).
///
/// # Panics
///
/// When the operating system refuses to report signals, because the server could then only be
/// stopped by killing it.
pub async fn shutdown() {
    ShutdownSignals::new().next().await;
}

/// The signals that ask the process to stop, from when this is made.
pub(crate) struct ShutdownSignals {
    #[cfg(unix)]
    interrupt: tokio::signal::unix::Signal,
    #[cfg(unix)]
    terminate: tokio::signal::unix::Signal,
    #[cfg(windows)]
    interrupt: tokio::signal::windows::CtrlC,
    #[cfg(windows)]
    break_key: tokio::signal::windows::CtrlBreak,
    #[cfg(windows)]
    close: tokio::signal::windows::CtrlClose,
    #[cfg(windows)]
    system: tokio::signal::windows::CtrlShutdown,
}

impl ShutdownSignals {
    const EXPECTATION: &str = "the process can listen for shutdown signals";

    #[cfg(unix)]
    pub(crate) fn new() -> Self {
        use tokio::signal::unix::{SignalKind, signal};

        Self {
            interrupt: signal(SignalKind::interrupt()).expect(Self::EXPECTATION),
            terminate: signal(SignalKind::terminate()).expect(Self::EXPECTATION),
        }
    }

    #[cfg(windows)]
    pub(crate) fn new() -> Self {
        use tokio::signal::windows;

        Self {
            interrupt: windows::ctrl_c().expect(Self::EXPECTATION),
            break_key: windows::ctrl_break().expect(Self::EXPECTATION),
            close: windows::ctrl_close().expect(Self::EXPECTATION),
            system: windows::ctrl_shutdown().expect(Self::EXPECTATION),
        }
    }

    /// Wait for one more signal. One that arrived since the last wait counts.
    #[cfg(unix)]
    pub(crate) async fn next(&mut self) {
        tokio::select! {
            _ = self.interrupt.recv() => {}
            _ = self.terminate.recv() => {}
        }
    }

    /// Wait for one more signal. One that arrived since the last wait counts.
    #[cfg(windows)]
    pub(crate) async fn next(&mut self) {
        tokio::select! {
            _ = self.interrupt.recv() => {}
            _ = self.break_key.recv() => {}
            _ = self.close.recv() => {}
            _ = self.system.recv() => {}
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
