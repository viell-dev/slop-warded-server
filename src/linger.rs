//! How long to take what a client still sends when it has its response.
//!
//! A client that is cut off while it sends can miss the response.

use std::time::Duration;

#[derive(Clone, Copy)]
pub(crate) struct Linger {
    /// How long the client may be silent.
    pub(crate) silence: Duration,
    /// How long it may take, however much it sends.
    pub(crate) longest: Duration,
}

pub(crate) const LINGER: Linger = Linger {
    silence: Duration::from_secs(5),
    longest: Duration::from_secs(30),
};
