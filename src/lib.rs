//! A ready-wired server for HTTP/1.1, HTTP/2 and HTTP/3.
//!
//! [`Server`] runs one application over TLS on TCP and QUIC. An [`axum::Router`] or another
//! infallible Tower service is one:
//!
//! ```no_run
//! use warded_server::{Server, axum::{Router, routing::get}};
//!
//! #[tokio::main]
//! async fn main() -> Result<(), warded_server::Error> {
//!     let app = Router::new().route("/", get(|| async { "Hello" }));
//!     Server::from_env()?.serve(app).await
//! }
//! ```
//!
//! With the `warded` feature, which is on by default, `app` makes one of
//! [Warded](https://docs.rs/warded) routes.
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
//! - with the `warded` feature, `warded::TransportSecurity`: `Https` over TLS or QUIC and
//!   `Unsecure` on the cleartext listener and the development address. Headers from a proxy are
//!   never trusted.
//!
//! HTTP/2 and HTTP/3 requests have their host in the URI instead of a `Host` field.
//!
//! # Starting points
//!
//! Thin helpers over Axum, `tower-http` and `tracing-subscriber` set up what most projects need,
//! and stay out of the way of anything else:
//!
//! - [`Server::development`]: the application over plain HTTP for a browser on this computer,
//!   with pages that reload when the server restarts.
//! - [`init_logging`]: logging to the terminal, as `RUST_LOG` says.
//! - [`DefaultMiddleware`]: logging of requests, compression and a 500 for panics.
//! - [`StaticFiles`] and [`SinglePageApp`]: files from a directory.
//!
//! # Features
//!
//! All are enabled by default:
//!
//! - `files`: `StaticFiles` and, with `warded`, `SinglePageApp`.
//! - `http3`: HTTP/3 over QUIC. The underlying [`h3`](https://github.com/hyperium/h3) library is
//!   experimental and has
//!   [known weaknesses](https://viell-dev.github.io/slop-warded-server/limits.html#http-3).
//! - `logging`: `init_logging`.
//! - `middleware`: `DefaultMiddleware`.
//! - `self-signed`: `Tls::self_signed`, for development.
//! - `warded`: `app`, `Params` and the security context.

#[cfg(all(feature = "http3", panic = "abort"))]
compile_error!(
    "the `http3` feature needs `panic = \"unwind\"`: h3 0.0.8 is experimental and has panicked at \
     what clients send (https://github.com/hyperium/h3/issues/386), and aborting would stop the \
     whole server"
);

mod context;
mod development;
mod env;
mod error;
#[cfg(feature = "files")]
mod files;
#[cfg(feature = "http3")]
mod http3;
mod limits;
mod linger;
mod listener;
#[cfg(feature = "logging")]
mod logging;
#[cfg(feature = "middleware")]
mod middleware;
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
#[cfg(all(feature = "files", feature = "warded"))]
pub use files::SinglePageApp;
#[cfg(feature = "files")]
pub use files::StaticFiles;
pub use limits::Limits;
#[cfg(feature = "logging")]
pub use logging::init_logging;
#[cfg(feature = "middleware")]
pub use middleware::DefaultMiddleware;
#[cfg(feature = "warded")]
pub use routes::{Params, ParamsRejection, app};
pub use server::{BoundServer, Server};
pub use signal::{reload_on_hangup, shutdown};
pub use tls::Tls;

#[cfg(all(doctest, feature = "warded"))]
#[doc = include_str!("../README.md")]
struct ReadmeDoctests;
