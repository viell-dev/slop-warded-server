---
description: "Create a project from a template, or add the server to one you have, and reach it over HTTP/1.1, HTTP/2 and HTTP/3."
---

# Getting started

Warded Server is a library that runs your application over HTTP/1.1, HTTP/2 and HTTP/3. You write
routes and handlers; it takes care of listening, TLS, QUIC, shutdown and limits. Templates give you a
working project to start from.

## What you need

- Rust 1.99.1 or later, from [rustup](https://rustup.rs).
- A C compiler, for the cryptography library. On Windows the Visual Studio build tools that Rust uses
  include one. On Debian and Ubuntu, install `build-essential`.
- [cargo-generate](https://cargo-generate.github.io/cargo-generate/installation.html) 0.26 or later,
  to use the templates:

```sh
cargo install cargo-generate --locked
```

## Create a project

```sh
cargo generate viell-dev/slop-warded-server --tag 0.1.0-alpha.1
```

It asks for a template and a project name, and makes a directory with that name:

| Template            | Choose it for                                        | Guide                        |
| ------------------- | ---------------------------------------------------- | ---------------------------- |
| `templates/minimal` | Learning, or adding only what you want               | this page                    |
| `templates/api`     | A JSON API, also as the back end of an app           | [JSON API](./json-api.md)    |
| `templates/website` | HTML pages from the server, with styles and images   | [Website](./website.md)      |

To skip the questions, name both:

```sh
cargo generate viell-dev/slop-warded-server templates/api --tag 0.1.0-alpha.1 --name my-api
```

## Run it

```sh
cd my-api
cargo run
```

The first build takes a minute or so. When the log says `serving`, the server is at
`https://localhost:8443`. It made a certificate that nothing trusts, so a browser warns about it and
curl needs `--insecure`:

```sh
curl --insecure --http1.1 https://localhost:8443/
curl --insecure --http2 https://localhost:8443/
curl --insecure --http3-only https://localhost:8443/
```

The last one needs a curl with HTTP/3, which `curl --version` lists among its features. Stop the
server with Ctrl+C, twice to do it at once.

[Certificates](./certificates.md) explains how to get one that browsers trust, also for development.

## The code

The minimal template's `src/main.rs` is a whole application:

<<< ../templates/minimal/src/main.rs

- **`Router::new().route(condition, handler)`** adds a route. Routes are tried in order, and the first
  whose conditions all hold gets the request. [Routes and requests](./requests.md) introduces them.
- **`any(home)`** turns an Axum handler into a destination. Warded already selects on the method, so
  the handler takes any.
- **`warded_server::app(routes)`** makes an `axum::Router` of them, which accepts
  [middleware](./middleware.md).
- **`Server::from_env()`** reads its settings from [environment variables](./configuration.md), and
  **`serve`** runs until the process is asked to stop.

## Add it to a project

The same parts work in a project of your own:

```toml
[dependencies]
axum = "0.8.9"
tokio = { version = "1", features = ["macros", "rt-multi-thread"] }
warded-server = { git = "https://github.com/viell-dev/slop-warded-server", tag = "0.1.0-alpha.1" }
```

Use Warded through `warded_server::warded`, as the examples here do. A direct dependency on another
version of `warded` would be a separate library to Rust, whose `secured()` condition does not see
what the server knows about the connection.

If you have an Axum application, see [existing Axum applications](./existing-axum.md).

### Features

All are on by default. Turn them off with `default-features = false` and list the ones you want:

| Feature       | Provides                                                         |
| ------------- | ---------------------------------------------------------------- |
| `http3`       | [HTTP/3](./http3.md)                                             |
| `self-signed` | The generated development certificate                            |
| `warded`      | `app`, `Params` and the security context for Warded's conditions |

## Update or remove

To update, change the `tag` in `Cargo.toml` and run `cargo update warded-server`. Read the
[changelog](./changelog.md) first: any alpha may break your code. Files that a template generated are
yours, and nothing updates them.

To remove the server, replace it with `axum::serve` and delete the dependency:

```rust,no_run
use axum::{Router, routing::get};

#[tokio::main]
async fn main() -> std::io::Result<()> {
    let app = Router::new().route("/", get(|| async { "Hello" }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:3000").await?;
    axum::serve(listener, app).await
}
```

Warded routes work there too; see [its guide](https://viell-dev.github.io/slop-warded-router/tower.html).

## Next steps

- Build something: a [JSON API](./json-api.md), a [website](./website.md) or a
  [single-page app](./single-page-app.md).
- Make it ready for others: [configuration](./configuration.md), [certificates](./certificates.md) and
  [deployment](./deployment.md).
- Know what is missing: [limits](./limits.md).
