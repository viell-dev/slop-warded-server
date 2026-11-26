---
description: "Start with one route, then add an endpoint and return data from Rust."
---

# Minimal template

Use this template to learn how a route works or to start with a small application.
The names used below are in the [glossary](./glossary.md).

## Start

See [what you need](./getting-started.md#what-you-need) for Rust, a C compiler,
`cargo-generate` and the shell to run the commands in.

```sh
cargo generate viell-dev/slop-warded-server templates/minimal --tag 0.1.0-alpha.2 --name my-app
cd my-app
cargo run
```

Open the address printed in the log, `http://127.0.0.1:3000/`. You should see `Hello`.
The project's `.cargo/config.toml` sets `WARDED_DEV_ADDR` for this
[development address](./configuration.md#the-development-address).

## What is where

| To change                             | Open                                        |
| ------------------------------------- | ------------------------------------------- |
| The response at `/`                   | `src/main.rs`, in `home`                    |
| The paths and methods the app answers | `src/main.rs`, where `main` builds `routes` |
| The local address                     | `.cargo/config.toml`                        |

<<< ../templates/minimal/src/main.rs

## First change

Change the string in `home` to `"Hello from my app\n"`. Stop the server with Ctrl+C, run
`cargo run` again, and refresh the browser. It now shows `Hello from my app`. Follow this
restart and refresh after the next two changes too.

## Add a page or endpoint

Give `GET /health` its own handler. In `src/main.rs`, add the `health` function from the
listing below after `home`. Add `.route(get().and(exact_path("/health")), any(health))` after
the route for `/` and move the semicolon to the end of the chain. The file should now look
like this:

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
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    warded_server::init_logging();
    let routes = Router::new()
        .route(get().and(exact_path("/")), any(home))
        .route(get().and(exact_path("/health")), any(health));
    Server::from_env()?.serve(warded_server::app(routes)).await?;
    Ok(())
}
```

Restart the server and open `http://127.0.0.1:3000/health`. It should say `OK`.
`get().and(exact_path("/health"))` requires both the GET method and the exact path.
`any(health)` connects the handler to Warded, which already checks the method.

## Add data or a form

Add an `/hours` endpoint that formats its response from two numbers. In `src/main.rs`, add
the `hours` function from the listing below after `health`. Add
`.route(get().and(exact_path("/hours")), any(hours))` after the health route and move the
semicolon to the end of the chain. The file should now look like this:

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
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    warded_server::init_logging();
    let routes = Router::new()
        .route(get().and(exact_path("/")), any(home))
        .route(get().and(exact_path("/health")), any(health))
        .route(get().and(exact_path("/hours")), any(hours));
    Server::from_env()?.serve(warded_server::app(routes)).await?;
    Ok(())
}
```

Open `http://127.0.0.1:3000/hours`. It should say `Open 09:00 to 17:00`.

## Test and put it online

Use `cargo check` to check the project while editing. Use `cargo run` and open the endpoints
above to check what they return. Follow [Your first test](./testing.md#your-first-test) to add
a check you can run with `cargo test`.

Run `cargo build --release` to build the standalone program. Run this alpha on your own computer
or a private network whose users you trust. Keep it off public sites and read the
[limits](./limits.md) before deploying.
[Deployment](./deployment.md) covers starting the binary and setting its addresses.
[Certificates](./certificates.md) explains how to get and configure a trusted certificate.
For a Linux server, build on Linux or use the [Dockerfile](./deployment.md#containers).
A Windows executable does not run on Linux.

## Learn more

- How do I combine conditions or read a name from `/hello/{name}`? [Routes and requests](./requests.md#declaring-routes)
- How do I return HTML or JSON from a handler? [Axum](https://docs.rs/axum/0.8.9/axum/)
- What else can a Warded route match? [Warded's guides](https://viell-dev.github.io/slop-warded-router/)
- How do I learn the Rust in these examples? [The Rust book](https://doc.rust-lang.org/book/)
