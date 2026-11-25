//! A ready-wired server for HTTP/1.1, HTTP/2 and HTTP/3.
//!
//! [`Server`] runs one application over TLS on TCP and QUIC, and [`app`] turns
//! [Warded](https://docs.rs/warded) routes into such an application. An ordinary
//! [`axum::Router`] or another infallible Tower service works as well.
//!
//! ```no_run
//! use warded_server::{
//!     Server,
//!     axum::routing::any,
//!     warded::{self, Matcher, matchers::{exact_path, get}},
//! };
//!
//! #[tokio::main]
//! async fn main() -> Result<(), warded_server::Error> {
//!     let routes = warded::Router::new()
//!         .route(get().and(exact_path("/")), any(|| async { "Hello" }));
//!     Server::from_env()?.serve(warded_server::app(routes)).await
//! }
//! ```
//!
//! # Guides
//!
//! The [documentation site](https://viell-dev.github.io/slop-warded-server/) explains each task with
//! complete examples, by kind of project.
//!
//! # What a request carries
//!
//! Every request has these extensions, whichever protocol carried it:
//!
//! - [`ConnectInfo<SocketAddr>`](axum::extract::ConnectInfo), the client's address;
//! - with the `warded` feature, [`warded::TransportSecurity`]: `Https` over TLS or QUIC and
//!   `Unsecure` on the cleartext listener. Headers from a proxy are never trusted.
//!
//! HTTP/2 and HTTP/3 requests have their host in the URI instead of a `Host` field.
//!
//! # Features
//!
//! All are enabled by default:
//!
//! - `http3`: HTTP/3 over QUIC. The underlying [`h3`](https://github.com/hyperium/h3) library is
//!   experimental and has known ways for a client to exhaust memory.
//! - `self-signed`: `Tls::self_signed`, for development.
//! - `warded`: `app`, `Params` and the security context.

#[cfg(all(feature = "http3", panic = "abort"))]
compile_error!(
    "the `http3` feature needs `panic = \"unwind\"`: a client can make h3 0.0.8 panic \
     (https://github.com/hyperium/h3/issues/386), and aborting would stop the whole server"
);

mod context;
mod env;
mod error;
#[cfg(feature = "http3")]
mod http3;
mod limits;
mod listener;
mod redirect;
#[cfg(feature = "warded")]
mod routes;
mod server;
mod signal;
mod tcp;
mod tls;

pub use axum;
#[cfg(feature = "warded")]
pub use warded;

pub use error::Error;
pub use limits::Limits;
#[cfg(feature = "warded")]
pub use routes::{Params, ParamsRejection, app};
pub use server::{BoundServer, Server};
pub use signal::{reload_on_hangup, shutdown};
pub use tls::Tls;

#[cfg(all(doctest, feature = "warded"))]
#[doc = include_str!("../README.md")]
struct ReadmeDoctests;
