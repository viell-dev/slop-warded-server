use crate::Error;
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
/// [`Server::bind`](crate::Server::bind) refuses zeros, timeouts of more than a year, and more than
/// 10 000 concurrent streams.
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
    /// Every HTTP/3 connection takes memory for this many when it opens, about 100 bytes each.
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
    /// How long a client may take to send a request head, on HTTP/2 only the first one of a
    /// connection.
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

impl Limits {
    /// Enough for "no limit" in practice, and safe to add to a point in time.
    const LONGEST_TIMEOUT: Duration = Duration::from_secs(365 * 24 * 60 * 60);

    /// QUIC prepares for every stream that a client may open when it connects.
    const MOST_STREAMS: u32 = 10_000;

    /// Refuse values with which the server could not serve anything, or only at great cost.
    pub(crate) fn validate(&self) -> Result<(), Error> {
        // QUIC counts whole milliseconds, and takes 0 for no timeout.
        let timeout = Duration::from_millis(1)..=Self::LONGEST_TIMEOUT;
        for (field, usable) in [
            ("connections", self.connections > 0),
            (
                "concurrent_streams",
                (1..=Self::MOST_STREAMS).contains(&self.concurrent_streams),
            ),
            ("head_bytes", self.head_bytes > 0),
            (
                "handshake_timeout",
                timeout.contains(&self.handshake_timeout),
            ),
            ("head_timeout", timeout.contains(&self.head_timeout)),
            ("idle_timeout", timeout.contains(&self.idle_timeout)),
        ] {
            if !usable {
                return Err(Error::Limit { field });
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    type Change = fn(&mut Limits);

    #[test]
    fn only_usable_limits_are_accepted() {
        assert!(Limits::default().validate().is_ok());
        let largest = Limits {
            concurrent_streams: Limits::MOST_STREAMS,
            handshake_timeout: Limits::LONGEST_TIMEOUT,
            head_timeout: Limits::LONGEST_TIMEOUT,
            idle_timeout: Limits::LONGEST_TIMEOUT,
            ..Limits::default()
        };
        assert!(largest.validate().is_ok());

        let changes: [(&str, Change); 10] = [
            ("connections", |limits| limits.connections = 0),
            ("concurrent_streams", |limits| limits.concurrent_streams = 0),
            ("concurrent_streams", |limits| {
                limits.concurrent_streams = u32::MAX
            }),
            ("head_bytes", |limits| limits.head_bytes = 0),
            ("handshake_timeout", |limits| {
                limits.handshake_timeout = Duration::ZERO
            }),
            ("handshake_timeout", |limits| {
                limits.handshake_timeout = Duration::MAX
            }),
            ("head_timeout", |limits| {
                limits.head_timeout = Duration::ZERO
            }),
            ("head_timeout", |limits| limits.head_timeout = Duration::MAX),
            ("idle_timeout", |limits| {
                limits.idle_timeout = Duration::from_micros(1)
            }),
            ("idle_timeout", |limits| limits.idle_timeout = Duration::MAX),
        ];
        for (name, change) in changes {
            let mut limits = Limits::default();
            change(&mut limits);
            let result = limits.validate();
            assert!(matches!(result, Err(Error::Limit { field }) if field == name));
        }
    }
}
