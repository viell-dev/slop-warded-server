---
layout: home
description: "A ready-wired Rust server for HTTP/1.1, HTTP/2 and HTTP/3, with the Warded router and project templates."
hero:
  name: Warded Server
  text: Start at your routes
  tagline: A ready-wired Rust server for HTTP/1.1, HTTP/2 and HTTP/3, with the Warded router and templates to begin from.
  actions:
    - theme: brand
      text: Get started
      link: /getting-started
    - theme: alt
      text: Build a JSON API
      link: /json-api
    - theme: alt
      text: Build a website
      link: /website
features:
  - title: Three protocols, one address
    details: HTTP/1.1 and HTTP/2 over TLS, and HTTP/3 over QUIC on the same port, from one certificate and with one graceful shutdown.
  - title: The ecosystem you know
    details: Handlers, extractors and middleware come from Axum and Tower. An existing Axum application runs unchanged.
  - title: Routes that see the whole request
    details: Warded selects on method, path, headers, language, HTTP version and whether the connection is secure, in the order you declare.
---

Create a project, choose a template and run it:

```sh
cargo install cargo-generate --locked
cargo generate viell-dev/slop-warded-server --tag 0.1.0-alpha.2
cd my-project
cargo run
```

A whole server is this much code:

```rust,no_run
use axum::{http::Version, routing::any};
use warded_server::{
    Server,
    warded::{
        Matcher, Router,
        matchers::{exact_path, get, version},
    },
};

#[tokio::main]
async fn main() -> Result<(), warded_server::Error> {
    let routes = Router::new()
        .route(
            exact_path("/").and(version(Version::HTTP_3)),
            any(|| async { "You are on HTTP/3" }),
        )
        .route(get().and(exact_path("/")), any(|| async { "Hello" }));
    Server::from_env()?.serve(warded_server::app(routes)).await
}
```

[Get started](./getting-started.md) to make a project, or see what to build:
a [JSON API](./json-api.md), a [website](./website.md), a [single-page app](./single-page-app.md)
with its API, or [an existing Axum application](./existing-axum.md) with more protocols.

::: warning Alpha
Everything may still change, and HTTP/3 rests on an experimental library with [known
weaknesses](./limits.md#http-3). Do not put it on the open internet yet. Agents write and maintain
this project, and no person reviews the code.
:::
