---
description: "Build a website with pages rendered on the server, static files, languages, forms and caching, starting from the website template."
---

# A website

Start from the `website` template:

```sh
cargo generate viell-dev/slop-warded-server templates/website --tag 0.1.0-alpha.2 --name my-site
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
`src/routes.rs`:

```text
.route(get_or_head().and(exact_path("/about")), pages::about.into_service())
```

Askama checks the templates when the project is built, so a missing value is a compile error. It
escapes values, so text from visitors cannot add markup.

## Static files

`ServeDir` from `tower-http` serves the directory with content types, range requests and
`Last-Modified`. It reads from the directory where the server is started, so copy `static/` along
when you [deploy](./deployment.md).

To let browsers keep files without asking again, put a version in their names, such as
`style.3f9a.css`, and change the name whenever the file changes. Build tools such as Vite do both,
or rename the file by hand. Then add a long `Cache-Control` to that route only, and only for files
that exist:

```sh
cargo add tower-http --features set-header
```

```rust
use axum::{
    Router,
    body::Body,
    http::{HeaderValue, Request, StatusCode, header::CACHE_CONTROL},
    response::Response,
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
        .layer(SetResponseHeaderLayer::if_not_present(
            CACHE_CONTROL,
            move |response: &Response| {
                let found = response.status().is_success()
                    || response.status() == StatusCode::NOT_MODIFIED;
                found.then(|| forever.clone())
            },
        ));
    let routes = warded::Router::new().route(path("/static/{*file}")?, files);

    let app = warded_server::app(routes);
    for (uri, cached) in [("/static/style.3f9a.css", true), ("/static/old.css", false)] {
        let request = Request::builder().uri(uri).body(Body::empty())?;
        let response = app.clone().oneshot(request).await?;
        assert_eq!(response.headers().contains_key(CACHE_CONTROL), cached, "{uri}");
    }
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

Show the form on GET and handle it on POST, then redirect, so that reloading does not send it again.
When the input is wrong, show the form again, with the reason:

```sh
cargo add serde --features derive
```

```rust
use axum::{
    Form,
    body::Body,
    extract::rejection::FormRejection,
    http::{Request, StatusCode, header::CONTENT_TYPE},
    response::{Html, IntoResponse, Redirect, Response},
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

/// The form, with what went wrong before. Use a template for it in a real site.
fn form(problem: &str) -> Html<String> {
    Html(format!(
        r#"<p>{problem}</p><form method="post"><input type="email" name="email" required>
        <button>Subscribe</button></form>"#
    ))
}

async fn show_form() -> Html<String> {
    form("")
}

async fn subscribe(input: Result<Form<Subscription>, FormRejection>) -> Response {
    let problem = match input {
        Ok(Form(subscription)) if subscription.email.contains('@') => {
            // Store it here.
            return Redirect::to("/thanks").into_response();
        }
        Ok(_) => "That is not an e-mail address.",
        // A field is missing, or the body is not a form.
        Err(_) => "Please fill in the form.",
    };
    (StatusCode::UNPROCESSABLE_ENTITY, form(problem)).into_response()
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let routes = Router::new()
        .route(get().and(exact_path("/subscribe")), any(show_form))
        .route(post().and(exact_path("/subscribe")), any(subscribe));
    let app = warded_server::app(routes);

    for (body, status) in [
        ("email=someone%40example.com", StatusCode::SEE_OTHER),
        ("email=someone", StatusCode::UNPROCESSABLE_ENTITY),
        ("", StatusCode::UNPROCESSABLE_ENTITY),
    ] {
        let request = Request::builder()
            .method("POST")
            .uri("/subscribe")
            .header(CONTENT_TYPE, "application/x-www-form-urlencoded")
            .body(Body::from(body))?;
        let response = app.clone().oneshot(request).await?;
        assert_eq!(response.status(), status, "{body}");
    }
    Ok(())
}
```

A site with logins also needs protection against forms on other sites (CSRF), for example cookies
marked `SameSite=Lax` and a check of the `Origin` field. Nothing here does that for you.

## Before others use it

- Remove `static/` files that you do not need, and the Swedish page if it stays unused.
- Decide on security headers; see [middleware](./middleware.md#security-headers).
- See [certificates](./certificates.md) and [deployment](./deployment.md).
