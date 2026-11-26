---
description: "Serve an Axum application over HTTP/1.1, HTTP/2 and HTTP/3, and put Warded routes in front of it where they help."
---

# Existing Axum applications

## Serve it

Add the dependency:

```toml
[dependencies]
warded-server = { git = "https://github.com/viell-dev/slop-warded-server", tag = "0.1.0-alpha.2" }
```

Replace the listener and `axum::serve` with `Server`:

```diff
-let listener = tokio::net::TcpListener::bind("0.0.0.0:3000").await?;
-axum::serve(listener, app).await?;
+warded_server::Server::from_env()?.serve(app).await?;
```

The application moves with that: by default it is now at `https://localhost:8443`, for this computer
only, with a certificate that nothing trusts. [Configuration](./configuration.md) and
[certificates](./certificates.md) change that.

The server tells its address, and what is wrong, through `tracing`. If the application has no
subscriber yet, add one, or nothing is shown. `init_logging` prints to the terminal, as `RUST_LOG`
says:

```rust
fn main() {
    warded_server::init_logging();
    // Then run the server.
}
```

To open it in a browser without a certificate, give it a [development
address](./configuration.md#the-development-address) as well.

The application needs no other changes. Its state, extractors, layers, 404s and 405s stay as they
are. A few things work differently underneath:

- **`ConnectInfo<SocketAddr>` is always there**, so drop `into_make_service_with_connect_info`,
  which `serve` does not accept.
- **Shutdown is built in**, so drop `with_graceful_shutdown`. To decide it yourself, use
  [`serve_until`](#decide-when-to-stop).
- **Handlers can see HTTP/2 and HTTP/3**, where the host is in the URI and there is no `Host` field.
  Read `uri.host()` first, and then `Host`.
- **WebSockets keep working over HTTP/1.1**, which browsers use for them.

Any other Tower service works too, if its requests and responses have Axum's types and its error is
`Infallible`. To leave Warded out of the build, turn off the `warded` feature.

## Put Warded in front

Warded can decide by anything in the request head and hand the rest to the application. Here one
customer tries a new version, and the HTTP/3 clients get a hint, without changing either
application:

```rust
use axum::{Router, body::Body, http::Request, routing::get};
use http_body_util::BodyExt;
use tower::ServiceExt;
use warded_server::warded::{
    self, Matcher,
    matchers::{exact_path, header_eq},
};

fn existing() -> Router {
    Router::new()
        .route("/", get(|| async { "current" }))
        .route("/about", get(|| async { "about" }))
}

fn next() -> Router {
    Router::new().route("/", get(|| async { "next" }))
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let routes = warded::Router::new()
        .route(
            exact_path("/").and(header_eq("x-customer", "early-bird")?),
            next(),
        )
        .fallback(existing());
    let app = warded_server::app(routes);

    for (uri, customer, expected) in [
        ("/", "early-bird", "next"),
        ("/", "someone", "current"),
        ("/about", "early-bird", "about"),
    ] {
        let request = Request::builder()
            .uri(uri)
            .header("x-customer", customer)
            .body(Body::empty())?;
        let response = app.clone().oneshot(request).await?;
        assert_eq!(response.into_body().collect().await?.to_bytes(), expected);
    }
    Ok(())
}
```

The fallback keeps all of the application's own behavior, so you can move routes over one at a time,
or never.

## Decide when to stop

`serve` listens for signals. `bind` and `serve_until` take that decision, and give you the
addresses:

```rust,no_run
use axum::{Router, routing::get};
use warded_server::{Server, Tls};

#[tokio::main]
async fn main() -> Result<(), warded_server::Error> {
    let app = Router::new().route("/", get(|| async { "Hello" }));
    let tls = Tls::pem_files("fullchain.pem", "privkey.pem")?;
    let server = Server::new(tls.clone()).https(([0, 0, 0, 0], 443).into());

    let bound = server.bind()?;
    println!("ready on {}", bound.https_addr());
    // Anything else that starts after the ports are taken, such as dropping privileges, goes here.
    // Without this, SIGHUP would end the process.
    tokio::spawn(warded_server::reload_on_hangup(tls));
    bound
        .serve_until(app, async {
            warded_server::shutdown().await;
            println!("stopping");
        })
        .await
}
```

`serve` reloads certificate files on `SIGHUP`. With `serve_until`, **`SIGHUP` ends the process**
unless you spawn `reload_on_hangup`, as here, or handle the signal yourself and call `Tls::reload`.
A second Ctrl+C does nothing here, either, so keep the grace period short enough.
