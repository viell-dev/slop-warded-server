---
description: "Build a JSON API: resources, errors, query parameters, authentication, versions, CORS and limits, starting from the api template."
---

# A JSON API

The [API template guide](./template-api.md) starts a project and walks through changing it.
Use the examples here when you are ready to handle errors, add a database or take input from
query parameters. See [Middleware](./middleware.md) for logging and compression.

## Errors

Errors are JSON with a status, such as `{"error":"there is no such note"}`. Handlers return a
`Problem`, and `JsonBody` and `PathParams` are extractors that return one when the request is wrong:

<<< ../templates/api/src/problem.rs

Wrap other extractors that you use, such as Axum's `Query`, in the same way, and return `Problem`
from middleware of your own. Errors that the server or the middleware answer, such as a request head
that is too large, are not JSON.

For typed ids, use a type that implements `Deserialize` in the fields of `NoteParams`. To name a new
resource to the client, return its path in `Location`, as in
`(StatusCode::CREATED, [(LOCATION, format!("/notes/{id}"))], Json(note))`.

Run the tests with `cargo test`. They call the application directly, without a network; see
[testing](./testing.md).

## Use a database

`Notes` is the state: a value that is cheap to clone and shared between clones. A connection pool,
such as SQLx's `PgPool`, is one too. Create it in `main`, pass it to `app` in place of
`Notes::default()`, and give it to handlers with `.with_state(pool.clone())`. Handlers receive it as
`State(pool): State<PgPool>`.

## Query parameters

