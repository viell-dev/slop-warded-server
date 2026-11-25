---
description: "Build a website with pages rendered on the server, static files, languages, forms and caching, starting from the website template."
---

# A website

Start from the `website` template:

```sh
cargo generate viell-dev/slop-warded-server templates/website --tag 0.1.0-alpha.1 --name my-site
cd my-site
cargo run
```

Open `https://localhost:8443/` and accept the warning about the development certificate, or make one
that your browser trusts with [mkcert](./certificates.md#for-development).

For pages that are built with JavaScript in the browser, see [single-page
apps](./single-page-app.md) instead.

## What you get

- `src/main.rs`: starts the server. You seldom need to change it.
- `src/routes.rs`: the routes and the middleware, with their tests in `src/routes/tests.rs`.
- `src/pages.rs`: the pages, as types that [Askama](https://askama.rs) templates fill in.
- `templates/`: the HTML. `base.html` is the frame that the pages extend.
- `static/`: styles, images and scripts, served below `/static/`.

The routes:

<<< ../templates/website/src/routes.rs

## Add a page

Write `templates/about.html`:

```text
{% extends "base.html" %}
{% block content %}
<p>{{ text }}</p>
{% endblock %}
```

Then add its type and handler to `src/pages.rs`, like `Home` and `home`, and a route to
`src/routes.rs`: `.route(get_or_head().and(exact_path("/about")), any(pages::about))`.

Askama checks the templates when the project is built, so a missing value is a compile error. It
escapes values, so text from visitors cannot add markup.

## Static files

`ServeDir` from `tower-http` serves the directory with content types, range requests and
`Last-Modified`. It reads from the directory where the server is started, so copy `static/` along when
you [deploy](./deployment.md).

To let browsers keep files without asking again, put a version in their names, such as
`style.3f9a.css`, and add a long `Cache-Control` to that route only:

```rust
use axum::{
    Router,
    body::Body,
    http::{HeaderValue, Request, header::CACHE_CONTROL},
};
use tower::ServiceExt;
use tower_http::{services::ServeDir, set_header::SetResponseHeaderLayer};
use warded_server::warded::{self, matchers::path};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let directory = tempfile::tempdir()?;
    std::fs::write(directory.path().join("style.3f9a.css"), "main {}")?;

    let forever = HeaderValue::from_static("public, max-age=31536000, immutable");
    let files = Router::new()
        .nest_service("/static", ServeDir::new(directory.path()))
        .layer(SetResponseHeaderLayer::if_not_present(CACHE_CONTROL, forever));
    let routes = warded::Router::new().route(path("/static/{*file}")?, files);

    let request = Request::builder()
        .uri("/static/style.3f9a.css")
        .body(Body::empty())?;
    let response = warded_server::app(routes).oneshot(request).await?;
    assert_eq!(response.headers()["content-type"], "text/css");
    assert!(response.headers()[CACHE_CONTROL].to_str()?.contains("immutable"));
    Ok(())
}
```

## Languages

The template serves `/` in English and Swedish. `Offers` holds the languages that exist, and each
language has a route that asks whether the visitor prefers it. The last route has no language, so it
catches everyone else. Responses carry `Vary: Accept-Language`, which keeps caches from giving one
visitor another's language.

[Warded's negotiation guide](https://viell-dev.github.io/slop-warded-router/negotiation.html) covers
regional languages, a language picker and many pages.

## Forms

Show the form on GET and handle it on POST, then redirect, so that reloading does not send it again:

```rust
use axum::{
    Form,
    body::Body,
    http::{Request, StatusCode, header::CONTENT_TYPE},
    response::{Html, Redirect},
    routing::any,
};
use serde::Deserialize;
use tower::ServiceExt;
use warded_server::warded::{
    Matcher, Router,
    matchers::{exact_path, get, post},
};

#[derive(Deserialize)]
struct Subscription {
    email: String,
}

async fn form() -> Html<&'static str> {
    Html(
        r#"<form method="post"><input type="email" name="email" required>
        <button>Subscribe</button></form>"#,
    )
}

async fn subscribe(Form(subscription): Form<Subscription>) -> Result<Redirect, StatusCode> {
    if !subscription.email.contains('@') {
        return Err(StatusCode::UNPROCESSABLE_ENTITY);
    }
    // Store it here.
    Ok(Redirect::to("/thanks"))
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let routes = Router::new()
        .route(get().and(exact_path("/subscribe")), any(form))
        .route(post().and(exact_path("/subscribe")), any(subscribe));

    let request = Request::builder()
        .method("POST")
        .uri("/subscribe")
        .header(CONTENT_TYPE, "application/x-www-form-urlencoded")
        .body(Body::from("email=someone%40example.com"))?;
    let response = warded_server::app(routes).oneshot(request).await?;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    assert_eq!(response.headers()["location"], "/thanks");
    Ok(())
}
```

A site with logins also needs protection against forms on other sites (CSRF), for example cookies
marked `SameSite=Lax` and a check of the `Origin` field. Nothing here does that for you.

## Before others use it

- Remove `static/` files that you do not need, and the Swedish page if it stays unused.
- Decide on security headers; see [middleware](./middleware.md#security-headers).
- See [certificates](./certificates.md) and [deployment](./deployment.md).
