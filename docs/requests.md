---
description: "Declare Warded routes to Axum handlers, read path captures, and see what a request carries on each protocol."
---

# Routes and requests

A route is a condition on the request and a destination. This page shows the parts you need to write
them for this server. [Warded's guides](https://viell-dev.github.io/slop-warded-router/) explain
every condition in full.

## Declaring routes

If the project does not have Serde yet, add it for the `Deserialize` derive:

```sh
cargo add serde --features derive
```

The checks also use the [shared example dependencies](./getting-started.md#about-the-examples).

```rust
use axum::{
    body::Body,
    http::{Request, StatusCode},
    routing::any,
};
use http_body_util::BodyExt;
use serde::Deserialize;
use tower::ServiceExt;
use warded_server::{
    Params,
    warded::{
        DefinitionError, Matcher, Router,
        matchers::{exact_path, get, head, path, post},
    },
};

#[derive(Deserialize)]
struct OrderParams {
    id: u64,
}

async fn list_orders() -> &'static str {
    "all orders"
}

async fn create_order(body: String) -> (StatusCode, String) {
    (StatusCode::CREATED, format!("created {body}"))
}

async fn show_order(Params(order): Params<OrderParams>) -> String {
    format!("order {}", order.id)
}

fn app() -> Result<axum::Router, DefinitionError> {
    let routes = Router::new()
        .route(
            get().or(head()).and(exact_path("/orders")),
            any(list_orders),
        )
        .route(post().and(exact_path("/orders")), any(create_order))
        .route(get().and(path("/orders/{id}")?), any(show_order));
    Ok(warded_server::app(routes))
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    for (method, uri, status, text) in [
        ("GET", "/orders", StatusCode::OK, "all orders"),
        ("HEAD", "/orders", StatusCode::OK, ""),
        ("POST", "/orders", StatusCode::CREATED, "created tea"),
        ("GET", "/orders/7", StatusCode::OK, "order 7"),
        // No route has these conditions.
        ("DELETE", "/orders/7", StatusCode::NOT_FOUND, ""),
        ("GET", "/orders/", StatusCode::NOT_FOUND, ""),
    ] {
        let request = Request::builder()
            .method(method)
            .uri(uri)
            .body(Body::from("tea"))?;
        let response = app()?.oneshot(request).await?;
        assert_eq!(response.status(), status, "{method} {uri}");
        assert_eq!(response.into_body().collect().await?.to_bytes(), text);
    }

    // A capture that does not parse is the client's mistake.
    let request = Request::builder().uri("/orders/seven").body(Body::empty())?;
    let response = app()?.oneshot(request).await?;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    Ok(())
}
```

- **Order matters.** The first route whose conditions all hold is selected, and its response is
  final. Put a fixed path above any pattern that would also match it.
- **`get()` does not include HEAD.** Write `get().or(head())` where clients should be able to ask
  for the head alone; Axum then leaves out the body.
- **Paths are exact.** `/orders` and `/orders/` are different, and `path` fails when the pattern is
  wrong, so definition errors appear at start. Captures are written `{id}` and need `path`;
  `exact_path("/orders/{id}")` and `path("/orders/:id")` are literal text that nobody asks for.
- **Nothing selected is a 404.** For 405 responses, let an Axum method router handle the methods:
  `.route(exact_path("/orders"), axum::routing::get(list_orders).post(create_order))`.

## Destinations

A destination is any Tower service with Axum's request and response types. In practice it is one of:

| Destination                               | Use it for                                         |
| ----------------------------------------- | -------------------------------------------------- |
| `any(handler)`                            | One handler                                        |
| `any(handler).with_state(state)`          | One handler that uses `State`                      |
| `handler.into_service()`                  | One handler, whose [response to HEAD](./middleware.md#the-response-to-head) layers can change |
| `axum::routing::get(a).post(b)`           | One path with several methods, and a 405 for others |
| `axum::Router`                            | A whole Axum application, or a Tower service in one |

Inside handlers, use Axum's extractors. One differs: Axum's `Path` does not see Warded's captures,
so use `Params`. It takes a struct whose fields are the names in the pattern, also for a single
capture, and decodes the values. If the names differ, every request gets a 400.

`get` and `post` are conditions in `warded::matchers` and functions in `axum::routing`. Where you
need both, import one under another name, or write `axum::routing::get`.

## What a request carries

Handlers and routes see the same things on every protocol:

| To learn                   | In a handler                                  | In a route                                         |
| -------------------------- | --------------------------------------------- | -------------------------------------------------- |
| The protocol               | `axum::http::Version`                         | `version(Version::HTTP_3)`                         |
| The client's address       | `ConnectInfo<SocketAddr>`                     | a [custom condition][custom], which cannot see it  |
| Whether it came over HTTPS | `Extension<TransportSecurity>`                | `secured()` and `unsecure()`                       |
| The client's languages     | the `Accept-Language` field                   | [negotiation][negotiation]                         |
| The host                   | see below                                     | a [host condition][host]                           |

```rust
use axum::{
    Extension,
    extract::ConnectInfo,
    http::Version,
    routing::any,
};
use std::net::SocketAddr;
use warded_server::warded::{Router, TransportSecurity, matchers::exact_path};

async fn describe(
    version: Version,
    ConnectInfo(client): ConnectInfo<SocketAddr>,
    Extension(security): Extension<TransportSecurity>,
) -> String {
    format!("{version:?} from {client}, {security:?}")
}

fn main() {
    let routes = Router::new().route(exact_path("/"), any(describe));
    let _app = warded_server::app(routes);
}
```

The security context is what the server saw: `Https` over TLS and QUIC, and `Unsecure` on the
cleartext listener. It never comes from a header.

The host is the one difference between protocols that a handler notices. HTTP/1.1 clients name it in
the `Host` field, and HTTP/2 and HTTP/3 clients in the URI. Check `uri.host()` first, and `Host`
when there is none.

## When a condition fails

A condition can be unable to decide. `warded_server::app` turns that into a response: 400 for a
malformed header that a route declared to be an error, and otherwise 500 with the reason in the log.
To choose other responses, use `HandleError` yourself as in [Warded's
guide](https://viell-dev.github.io/slop-warded-router/tower.html#map-errors-to-responses), and serve
the result.

[custom]: https://viell-dev.github.io/slop-warded-router/custom-matchers.html
[host]: https://viell-dev.github.io/slop-warded-router/custom-matchers.html#a-host-condition-with-match-fn
[negotiation]: https://viell-dev.github.io/slop-warded-router/negotiation.html
