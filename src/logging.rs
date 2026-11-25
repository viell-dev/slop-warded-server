//! Logging to the terminal, from `tracing-subscriber`.

use tracing_subscriber::{EnvFilter, filter::LevelFilter};

/// Print what the server and the application log, as the `RUST_LOG` environment variable says.
///
/// Without `RUST_LOG`, it prints the `info` level and above: the addresses to open, the requests
/// that [`DefaultMiddleware`](crate::DefaultMiddleware) logs, warnings and errors. For more, set it
/// as [`EnvFilter`](https://docs.rs/tracing-subscriber/0.3/tracing_subscriber/filter/struct.EnvFilter.html)
/// describes, such as `RUST_LOG=info,warded_server=debug`. Parts of it that cannot be read are
/// left out with a warning.
///
/// This is `tracing-subscriber`'s formatter with that filter. Set up a subscriber of your own
/// instead for other output, such as JSON.
///
/// # Panics
///
/// If a global `tracing` subscriber has been set already, such as by an earlier call.
pub fn init_logging() {
    let filter = EnvFilter::builder()
        .with_default_directive(LevelFilter::INFO.into())
        .from_env_lossy();
    tracing_subscriber::fmt().with_env_filter(filter).init();
}
