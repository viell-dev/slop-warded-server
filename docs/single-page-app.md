---
description: "Serve a built React, Vue, Svelte, Angular or WebAssembly app together with its JSON API."
---

# A single-page app

Warded Server can serve a [Vite build](https://vite.dev/guide/static-deploy) and a Rust API from
one program. Vite runs during development and building. For a static build it writes
`index.html`, scripts and styles that an ordinary file server can send. The browser runs the
JavaScript and calls the API.

## Choose what to run

| You have | Use | Why |
| --- | --- | --- |
| A built site with no API | A static host or CDN | It can handle certificates and caching and serve files close to visitors. |
| A built site and an API in another language | That API's server, or Caddy or nginx | The build consists of ordinary files. Your API can keep its current runtime. |
| A built site and a Rust API | Warded Server for local or private trials | Run the build and API on one origin and deploy them together. |
| Nuxt, SvelteKit or Next.js with [server-side rendering][vite-ssr] | A supported JavaScript runtime such as Node.js, behind a proxy | The framework needs to run JavaScript to render pages for requests. |
| Interactive pages rendered by Rust | [Leptos](https://book.leptos.dev/) | Rust renders pages on the server and WebAssembly can make them interactive in the browser. |

## What you get with a Rust API

- **One origin.** Browser code can call `/api/hello` without cross-origin (CORS) settings. The
  frontend and API can ship together. Any server that serves both on the same origin gives you
  this.
- **HTTP/2 and HTTP/3 without a proxy.** [Node's QUIC module](https://nodejs.org/api/quic.md) is
  marked early development. [Caddy supports all three HTTP versions][caddy-protocols] in front
  of an application and can [obtain and renew certificates][caddy-https].
- **Routing by request details.** Warded can choose a build by a header or a language. A
  proxy that terminates the connection leaves the app with the proxy's HTTP version.
  `SinglePageApp` keeps API 404s separate from the page fallback.

Warded Server is an alpha. HTTP/3 uses an experimental library with [known
weaknesses](./limits.md#http-3). Keep it off the open internet. It serves files from where the
program runs. A CDN can serve them from locations closer to visitors. Warded Server requires you
to provide and renew [certificates](./certificates.md) for deployment
([issue #4](https://github.com/viell-dev/slop-warded-server/issues/4)).

Warded Server serves built JavaScript as files. For HTML rendered in Rust with JavaScript and CSS
built by Vite, see [Vite's backend integration guide](https://vite.dev/guide/backend-integration).
It shows how to find the generated filenames in the build manifest. This setup has not been
tested here.

[caddy-protocols]: https://caddyserver.com/docs/caddyfile/options#protocols
[caddy-https]: https://caddyserver.com/docs/automatic-https
[vite-ssr]: https://vite.dev/guide/ssr

## The server

Start from the [API template](./template-api.md), build the app into a directory, and give both to
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

To run the checks as written, save the listing as `examples/spa.rs` and use
`cargo run --example spa`. To serve your build with the template's API, put its routes under
`/api` (for example, `/notes` becomes `/api/notes`). Update the endpoint list and the tests to
match. In `src/main.rs`, wrap the result of `routes::app` before passing it to `serve`:

```text
let app = warded_server::SinglePageApp::new("dist").api("/api", app).into_router();
```

Use the directory of your build in place of `dist`. To apply logging and compression to both
the files and the API, move the template's `DefaultMiddleware` to the combined router returned
by `into_router`.

For build tools that put the files with generated names elsewhere, name that directory with
`assets`, as in `SinglePageApp::new("dist").assets("static")`. Its HTML files get `no-cache`, in
case it holds pages too.

## During development

Run the build tool's own server for the app, and let it pass requests for the API on. With
[Vite's development proxy](https://vite.dev/config/server-options.html#server-proxy):

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

If your Rust framework gives you an Axum router, see [Existing Axum
applications](./existing-axum.md). See [tested environments](./limits.md#tested-environments)
for framework coverage.
