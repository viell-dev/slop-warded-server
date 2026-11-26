---
description: "Add logging, compression, panic handling, timeouts, body limits and security headers with Tower layers."
---

# Middleware

`warded_server::app` returns an `axum::Router`, so any Tower layer for Axum works. The `api` and
`website` templates start with `DefaultMiddleware`:

```rust
use axum::{body::Body, handler::HandlerWithoutStateExt, http::Request};
use tower::ServiceExt;
use warded_server::{
    DefaultMiddleware,
    warded::{
        Matcher, Router,
        matchers::{exact_path, get},
    },
};

async fn broken() -> &'static str {
    panic!("a mistake")
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let routes = Router::new().route(get().and(exact_path("/")), broken.into_service());
    let app = warded_server::app(routes).layer(DefaultMiddleware::new());

    let request = Request::builder().uri("/").body(Body::empty())?;
    let response = app.oneshot(request).await?;
    assert_eq!(response.status(), 500);
    Ok(())
}
```

It is these layers from `tower-http`, from the outside in:

- **`TraceLayer`** logs each request at the `info` level, with its method, URI, protocol, status and
  duration, and server errors at `error`.
- **`CompressionLayer`** compresses responses with Brotli or gzip for clients that accept them. Give
  it [handlers as services](#the-response-to-head), as here and in the templates.
- **`CatchPanicLayer`** turns a panic into a 500, and logs it. Without it the request fails without
  a response; other requests are not affected either way. `on_panic` chooses the response, as the
  `api` template does for a JSON error and the `website` template for an error page:
  `DefaultMiddleware::new().on_panic(problem::panicked)`.

### The same by hand

To choose other layers, or another order, use them yourself:

```sh
cargo add tower-http --features catch-panic,compression-br,compression-gzip,trace
```

```rust
use axum::{body::Body, handler::HandlerWithoutStateExt, http::Request};
use tower::{ServiceBuilder, ServiceExt};
use tower_http::{catch_panic::CatchPanicLayer, compression::CompressionLayer, trace::TraceLayer};
use warded_server::warded::{
    Matcher, Router,
    matchers::{exact_path, get},
};

async fn broken() -> &'static str {
    panic!("a mistake")
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let routes = Router::new().route(get().and(exact_path("/")), broken.into_service());
    let middleware = ServiceBuilder::new()
        .layer(TraceLayer::new_for_http())
        .layer(CompressionLayer::new())
        .layer(CatchPanicLayer::new());
    let app = warded_server::app(routes).layer(middleware);

    let request = Request::builder().uri("/").body(Body::empty())?;
    let response = app.oneshot(request).await?;
    assert_eq!(response.status(), 500);
    Ok(())
}
```

`TraceLayer::new_for_http()` logs at the `debug` level, so its requests show with
`RUST_LOG=info,tower_http=debug`.

## Where to put a layer

- On **the router that `app` returns**, it runs for every request, before a route is selected. It
  also sees requests that no route selects, and can change what the routes see.
- On **a destination**, as in `any(handler).layer(…)`, it runs when that route is selected.

## The response to HEAD

A response to HEAD has the head that GET would get, and no content. `any(handler)` takes the content
off itself, so a layer on the router no longer sees it: `CompressionLayer` leaves the response alone,
and its `Content-Length` is not the one for GET. `handler.into_service()`, or
`handler.with_state(state)`, keeps the content, and the server takes it off after all layers. Both
need a trait from `axum::handler`: `HandlerWithoutStateExt` or `Handler`.

Use them for routes that answer HEAD inside such a layer. Files from `ServeDir` have no content for
HEAD in any case, and their `Content-Length` is that of the file, also where GET gets it compressed.

## Timeouts

The server limits how long a client may take to send a request head, and leaves the rest to you,
because an event stream and a JSON endpoint need different limits. `TimeoutLayer` limits the time
until the response begins, so it does not end streams:

```sh
cargo add tower-http --features timeout
cargo add tokio --features time
```

```rust
use axum::{
    body::Body,
    http::{Request, StatusCode},
    routing::any,
};
use std::time::Duration;
use tower::ServiceExt;
use tower_http::timeout::TimeoutLayer;
use warded_server::warded::{
    Matcher, Router,
    matchers::{exact_path, get},
};

async fn slow() {
    tokio::time::sleep(Duration::from_secs(60)).await;
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let limit = Duration::from_millis(20);
    let timeout = TimeoutLayer::with_status_code(StatusCode::SERVICE_UNAVAILABLE, limit);
    let routes = Router::new().route(
        get().and(exact_path("/slow")),
        any(slow).layer(timeout),
    );
    let request = Request::builder().uri("/slow").body(Body::empty())?;
    let response = warded_server::app(routes).oneshot(request).await?;
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    Ok(())
}
```

## Busy handlers

The server limits connections, and requests on one connection. To limit how many requests use
something expensive at the same time, put `tower::limit::GlobalConcurrencyLimitLayer` on those
destinations, and create it once so that they share it. It needs the `limit` feature of `tower`.

## Security headers

Nothing is added by default, because some of them are hard to take back. Common ones:

```sh
cargo add tower-http --features set-header
```

```rust
use axum::{
    body::Body,
    http::{HeaderName, HeaderValue, Request, header},
    routing::any,
};
use tower::{ServiceBuilder, ServiceExt};
use tower_http::set_header::SetResponseHeaderLayer;
use warded_server::warded::{
    Matcher, Router,
    matchers::{exact_path, get},
};

fn set(name: HeaderName, value: &'static str) -> SetResponseHeaderLayer<HeaderValue> {
    SetResponseHeaderLayer::if_not_present(name, HeaderValue::from_static(value))
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let headers = ServiceBuilder::new()
        .layer(set(header::X_CONTENT_TYPE_OPTIONS, "nosniff"))
        .layer(set(header::CONTENT_SECURITY_POLICY, "default-src 'self'"))
        .layer(set(header::REFERRER_POLICY, "same-origin"));
    let routes = Router::new().route(get().and(exact_path("/")), any(|| async { "page" }));
    let app = warded_server::app(routes).layer(headers);

    let request = Request::builder().uri("/").body(Body::empty())?;
    let response = app.oneshot(request).await?;
    assert_eq!(response.headers()["x-content-type-options"], "nosniff");
    Ok(())
}
```

Add them after `DefaultMiddleware`, as in `app.layer(DefaultMiddleware::new()).layer(headers)`, so
that they are outside it, and the responses to panics have them too.

`Strict-Transport-Security` tells browsers to use only HTTPS with your host name for as long as it
says. Add it once HTTPS works for good, and never while you develop on `localhost`, where it would
apply to every other project on your computer. The layer also covers the redirect from HTTP only
when the cleartext listener serves your application.
