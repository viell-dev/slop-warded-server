---
description: "Start the JSON API template, add an endpoint, and extend the data it stores."
---

# API template

The API starts with a list of endpoints and an example resource called a note.
The names used below are in the [glossary](./glossary.md).

## Start

See [what you need](./getting-started.md#what-you-need) for Rust, a C compiler,
`cargo-generate` and the shell to run the commands in.

```sh
cargo generate viell-dev/slop-warded-server templates/api --tag 0.1.0-alpha.3 --name my-api
cd my-api
cargo run
```

Open `http://127.0.0.1:3000/`. You should see a JSON object with an `endpoints` list.
The project's `.cargo/config.toml` sets `WARDED_DEV_ADDR` for this
[development address](./configuration.md#the-development-address).

## What is where

| To change                                     | Open                                  |
| --------------------------------------------- | ------------------------------------- |
| The endpoints or their list at `/`            | `src/routes.rs`                       |
| What a note contains or how it is stored      | `src/notes.rs`                        |
| JSON errors and how invalid input is reported | `src/problem.rs`                      |
| The checks for requests and responses         | `src/routes/tests.rs`                 |
| The server startup or local address           | `src/main.rs` or `.cargo/config.toml` |

## First change

In `src/routes.rs`, add `service: &'static str` to `struct Index`. In `index`, add
`service: "Notes API",` to the `Index { ... }` it returns.

<<< ../templates/api/src/routes.rs

Stop the server with Ctrl+C, run `cargo run` again, and refresh `/` to see the new
`"service":"Notes API"` field alongside `endpoints`. Restart after the next changes too.

## Add a page or endpoint

Add a `GET /health` endpoint. In `src/routes.rs`, paste `Health` and `health` from the
example below above `#[cfg(test)]`. Add its `.route(...)` call just after the one for `GET /`.
Add `"GET /health",` to the `endpoints` list in `index` so that it shows up at `/`.

Warded chooses the first matching route. Put fixed paths above patterns that could also match
them. For example, put `GET /notes/count` above `GET /notes/{id}` so the latter does not try to
read `count` as an integer ID. See [Declaring routes](./requests.md#declaring-routes).
When a handler takes `State<Notes>`, give its route `.with_state(notes.clone())`. The clones
share the same notes.

This complete example shows the new handler and checks what it returns. The template already
has the imports needed for the code you copy.

```rust
use axum::{Json, body::Body, http::Request, routing::any};
use http_body_util::BodyExt;
use serde::Serialize;
use tower::ServiceExt;
use warded_server::warded::{
    Matcher, Router,
    matchers::{exact_path, get},
};

#[derive(Serialize)]
struct Health {
    status: &'static str,
}

async fn health() -> Json<Health> {
    Json(Health { status: "ok" })
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let routes = Router::new().route(get().and(exact_path("/health")), any(health));
    let request = Request::get("/health").body(Body::empty())?;
    let response = warded_server::app(routes).oneshot(request).await?;
    assert!(response.status().is_success());
    assert_eq!(response.into_body().collect().await?.to_bytes(), r#"{"status":"ok"}"#);
    Ok(())
}
```

Open `http://127.0.0.1:3000/health` for `{"status":"ok"}`. This endpoint says that the
app can answer. It has no database to check.

Add the lines marked `+` to `src/routes/tests.rs`, without the `+`, and run `cargo test`.
The test uses the file's existing `send` helper to check the response from your application.

```diff
+#[tokio::test]
+async fn health_answers() {
+    let app = app(Notes::default()).unwrap();
+    let response = send(&app, "GET", "/health", None).await;
+    assert_eq!(response, (StatusCode::OK, json!({ "status": "ok" })));
+}
```

## Add data or a form

Add a `pinned` flag to a note. The example store is in memory, so restarting clears the notes.

<<< ../templates/api/src/notes.rs

1. In `src/notes.rs`, add `pinned: bool,` to both `Note` and `NewNote`. Put
   `#[serde(default)]` on the field in `NewNote` so older requests can leave it out and get
   `false`. In `create`, add `pinned: new.pinned,` to the `Note { ... }` you store and return.
2. In `src/routes/tests.rs`, update the expected `note` in
   `notes_can_be_created_read_and_deleted` to `json!({ "id": 1, "text": "milk", "pinned": false })`.
   Keep its POST body as it is to check the default.
3. Restart and run these [curl commands](./getting-started.md#shell-and-curl) from another
   terminal. The first sends a note as JSON.
   The second lists the notes, including the new one with `"pinned":true`.

```sh
curl --json '{"text":"milk","pinned":true}' http://127.0.0.1:3000/notes
curl http://127.0.0.1:3000/notes
```

## Test and put it online

Run `cargo test` to check creating, reading and deleting notes, the endpoint list and error
responses. See [Testing](./testing.md) for more ways to check the application.

Run `cargo build --release` to build the program. Replace the example in-memory store before
you need notes to survive a restart. Run this alpha on your own computer or a private network
whose users you trust. Keep it off public sites and read the [limits](./limits.md) before deploying.
[Deployment](./deployment.md) covers starting the binary and setting its addresses.
[Certificates](./certificates.md) explains how to get and configure a trusted certificate.
For a Linux server, build on Linux or use the [Dockerfile](./deployment.md#containers).
A Windows executable does not run on Linux.

## Learn more

- How do I add query parameters or a database? [A JSON API](./json-api.md)
- How does a Rust struct become JSON? [Serde](https://serde.rs/)
- How do handlers read query parameters and input? [Axum extractors](https://docs.rs/axum/0.8.9/axum/extract/)
- What can a route match besides the path? [Warded's guides](https://viell-dev.github.io/slop-warded-router/)
