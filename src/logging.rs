//! Logging to the terminal, from `tracing-subscriber`.

use std::io::IsTerminal;
use tracing_subscriber::{EnvFilter, filter::LevelFilter};

/// Print what the server and the application log, as the `RUST_LOG` environment variable says.
///
/// Without `RUST_LOG`, it prints the `info` level and above: the addresses to open, the requests
/// that `DefaultMiddleware` logs, warnings and errors. For more, set it
/// as [`EnvFilter`](https://docs.rs/tracing-subscriber/0.3/tracing_subscriber/filter/struct.EnvFilter.html)
/// describes, such as `RUST_LOG=info,warded_server=debug`. Parts of it that cannot be read are
/// left out with a warning.
///
/// This is `tracing-subscriber`'s formatter with that filter, on standard output. It has colours
/// only in a terminal, and not where `NO_COLOR` is set. Set up a subscriber of your own instead for
/// other output, such as JSON.
///
/// # Panics
///
/// If a global `tracing` subscriber has been set already, such as by an earlier call.
pub fn init_logging() {
    let filter = EnvFilter::builder()
        .with_default_directive(LevelFilter::INFO.into())
        .from_env_lossy();
    let logging = tracing_subscriber::fmt().with_env_filter(filter);
    if std::io::stdout().is_terminal() {
        logging.init();
    } else {
        // Not for a file or the journal, which would keep the escape codes.
        logging.with_ansi(false).init();
    }
}
