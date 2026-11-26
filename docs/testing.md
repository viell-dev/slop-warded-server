---
description: "Test an application without a network, and over real connections of each protocol."
---

# Testing

## Your first test

In a project from `api` or `website`, open `src/routes/tests.rs`. Copy a test that sends a
similar request, give it a new function name, and change the method, path and expected response
to match your route. Run `cargo test` from the project directory.

In a project from `minimal`, add the libraries you need for tests:

```sh
cargo add --dev tower --features util
cargo add --dev http-body-util
```

In `src/main.rs`, move the route declarations and `warded_server::app(routes)` into
`fn app()`. Call `app()` from both `main` and the test. This listing shows the starter route
and the first test. Keep any routes you already added, and use your own greeting in the check.

```rust,no_run
use axum::routing::any;
use warded_server::{
    Server,
    warded::{
        Matcher, Router,
        matchers::{exact_path, get},
    },
};

async fn home() -> &'static str {
    "Hello\n"
}

fn app() -> axum::Router {
    let routes = Router::new().route(get().and(exact_path("/")), any(home));
    warded_server::app(routes)
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    warded_server::init_logging();
    Server::from_env()?.serve(app()).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::app;
    use axum::{
        body::Body,
        http::{Request, StatusCode},
    };
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    #[tokio::test]
    async fn home_says_hello() -> Result<(), Box<dyn std::error::Error>> {
        let request = Request::get("/").body(Body::empty())?;
        let response = app().oneshot(request).await?;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.into_body().collect().await?.to_bytes(), "Hello\n");
        Ok(())
    }
}
```

Run `cargo test`. You should see `home_says_hello ... ok`.

If a route uses `path(...)?`, give `app` a `Result` return type as in
[Declaring routes](./requests.md#declaring-routes), and call it as `app()?` in both places.

## Without a network

An application is a Tower service, so a test can call it with a request and check the response. This
is fast, and enough for routes and handlers. The templates' tests work this way:

```rust
use axum::{
    Router,
    body::Body,
    http::{Request, StatusCode, Version},
    routing::any,
};
use tower::ServiceExt;
use warded_server::warded::{
    self, Matcher, TransportSecurity,
    matchers::{exact_path, secured, version},
};

fn app() -> Router {
    let routes = warded::Router::new().route(
        exact_path("/")
            .and(version(Version::HTTP_3))
            .and(secured()),
        any(|| async { "secured HTTP/3" }),
    );
    warded_server::app(routes)
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut request = Request::builder()
        .uri("/")
        .version(Version::HTTP_3)
        .body(Body::empty())?;
    // The server adds this to real requests.
    request.extensions_mut().insert(TransportSecurity::Https);
    let response = app().oneshot(request).await?;
    assert_eq!(response.status(), StatusCode::OK);
    Ok(())
}
```

What the server adds to requests is missing here. Insert `TransportSecurity` for routes with
`secured()` or `unsecure()`, which otherwise fail with a 500, and `ConnectInfo(address)` for
handlers that take the client's address. What it does to responses is missing too: the content of a
response to HEAD can be there, and `Alt-Svc` and `Date` are not.

## Over real connections

Bind to port 0, which picks a free one, and stop the server when the test ends. With
[reqwest](https://docs.rs/reqwest), which uses HTTP/2 here:

```sh
cargo add --dev reqwest --features http2
cargo add tokio --features sync
```

```rust
use axum::{Router, http::Version, routing::get};
use tokio::sync::oneshot;
use warded_server::{Server, Tls};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let app = Router::new().route("/", get(|version: Version| async move {
        format!("{version:?}")
    }));
    let tls = Tls::self_signed(["localhost"])?;
    let bound = Server::new(tls).https(([127, 0, 0, 1], 0).into()).bind()?;
    let address = bound.https_addr();
    let (stop, stopped) = oneshot::channel::<()>();
    let serving = tokio::spawn(bound.serve_until(app, async {
        let _ = stopped.await;
    }));

    let client = reqwest::Client::builder()
        // The certificate was made above. Do this in tests only.
        .danger_accept_invalid_certs(true)
        .build()?;
    let response = client.get(format!("https://{address}/")).send().await?;
    assert_eq!(response.text().await?, "HTTP/2.0");

    drop(client);
    let _ = stop.send(());
    serving.await??;
    Ok(())
}
```

## By hand

Use a [curl build that supports HTTP/3](./getting-started.md#shell-and-curl).

```sh
curl --insecure --include --http3-only https://localhost:8443/
```

See [HTTP/3](./http3.md#check-that-it-works) for browsers. For HTTP/3 in automated tests, run curl,
or use the `h3` and `h3-quinn` crates as [this project's own
tests](https://github.com/viell-dev/slop-warded-server/blob/main/tests/common/mod.rs) do.
