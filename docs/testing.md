---
description: "Test an application without a network, and over real connections of each protocol."
---

# Testing

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
handlers that take the client's address.

## Over real connections

Bind to port 0, which picks a free one, and stop the server when the test ends. With
[reqwest](https://docs.rs/reqwest), which uses HTTP/2 here:

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

```sh
curl --insecure --include --http3-only https://localhost:8443/
```

See [HTTP/3](./http3.md#check-that-it-works) for browsers. For HTTP/3 in automated tests, run curl,
or use the `h3` and `h3-quinn` crates as [this project's own
tests](https://github.com/viell-dev/slop-warded-server/blob/main/tests/common/mod.rs) do.
