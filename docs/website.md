---
description: "Build a website with pages rendered on the server, a form, static files, languages and caching, starting from the website template."
---

# A website

Start from the `website` template:

```sh
cargo generate viell-dev/slop-warded-server templates/website --tag 0.1.0-alpha.2 --name my-site
cd my-site
cargo run
```

Open the address that the log names, `http://127.0.0.1:3000/`. It is the [development
address](./configuration.md#the-development-address), without a certificate to accept, and its pages
reload by themselves when the server starts again. `bacon run-long` restarts the server after each
change; see [getting started](./getting-started.md#run-it).

For pages that are built with JavaScript in the browser, see [single-page
apps](./single-page-app.md) instead.

## What you get

A small site with a menu, three pages, a contact form that the server checks, and pages for a
missing address and for an error:

- `src/main.rs`: starts the server. You seldom need to change it.
- `src/routes.rs`: the routes and the middleware, with their tests in `src/routes/tests.rs`.
- `src/pages.rs`: the pages, as types that [Askama](https://askama.rs) templates fill in.
- `src/contact.rs`: the contact form.
- `templates/`: the HTML. `base.html` is the frame with the menu, which the pages extend.
- `static/`: styles and the icon, served below `/static/`.
- `bacon.toml`: makes bacon watch `templates/` and `static/` as well as `src/`.

The routes:

<<< ../templates/website/src/routes.rs

## Add a page

Write `templates/news.html`:

```text
{% extends "base.html" %}
{% block content %}
<p>What is new.</p>
{% endblock %}
```

Then add its type and handler to `src/pages.rs`, like `About` and `about`, a route to
`src/routes.rs`, and a link to the menu in `templates/base.html`:

```text
.route(page("/news"), pages::news.into_service())
```

Askama checks the templates when the project is built, so a missing value is a compile error. It
escapes values, so text from visitors cannot add markup.

## Static files

`StaticFiles` serves the directory below `/static/`, and the site's page for a 404 where there is no
file. It uses `ServeDir` from `tower-http`, which sends content types and `Last-Modified`, answers
range requests, and finds no file for paths that would leave the directory. It reads from the
directory where the server is started, so copy `static/` along when you [deploy](./deployment.md).

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

## Forms

The contact page shows its form on GET and handles it on POST. When the input is wrong, it shows
the form again, with a 422, what the visitor entered and what to correct. When it is right, it
redirects with a 303, so that reloading the next page does not send it again:

<<< ../templates/website/src/contact.rs

Each field has a label, and one with a problem has `aria-invalid` and a description, which screen
readers announce. The list of problems links to the fields, and the page's title begins with
`Error:`:

<<< ../templates/website/templates/contact.html

The form has `novalidate`, so that the browser leaves the checks to the server, whose messages are
the same everywhere. A site with logins also needs protection against forms on other sites (CSRF),
for example cookies marked `SameSite=Lax` and a check of the `Origin` field. Nothing here does that
for you.

## Languages

To serve a page in the language that the visitor prefers, give each language a route that asks for
it. `Offers` holds the languages that exist. The last route has no language, so it catches everyone
else. Responses carry `Vary: Accept-Language`, which keeps caches from giving one visitor another's
language:

```rust
use axum::{
    body::Body,
    handler::HandlerWithoutStateExt,
    http::{
        Request,
        header::{ACCEPT_LANGUAGE, CONTENT_LANGUAGE, VARY},
    },
    response::{Html, IntoResponse, Response},
};
use tower::ServiceExt;
use warded_server::warded::{
    self, Matcher,
    matchers::{exact_path, get, head},
    negotiation::{LanguageMatching, Offers},
};

fn in_language(language: &'static str, html: &'static str) -> Response {
    let headers = [(CONTENT_LANGUAGE, language), (VARY, "accept-language")];
    (headers, Html(html)).into_response()
}

async fn home() -> Response {
    in_language("en", r#"<html lang="en"><h1>Welcome</h1>"#)
}

async fn home_in_swedish() -> Response {
    in_language("sv", r#"<html lang="sv"><h1>Välkommen</h1>"#)
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let languages = Offers::languages(["en", "sv"], LanguageMatching::Lookup)?;
    let page = || get().or(head()).and(exact_path("/"));
    let routes = warded::Router::new()
        // Swedish when the visitor prefers it, and English otherwise.
        .route(
            page().and(languages.selected("sv")?),
            home_in_swedish.into_service(),
        )
        .route(page(), home.into_service());
    let app = warded_server::app(routes);

    for (preferred, expected) in [("sv-SE, en;q=0.5", "sv"), ("en", "en"), ("de", "en")] {
        let request = Request::builder()
            .uri("/")
            .header(ACCEPT_LANGUAGE, preferred)
            .body(Body::empty())?;
        let response = app.clone().oneshot(request).await?;
        assert_eq!(response.headers()[CONTENT_LANGUAGE], expected, "{preferred}");
        assert_eq!(response.headers()[VARY], "accept-language");
    }
    Ok(())
}
```

In a project from the template, give the pages' types a `language`, and write
`<html lang="{{ language }}">` in `base.html`. [Warded's negotiation
guide](https://viell-dev.github.io/slop-warded-router/negotiation.html) covers regional languages, a
language picker and many pages.

## Before others use it

- Replace the example pages and texts, and remove `static/` files that you do not need.
- Decide what happens with the contact form's messages: `send` in `src/contact.rs` only logs that
  one came.
- Decide on security headers; see [middleware](./middleware.md#security-headers).
- See [certificates](./certificates.md) and [deployment](./deployment.md).
