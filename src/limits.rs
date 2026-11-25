use std::time::Duration;

/// Bounds on what clients can make the server hold at once.
///
/// The defaults suit a small public site. Start from [`Limits::default`] and change fields:
///
/// ```
/// use std::time::Duration;
/// use warded_server::Limits;
///
/// let mut limits = Limits::default();
/// limits.connections = 2_000;
/// limits.head_timeout = Duration::from_secs(10);
/// ```
///
/// These bound the transport, not the application. Limit request bodies, handler time and
/// concurrent handlers with Tower middleware, because a JSON endpoint, an upload and an event
/// stream need different values.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct Limits {
    /// Open connections across every listener. Further ones are closed or refused.
    ///
    /// Default: 10 000.
    pub connections: usize,
    /// Concurrent requests on one HTTP/2 or HTTP/3 connection.
    ///
    /// Default: 128.
    pub concurrent_streams: u32,
    /// Size of one request head, in bytes.
    ///
    /// HTTP/2 and HTTP/3 count the decoded fields with 32 bytes for each, as their
    /// specifications define. HTTP/1.1 counts received bytes and rounds up to at least 8 KiB.
    ///
    /// Default: 32 KiB.
    pub head_bytes: u32,
    /// How long a client may take to finish a TLS or QUIC handshake.
    ///
    /// Default: 10 seconds.
    pub handshake_timeout: Duration,
    /// How long a client may take to send a request head.
    ///
    /// An idle HTTP/1.1 connection is waiting for a head too, so this also closes those.
    ///
    /// Default: 30 seconds.
    pub head_timeout: Duration,
    /// How long an HTTP/2 connection may leave a ping unanswered, and how long an HTTP/3
    /// connection may be silent, before it is closed.
    ///
    /// Default: 30 seconds.
    pub idle_timeout: Duration,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            connections: 10_000,
            concurrent_streams: 128,
            head_bytes: 32 * 1024,
            handshake_timeout: Duration::from_secs(10),
            head_timeout: Duration::from_secs(30),
            idle_timeout: Duration::from_secs(30),
        }
    }
}
