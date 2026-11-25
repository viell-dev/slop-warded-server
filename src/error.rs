use std::{io, net::SocketAddr, path::PathBuf};

/// Why a server could not be configured, started or kept running.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// A listener could not be bound to its address.
    #[error("cannot listen on {address} for {listener}")]
    Bind {
        /// What the listener was for, such as `HTTPS over TCP`.
        listener: &'static str,
        /// The requested address.
        address: SocketAddr,
        /// The operating system's reason.
        source: io::Error,
    },
    /// A certificate or private key file could not be read.
    #[error("cannot read {}", path.display())]
    Read {
        /// The file that was read.
        path: PathBuf,
        /// Why reading failed.
        source: rustls::pki_types::pem::Error,
    },
    /// A certificate and key are unusable, or do not belong together.
    #[error("the certificate and private key cannot be used")]
    Certificate(#[source] rustls::Error),
    /// A development certificate could not be generated.
    #[cfg(feature = "self-signed")]
    #[error("cannot generate a development certificate")]
    Generate(#[source] rcgen::Error),
    /// The TLS configuration was refused.
    #[error("cannot configure TLS")]
    Tls(#[source] rustls::Error),
    /// An environment variable read by [`Server::from_env`](crate::Server::from_env) is invalid.
    #[error("{variable}: {reason}")]
    Environment {
        /// The variable's name.
        variable: &'static str,
        /// What is wrong with its value.
        reason: String,
    },
    /// A listener stopped while the server was running.
    #[error("the {listener} listener stopped")]
    Listener {
        /// Which listener stopped.
        listener: &'static str,
        /// Why it stopped.
        source: io::Error,
    },
}
