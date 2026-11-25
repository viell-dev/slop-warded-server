---
description: "Serve a built React, Vue, Svelte, Angular or WebAssembly app together with its JSON API."
---

# A single-page app

A single-page app is a set of files that a build tool writes: an `index.html`, and scripts and
styles with generated names. The browser runs it and asks an API for data. This page serves both
from one server. It works for Vite (React, Vue, Svelte), Angular, Trunk (Yew, Leptos in the browser)
and others that produce such files. It does not run JavaScript on the server.

## The server

Start from the [API template](./json-api.md), build the app into a directory, and add three kinds of
routes, in this order:

1. the API, which answers its own 404s;
2. the built files;
3. `index.html` for every other page that a browser opens, because the app has its own routes.

The file services need two more features of `tower-http`, and the checks at the end two crates:

```sh
cargo add tower-http --features fs,set-header
cargo add --dev http-body-util tempfile
```

The whole application:

```rust
use axum::{
    Json, Router,
    body::Body,
    http::{HeaderValue, Request, StatusCode, header},
    response::Response,
    routing::{any, get as axum_get},
};
use http_body_util::BodyExt;
use std::path::Path;
use tower::ServiceExt;
use tower_http::{
    services::{ServeDir, ServeFile},
    set_header::SetResponseHeaderLayer,
};
use warded_server::warded::{
    self, DefinitionError, Matcher, match_fn,
    matchers::{get, head},
    negotiation::Offers,
};

/// Whether the path is this one or below it.
fn under(prefix: &'static str) -> impl Matcher + Clone {
    match_fn(move |head| {
        let rest = head.uri().path().strip_prefix(prefix);
        rest.is_some_and(|rest| rest.is_empty() || rest.starts_with('/'))
    })
}

fn app(dist: &Path) -> Result<Router, DefinitionError> {
    let get_or_head = || get().or(head());

    let api = Router::new().route("/api/hello", axum_get(|| async { Json("hello") }));

    // Vite puts the files with generated names in `assets`. They never change.
    let forever = HeaderValue::from_static("public, max-age=31536000, immutable");
    let assets = Router::new()
        .nest_service("/assets", ServeDir::new(dist.join("assets")))
        .layer(SetResponseHeaderLayer::if_not_present(
            header::CACHE_CONTROL,
            // Not for missing files, which a later build may bring back.
            move |response: &Response| {
                let found = response.status().is_success()
                    || response.status() == StatusCode::NOT_MODIFIED;
                found.then(|| forever.clone())
            },
        ));

    // Browsers must ask whether this one changed, to find new names after an update.
    let revalidate = HeaderValue::from_static("no-cache");
    // Caches must not give this page, or its absence, to a request that prefers something else.
    let vary = HeaderValue::from_static("accept");
    let index = Router::new()
        .fallback_service(ServeFile::new(dist.join("index.html")))
        .layer(SetResponseHeaderLayer::overriding(
            header::CACHE_CONTROL,
            revalidate,
        ))
        .layer(SetResponseHeaderLayer::appending(header::VARY, vary));
    let not_found = any(|| async { (StatusCode::NOT_FOUND, [(header::VARY, "accept")]) });

    // Browsers prefer HTML when they open a page. Requests from scripts, and for scripts, styles
    // and images, take anything, so they choose the first offer.
    let preferred = Offers::media_types(["application/octet-stream", "text/html"])?;
    let routes = warded::Router::new()
        .route(under("/api"), api)
        .route(get_or_head().and(under("/assets")), assets)
        .route(get_or_head().and(preferred.selected("text/html")?), index)
        .fallback(not_found);
    Ok(warded_server::app(routes))
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let dist = tempfile::tempdir()?;
    std::fs::write(dist.path().join("index.html"), "<!doctype html>")?;
    std::fs::create_dir(dist.path().join("assets"))?;
    std::fs::write(dist.path().join("assets/app.1a2b.js"), "start()")?;

    let app = app(dist.path())?;
    let browser = "text/html,application/xhtml+xml,*/*;q=0.8";
    for (uri, accept, status, body) in [
        ("/", browser, StatusCode::OK, "<!doctype html>"),
        ("/settings/profile", browser, StatusCode::OK, "<!doctype html>"),
        ("/assets/app.1a2b.js", "*/*", StatusCode::OK, "start()"),
        ("/api/hello", "application/json", StatusCode::OK, "\"hello\""),
        // A mistake in the app's requests must not look like a page.
        ("/api/unknown", "application/json", StatusCode::NOT_FOUND, ""),
        ("/assets/old.js", "*/*", StatusCode::NOT_FOUND, ""),
        ("/api", browser, StatusCode::NOT_FOUND, ""),
        ("/assets/", browser, StatusCode::NOT_FOUND, ""),
        ("/settings", "application/json", StatusCode::NOT_FOUND, ""),
        ("/settings", "*/*", StatusCode::NOT_FOUND, ""),
    ] {
        let request = Request::builder()
            .uri(uri)
            .header(header::ACCEPT, accept)
            .body(Body::empty())?;
        let response = app.clone().oneshot(request).await?;
        assert_eq!(response.status(), status, "{uri}");
        if uri.starts_with("/settings") {
            assert_eq!(response.headers()[header::VARY], "accept", "{uri}");
        }
        if uri.starts_with("/assets/") {
            let cached = response.headers().contains_key(header::CACHE_CONTROL);
            assert_eq!(cached, status == StatusCode::OK, "{uri}");
        }
        assert_eq!(response.into_body().collect().await?.to_bytes(), body);
    }
    Ok(())
}
```

The last route is limited to requests that prefer HTML. Without that, a wrong address in the app's
code gets `index.html` with a 200, which is hard to find. Programs such as curl, which take
anything, get a 404 too, unless they send `Accept: text/html`.

`app` goes into `src/routes.rs`, and `main` calls it with the directory of the build. The checks in
the `main` above belong in a test. To run the listing as it is, save it as `examples/spa.rs` and use
`cargo run --example spa`.

`get` is the name of a condition in `warded::matchers` and of a function in `axum::routing`. Import
one of them under another name, as here, where you need both.

Files at the top of the build, such as `favicon.ico`, need routes of their own, each with
`Router::new().fallback_service(ServeFile::new(…))` as the destination.

## During development

Run the build tool's own server for the app, and let it pass requests for the API on. With Vite:

```text
// vite.config.js
export default {
  server: {
    proxy: {
      // `secure: false` accepts the server's development certificate.
      "/api": { target: "https://localhost:8443", secure: false },
    },
  },
};
```

The browser then uses one origin, so no CORS settings are needed. They are only needed when the app
and the API stay on different origins; see [the API
guide](./json-api.md#call-it-from-a-browser-on-another-site).

## WebAssembly

`ServeDir` sends `.wasm` files as `application/wasm`, which browsers require to start them while
they download. Trunk writes its files next to `index.html` instead of in a directory; build with
`trunk build --release --public-url /assets/` and copy everything except `index.html` to `assets`.

## Frameworks that render on the server

Leptos and Dioxus applications are Axum routers, so they can be served like [any other Axum
application](./existing-axum.md). That has not been tested with this server. Next.js, SvelteKit,
Nuxt and others need their own JavaScript server; run that behind a proxy instead.
