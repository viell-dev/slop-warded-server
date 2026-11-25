//! Warded routes over HTTP/1.1, HTTP/2 and HTTP/3, configured by environment variables.
//!
//! ```sh
//! cargo run --example hello
//! curl --insecure --http1.1 https://localhost:8443/
//! curl --insecure --http2 https://localhost:8443/
//! curl --insecure --http3-only https://localhost:8443/
//! ```

use axum::{Extension, http::Version, routing::any};
use serde::Deserialize;
use warded::{
    Matcher, Router, TransportSecurity,
    matchers::{exact_path, get, path, version},
};
use warded_server::{Params, Server};

#[derive(Deserialize)]
struct Greeting {
    name: String,
}

async fn home(version: Version, Extension(security): Extension<TransportSecurity>) -> String {
    format!("Hello over {version:?} ({security:?})\n")
}

async fn greet(Params(greeting): Params<Greeting>) -> String {
    format!("Hello, {}\n", greeting.name)
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt::init();
    let routes = Router::new()
        .route(
            exact_path("/").and(version(Version::HTTP_3)),
            any(|| async { "Hello over HTTP/3, which has its own route\n" }),
        )
        .route(get().and(exact_path("/")), any(home))
        .route(get().and(path("/hello/{name}")?), any(greet));
    Server::from_env()?
        .serve(warded_server::app(routes))
        .await?;
    Ok(())
}
