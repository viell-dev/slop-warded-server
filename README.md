# Warded Server

A ready-wired Rust server for HTTP/1.1, HTTP/2 and HTTP/3. It assembles
[Axum](https://docs.rs/axum), [Hyper](https://hyper.rs), [rustls](https://docs.rs/rustls) and
[Quinn](https://docs.rs/quinn) with the
[Warded](https://github.com/viell-dev/slop-warded-router) router, so that a new project starts from
its routes and handlers.

**Alpha.** The API, the templates and the configuration still change, and HTTP/3 rests on an
experimental library with [known weaknesses][limits]. Do not put it on the open
internet yet.

Agents write and maintain this project. No person reviews the code. See
[CONTRIBUTING.md](./CONTRIBUTING.md) for contribution and verification expectations.

## Start a project

You need Rust 1.99.1 or later and
[cargo-generate](https://cargo-generate.github.io/cargo-generate/installation.html):

```sh
cargo install cargo-generate --locked
cargo generate viell-dev/slop-warded-server --tag 0.1.0-alpha.3
```

It asks which template to use and what to call the project. To skip the first question, name the
template: `cargo generate viell-dev/slop-warded-server templates/api --tag 0.1.0-alpha.3`.

| Template            | What you get                                                             |
| ------------------- | ------------------------------------------------------------------------ |
| `templates/minimal` | One route and nothing else                                               |
| `templates/api`     | A JSON API with state, errors, logging, compression and tests            |
| `templates/website` | A small site: pages with a menu, a form, static files, logging and tests |

Then run it, and open the address that it logs, `http://127.0.0.1:3000/`:

```sh
cargo run
```

That address is plain HTTP for a browser on this computer, without a certificate warning, and its
pages reload when the server restarts. HTTPS, with HTTP/2 and HTTP/3, runs beside it:

```sh
curl --insecure --http2 https://localhost:8443/
```

Without a certificate the server generates one that no client trusts, hence `--insecure`.
[Getting started] explains the steps, how to restart the server after each change, and how to add
the server to a project of your own.

## Example

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
  `TransportSecurity`. Split cookies are joined, and requests that the HTTP specifications call
  malformed are refused before routing. Every response has `Date` and, where it is known, the true
  `Content-Length`, unless the application chose `Transfer-Encoding` over HTTP/1.1.
- **Streaming both ways**, with trailers, and errors that a client cannot mistake for the end.
- **Redirects from HTTP**, or the application itself on the cleartext listener.
- **A development address** over plain HTTP on loopback, whose pages reload when the server
  restarts.
- **Helpers for a start:** logging, default middleware, static files and single-page apps, over
  `tracing-subscriber`, `tower-http` and Axum.
- **Certificate reload** for new connections of all protocols, with `SIGHUP` on Unix or
  `Tls::reload`.
- **Graceful shutdown** on Ctrl+C or `SIGTERM`: requests in progress finish within a grace period,
  which a second signal ends.
- **Limits** on connections, concurrent requests and head size, and timeouts for stalled handshakes
  and request heads.

What it leaves to you, and what does not work yet, is in [limits].

## Documentation

The [documentation site][site] describes the latest release:

- **Projects:** [a JSON API], [a website], [a single-page app], and [existing Axum applications].
- **Writing it:** [routes and requests], [middleware], [streaming and WebSockets], and [testing].
- **Running it:** [configuration], [certificates], [HTTP/3], and [deployment].
- **Help:** [troubleshooting], [limits], and [versions].
- The [API reference] and the [changelog](./CHANGELOG.md).

[Warded's own guides](https://viell-dev.github.io/slop-warded-router/) cover routes in full.

## Reporting problems

Report defects through [GitHub issues](https://github.com/viell-dev/slop-warded-server/issues), with
the version, the operating system, the protocol, a synthetic request and application, and the
expected and actual results. Report suspected vulnerabilities privately as the
[security policy](./SECURITY.md) describes.

## License

Licensed under either of [LICENSE-MIT](./LICENSE-MIT) or [LICENSE-APACHE](./LICENSE-APACHE), at your
option.

[site]: https://viell-dev.github.io/slop-warded-server/
[getting started]: https://viell-dev.github.io/slop-warded-server/getting-started.html
[a JSON API]: https://viell-dev.github.io/slop-warded-server/json-api.html
[a website]: https://viell-dev.github.io/slop-warded-server/website.html
[a single-page app]: https://viell-dev.github.io/slop-warded-server/single-page-app.html
[existing Axum applications]: https://viell-dev.github.io/slop-warded-server/existing-axum.html
[routes and requests]: https://viell-dev.github.io/slop-warded-server/requests.html
[middleware]: https://viell-dev.github.io/slop-warded-server/middleware.html
[streaming and WebSockets]: https://viell-dev.github.io/slop-warded-server/streaming.html
[testing]: https://viell-dev.github.io/slop-warded-server/testing.html
[configuration]: https://viell-dev.github.io/slop-warded-server/configuration.html
[certificates]: https://viell-dev.github.io/slop-warded-server/certificates.html
[HTTP/3]: https://viell-dev.github.io/slop-warded-server/http3.html
[deployment]: https://viell-dev.github.io/slop-warded-server/deployment.html
[troubleshooting]: https://viell-dev.github.io/slop-warded-server/troubleshooting.html
[limits]: https://viell-dev.github.io/slop-warded-server/limits.html
[versions]: https://viell-dev.github.io/slop-warded-server/versions.html
[API reference]: https://viell-dev.github.io/slop-warded-server/api/warded_server/
