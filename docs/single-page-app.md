---
description: "Serve a built React, Vue, Svelte, Angular or WebAssembly app together with its JSON API."
---

# A single-page app

A single-page app is a set of files that a build tool writes: an `index.html`, and scripts and
styles with generated names. The browser runs it and asks an API for data. This page serves both
from one server. It works for Vite (React, Vue, Svelte), Angular, Trunk (Yew, Leptos in the browser)
and others that produce such files. It does not run JavaScript on the server.

## The server

Start from the [API template](./json-api.md), build the app into a directory, and give both to
`SinglePageApp`. It answers, in this order:

1. below the API's prefix, with the API, which answers its own 404s;
2. below `/assets`, with the files that have generated names, which browsers may keep for a year;
3. with other files at the top of the build, such as `favicon.svg` or `robots.txt`;
4. with `index.html` for every other page that a browser opens, because the app has its own routes;
5. with a 404 for everything else.

Browsers must ask whether everything outside `assets/` has changed before they use it again, so that
they find the new names after an update. The checks at the end need two crates:

```sh
cargo add --dev http-body-util tempfile
```

The whole application:

```rust
use axum::{
    Json, Router,
    body::Body,
    http::{Request, StatusCode, header},
    routing::get,
};
use http_body_util::BodyExt;
use std::path::Path;
use tower::ServiceExt;
use warded_server::SinglePageApp;

fn app(dist: &Path) -> Router {
    let api = Router::new().route("/api/hello", get(|| async { Json("hello") }));
    SinglePageApp::new(dist).api("/api", api).into_router()
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let dist = tempfile::tempdir()?;
    std::fs::write(dist.path().join("index.html"), "<!doctype html>")?;
    std::fs::write(dist.path().join("favicon.svg"), "<svg></svg>")?;
    std::fs::create_dir(dist.path().join("assets"))?;
    std::fs::write(dist.path().join("assets/app.1a2b.js"), "start()")?;

    let app = app(dist.path());
    let browser = "text/html,application/xhtml+xml,*/*;q=0.8";
    for (uri, accept, status, body) in [
        ("/", browser, StatusCode::OK, "<!doctype html>"),
        ("/settings/profile", browser, StatusCode::OK, "<!doctype html>"),
        ("/assets/app.1a2b.js", "*/*", StatusCode::OK, "start()"),
        ("/favicon.svg", "image/*", StatusCode::OK, "<svg></svg>"),
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
            let kept = response.headers().get(header::CACHE_CONTROL);
            let forever = "public, max-age=31536000, immutable";
            assert_eq!(kept.is_some_and(|value| value == forever), status == StatusCode::OK);
        }
        assert_eq!(response.into_body().collect().await?.to_bytes(), body);
    }
    Ok(())
}
```

The fourth step is limited to requests that prefer HTML. Without that, a wrong address in the app's
code gets `index.html` with a 200, which is hard to find. Programs such as curl, which take
anything, get a 404 too, unless they send `Accept: text/html`.

`app` goes into `src/routes.rs`, and `main` calls it with the directory of the build. The checks in
the `main` above belong in a test. To run the listing as it is, save it as `examples/spa.rs` and use
`cargo run --example spa`. Add `DefaultMiddleware` to what `into_router` returns, as the API
template does, for logging and compression.

For build tools that put the files with generated names elsewhere, name that directory with
`assets`, as in `SinglePageApp::new("dist").assets("static")`. Its HTML files are never kept, in
case it holds pages too.

## During development

Run the build tool's own server for the app, and let it pass requests for the API on. With Vite:

```text
// vite.config.js
export default {
  server: {
    proxy: {
      // The server's development address, over plain HTTP.
      "/api": { target: "http://127.0.0.1:3000" },
    },
  },
};
```

The browser then uses one origin, so no CORS settings are needed. They are only needed when the app
and the API stay on different origins; see [the API
guide](./json-api.md#call-it-from-a-browser-on-another-site).

## WebAssembly

`SinglePageApp` sends `.wasm` files as `application/wasm`, which browsers require to start them
while they download. Trunk writes its files next to `index.html` instead of in a directory. They are
served from there, but browsers must ask again before they use them. To let browsers keep them,
build with `trunk build --release --public-url /assets/` and move everything except `index.html` to
`assets`.

## Frameworks that render on the server

Leptos and Dioxus applications are Axum routers, so they can be served like [any other Axum
application](./existing-axum.md). That has not been tested with this server. Next.js, SvelteKit,
Nuxt and others need their own JavaScript server; run that behind a proxy instead.
