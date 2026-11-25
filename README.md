# Warded Server

A ready-wired Rust server for HTTP/1.1, HTTP/2 and HTTP/3. It assembles
[Axum](https://docs.rs/axum), [Hyper](https://hyper.rs), [rustls](https://docs.rs/rustls) and
[Quinn](https://docs.rs/quinn) with the
[Warded](https://github.com/viell-dev/slop-warded-router) router, so that a new project starts from
its routes and handlers.

**Alpha.** The API, the templates and the configuration still change, and HTTP/3 rests on an
experimental library with [known weaknesses](./docs/limits.md#http3). Do not put it on the open
internet yet.

Agents write and maintain this project. No person reviews the code. See
[CONTRIBUTING.md](./CONTRIBUTING.md) for contribution and verification expectations.

## Start a project

You need Rust 1.99.1 or later and
[cargo-generate](https://cargo-generate.github.io/cargo-generate/installation.html):

```sh
cargo install cargo-generate --locked
cargo generate viell-dev/slop-warded-server --tag 0.1.0-alpha.1
```

It asks which template to use and what to call the project. To skip the first question, name the
template: `cargo generate viell-dev/slop-warded-server templates/api --tag 0.1.0-alpha.1`.

| Template            | What you get                                                           |
| ------------------- | ---------------------------------------------------------------------- |
| `templates/minimal` | One route and nothing else                                             |
| `templates/api`     | A JSON API with state, errors, logging, compression and tests          |
| `templates/website` | HTML pages in two languages, static files, logging, compression, tests |

Then run it:

```sh
cargo run
curl --insecure --http3 https://localhost:8443/
```

Without a certificate the server generates one that no client trusts, hence `--insecure`.
[Configuration](./docs/configuration.md) covers real certificates and the environment variables.

## Add it to an existing project

```toml
[dependencies]
axum = "0.8.9"
tokio = { version = "1", features = ["macros", "rt-multi-thread"] }
warded-server = { git = "https://github.com/viell-dev/slop-warded-server", tag = "0.1.0-alpha.1" }
```

```rust,no_run
use axum::{http::Version, routing::any};
use warded_server::{
    Server,
    warded::{
        Matcher, Router,
        matchers::{exact_path, get, version},
    },
};

#[tokio::main]
async fn main() -> Result<(), warded_server::Error> {
    let routes = Router::new()
        .route(
            exact_path("/").and(version(Version::HTTP_3)),
            any(|| async { "You are on HTTP/3" }),
        )
        .route(get().and(exact_path("/")), any(|| async { "Hello" }));
    Server::from_env()?.serve(warded_server::app(routes)).await
}
```

`serve` takes any infallible Tower service, so an ordinary `axum::Router` works in place of the
Warded routes. `warded_server::app` turns Warded routes into an `axum::Router`, so Axum and
`tower-http` layers apply to them.

## What it does

- **Three protocols, one address.** HTTP/1.1 and HTTP/2 over TLS on TCP, and HTTP/3 over QUIC on the
  same UDP port. HTTPS responses advertise HTTP/3 with `Alt-Svc`.
- **The same request on each.** Handlers see the real version, the client's address and Warded's
  `TransportSecurity`. Split cookies are joined, and every response has `Date` and, where it is
  known, `Content-Length`.
- **Streaming both ways**, with trailers, and errors that a client cannot mistake for the end.
- **Redirects from HTTP**, or the application itself on the cleartext listener.
- **Certificate reload** for new connections of all protocols, with `SIGHUP` on Unix or
  `Tls::reload`.
- **Graceful shutdown** on Ctrl+C or `SIGTERM`: requests in progress finish within a grace period.
- **Limits** on connections, concurrent requests, head size and stalled clients.

What it leaves to you, and what does not work yet, is in [limits](./docs/limits.md).

## Documentation

- [Configuration](./docs/configuration.md): environment variables, certificates and HTTP/3 in
  browsers.
- [Limits](./docs/limits.md): known weaknesses, tested environments and missing parts.
- [Changelog](./CHANGELOG.md).
- API reference: `cargo doc --open` in a project that uses it.
- [Warded's guides](https://viell-dev.github.io/slop-warded-router/) for everything about routes.

To remove the server from a project, delete the `warded-server` dependency and serve the same
`axum::Router` with `axum::serve`. Warded routes also work there; see
[Tower and Axum](https://viell-dev.github.io/slop-warded-router/tower.html).

## Reporting problems

Report defects through [GitHub issues](https://github.com/viell-dev/slop-warded-server/issues), with
the version, the operating system, the protocol, a synthetic request and application, and the
expected and actual results. Report suspected vulnerabilities privately as the
[security policy](./SECURITY.md) describes.

## License

Licensed under either of [LICENSE-MIT](./LICENSE-MIT) or [LICENSE-APACHE](./LICENSE-APACHE), at your
option.
