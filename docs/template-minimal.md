---
description: "Start with one route, then add an endpoint and return data from Rust."
---

# Minimal template

Use this template to learn how a route works or to start with a small application.
The names used below are in the [glossary](./glossary.md).

## Start

See [what you need](./getting-started.md#what-you-need) for Rust, a C compiler and
`cargo-generate`. Install `cargo-generate` once if you need it.

```sh
cargo generate viell-dev/slop-warded-server templates/minimal --tag 0.1.0-alpha.2 --name my-app
cd my-app
cargo run
```

Open the address printed in the log, `http://127.0.0.1:3000/`. You should see `Hello`.
The project's `.cargo/config.toml` sets `WARDED_DEV_ADDR` for this
[development address](./configuration.md#the-development-address).

## What is where

| To change | Open |
| --- | --- |
| The response at `/` | `src/main.rs`, in `home` |
| The paths and methods the app answers | `src/main.rs`, where `main` builds `routes` |
| The local address | `.cargo/config.toml` |

<<< ../templates/minimal/src/main.rs

## First change

Change the string in `home` to `"Hello from my app\n"`. Stop the server with Ctrl+C, run
`cargo run` again, and refresh the browser. It now shows `Hello from my app`. Follow this
restart and refresh after the next two changes too.

## Add a page or endpoint

Give `GET /health` its own handler. Replace `src/main.rs` with the program below, then open
`http://127.0.0.1:3000/health`. It should say `OK`. The first route still answers `GET /`.

```rust,no_run
use axum::routing::any;
use warded_server::{
    Server,
    warded::{
        Matcher, Router,
        matchers::{exact_path, get},
    },
};

async fn home() -> &'static str {
    "Hello from my app\n"
}

async fn health() -> &'static str {
    "OK\n"
}

#[tokio::main]
async fn main() -> Result<(), warded_server::Error> {
    warded_server::init_logging();
    let routes = Router::new()
        .route(get().and(exact_path("/")), any(home))
        .route(get().and(exact_path("/health")), any(health));
    Server::from_env()?.serve(warded_server::app(routes)).await
}
```

`get().and(exact_path("/health"))` requires both the GET method and the exact path.
`any(health)` connects the handler to Warded, which already checks the method.

## Add data or a form

Put some fixed data in the app. Replace `src/main.rs` with this version. It adds an
`/hours` endpoint that formats its response from two numbers in the code.

```rust,no_run
use axum::routing::any;
use warded_server::{
    Server,
    warded::{
        Matcher, Router,
        matchers::{exact_path, get},
    },
};

async fn home() -> &'static str {
    "Hello from my app\n"
}

async fn health() -> &'static str {
    "OK\n"
}

async fn hours() -> String {
    let opens = 9;
    let closes = 17;
    format!("Open {opens:02}:00 to {closes:02}:00\n")
}

#[tokio::main]
async fn main() -> Result<(), warded_server::Error> {
    warded_server::init_logging();
    let routes = Router::new()
        .route(get().and(exact_path("/")), any(home))
        .route(get().and(exact_path("/health")), any(health))
        .route(get().and(exact_path("/hours")), any(hours));
    Server::from_env()?.serve(warded_server::app(routes)).await
}
```

Open `http://127.0.0.1:3000/hours`. It should say `Open 09:00 to 17:00`.

## Test and put it online

Use `cargo check` to check the project while editing. Use `cargo run` and open the endpoints
above to check what they return. There are no starter tests. When you add tests, run
`cargo test` and see [Testing](./testing.md) for how to call the application in a test.

Run `cargo build --release` to build the standalone program. Read [limits](./limits.md) before deploying. Keep this alpha off the open internet.
[Deployment](./deployment.md) covers starting the binary and setting its addresses.
[Certificates](./certificates.md) explains how to get and configure a trusted certificate.

## Learn more

- How do I combine route conditions? [Routes and requests](./requests.md)
- How do I return HTML or JSON from a handler? [Axum](https://docs.rs/axum/0.8.9/axum/)
- What else can a Warded route match? [Warded's guides](https://viell-dev.github.io/slop-warded-router/)
- Where can I look up Rust structs and functions? [The Rust book](https://doc.rust-lang.org/book/)