Axum's `Query` extractor parses them. A route can also require one; see
[Warded's guide](https://viell-dev.github.io/slop-warded-router/conditions.html).

```rust
use axum::{
    Json,
    body::Body,
    extract::Query,
    http::Request,
    routing::any,
};
use http_body_util::BodyExt;
use serde::Deserialize;
use tower::ServiceExt;
use warded_server::warded::{
    Matcher, Router,
    matchers::{exact_path, get},
};

#[derive(Deserialize)]
struct Page {
    #[serde(default)]
    offset: usize,
    #[serde(default = "default_limit")]
    limit: usize,
}

fn default_limit() -> usize {
    20
}

async fn list(Query(page): Query<Page>) -> Json<Vec<usize>> {
    let limit = page.limit.min(100);
    Json((0..1_000).skip(page.offset).take(limit).collect())
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let routes = Router::new().route(get().and(exact_path("/numbers")), any(list));
    let request = Request::builder()
        .uri("/numbers?offset=5&limit=3")
        .body(Body::empty())?;
    let response = warded_server::app(routes).oneshot(request).await?;
    assert_eq!(response.into_body().collect().await?.to_bytes(), "[5,6,7]");
    Ok(())
}
```

## Require authentication on some routes

A layer on a destination runs only when that route is selected. This one checks a bearer token
before the handler runs. In a project from the template, return a `Problem` in place of the plain
response:

```rust
use axum::{
    body::Body,
    extract::Request,
    http::{
        StatusCode,
        header::{AUTHORIZATION, WWW_AUTHENTICATE},
    },
    middleware::{Next, from_fn_with_state},
    response::{IntoResponse, Response},
    routing::any,
};
use std::{collections::HashSet, sync::Arc};
use tower::ServiceExt;
use warded_server::warded::{
    Matcher, Router,
    matchers::{exact_path, get},
};

/// The clients' tokens. Read real ones from outside the source, and store only hashes of them.
#[derive(Clone)]
struct Tokens(Arc<HashSet<String>>);

async fn require_token(
    axum::extract::State(tokens): axum::extract::State<Tokens>,
    request: Request,
    next: Next,
) -> Response {
    let token = request
        .headers()
        .get(AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "));
    match token {
        Some(token) if tokens.0.contains(token) => next.run(request).await,
        _ => (StatusCode::UNAUTHORIZED, [(WWW_AUTHENTICATE, "Bearer")]).into_response(),
    }
}

fn app(tokens: Tokens) -> axum::Router {
    let protected = || from_fn_with_state(tokens.clone(), require_token);
    let routes = Router::new()
        .route(
            get().and(exact_path("/health")),
            any(|| async { "healthy" }),
        )
        .route(
            get().and(exact_path("/account")),
            any(|| async { "your account" }).layer(protected()),
        );
    warded_server::app(routes)
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let app = app(Tokens(Arc::new(HashSet::from(["secret".to_owned()]))));
    for (path, authorization, expected) in [
        ("/health", "", StatusCode::OK),
        ("/account", "", StatusCode::UNAUTHORIZED),
        ("/account", "Bearer wrong", StatusCode::UNAUTHORIZED),
        ("/account", "Bearer secret", StatusCode::OK),
    ] {
        let request = Request::builder()
            .uri(path)
            .header(AUTHORIZATION, authorization)
            .body(Body::empty())?;
        let response = app.clone().oneshot(request).await?;
        assert_eq!(response.status(), expected, "{path} {authorization}");
    }
    Ok(())
}
```

Do not check access in a route's condition: when it fails, the next route is tried, instead of the
request being refused.

## Versions of the API

Put the version in the path, or let clients name it in a header. A route per version keeps the
handlers apart:

```sh
cargo add tower-http --features set-header
```

```rust
use axum::{
    body::Body,
    http::{HeaderValue, Request, header::VARY},
    routing::any,
};
use http_body_util::BodyExt;
use tower::ServiceExt;
use tower_http::set_header::SetResponseHeaderLayer;
use warded_server::warded::{
    Matcher, Router,
    matchers::{exact_path, get, header_eq},
};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let users = || get().and(exact_path("/users"));
    let routes = Router::new()
        .route(
            users().and(header_eq("x-api-version", "2")?),
            any(|| async { "version 2" }),
        )
        // Clients that name no version, or another one, get the oldest.
        .route(users(), any(|| async { "version 1" }));
    // Tells caches to keep the versions apart.
    let vary = SetResponseHeaderLayer::appending(VARY, HeaderValue::from_static("x-api-version"));
    let app = warded_server::app(routes).layer(vary);

    for (version, expected) in [("2", "version 2"), ("1", "version 1")] {
        let request = Request::builder()
            .uri("/users")
            .header("x-api-version", version)
            .body(Body::empty())?;
        let response = app.clone().oneshot(request).await?;
        assert_eq!(response.into_body().collect().await?.to_bytes(), expected);
    }
    Ok(())
}
```

## Call it from a browser on another site

Browsers ask for permission first (CORS). `tower-http` answers for the whole application, so add its
layer to what `warded_server::app` returns, where it also sees the `OPTIONS` requests that no route
handles. Allow every method and header that the app uses:

```sh
cargo add tower-http --features cors
```

```rust
use axum::{
    body::Body,
    http::{HeaderName, HeaderValue, Method, Request, header},
    routing::any,
};
use tower::ServiceExt;
use tower_http::cors::CorsLayer;
use warded_server::warded::{
    Matcher, Router,
    matchers::{exact_path, get},
};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let routes = Router::new().route(get().and(exact_path("/")), any(|| async { "data" }));
    let cors = CorsLayer::new()
        .allow_origin(HeaderValue::from_static("https://app.example.com"))
        .allow_methods([Method::GET, Method::POST, Method::DELETE])
        .allow_headers([
            header::AUTHORIZATION,
            header::CONTENT_TYPE,
            HeaderName::from_static("x-api-version"),
        ]);
    let app = warded_server::app(routes).layer(cors);

    let preflight = Request::builder()
        .method(Method::OPTIONS)
        .uri("/")
        .header(header::ORIGIN, "https://app.example.com")
        .header(header::ACCESS_CONTROL_REQUEST_METHOD, "GET")
        .body(Body::empty())?;
    let response = app.oneshot(preflight).await?;
    let allowed = &response.headers()[header::ACCESS_CONTROL_ALLOW_ORIGIN];
    assert_eq!(allowed, "https://app.example.com");
    Ok(())
}
```

## Before others use it

- **Body size.** Axum's extractors refuse bodies over 2 MB. Change that on a destination, as in
  `any(create).layer(DefaultBodyLimit::max(10_000)).with_state(state)`, with
  `axum::extract::DefaultBodyLimit`.
- **Time.** Add a timeout where handlers wait for something; see [middleware](./middleware.md).
- **Deployment.** See [certificates](./certificates.md) and [deployment](./deployment.md).
