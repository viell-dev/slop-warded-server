---
description: "Every environment variable and setting: addresses, certificates, HTTP/3, limits, shutdown and logging."
---

# Configuration

`Server::from_env()` reads environment variables, and the methods of `Server` set the rest in code.
Use either or both:

| Variable             | Method          | Meaning                                       | Default          |
| -------------------- | --------------- | --------------------------------------------- | ---------------- |
| `WARDED_HTTPS_ADDR`  | `https`         | HTTPS address, over TCP and UDP               | `127.0.0.1:8443` |
| `WARDED_HTTP_ADDR`   | `redirect_http` | Cleartext address that redirects to HTTPS     | none             |
| `WARDED_DEV_ADDR`    | `development`   | [Development address](#the-development-address), on loopback | none |
| `WARDED_TLS_CERT`    | `Server::new`   | PEM certificate chain file                    | generated        |
| `WARDED_TLS_KEY`     | `Server::new`   | PEM private key file                          | generated        |
| `WARDED_HTTP3`       | `http3`         | `on` or `off`                                 | `on`             |
| `WARDED_PUBLIC_PORT` | `public_port`   | HTTPS port that clients use, if it differs    | the bound port   |
|                      | `serve_http`    | Cleartext address that serves the application | none             |
|                      | `live_reload`   | Pages that reload on the development address  | on               |
|                      | `limits`        | [Limits](#limits) on clients                  | see below        |
|                      | `grace_period`  | [Time to finish](#stopping) when stopping     | 30 seconds       |

The templates set `WARDED_DEV_ADDR` to `127.0.0.1:3000` in `.cargo/config.toml`.

In code:

```rust,no_run
use axum::{Router, routing::get};
use std::time::Duration;
use warded_server::{Limits, Server};

#[tokio::main]
async fn main() -> Result<(), warded_server::Error> {
    let app = Router::new().route("/", get(|| async { "Hello" }));
    let mut limits = Limits::default();
    limits.connections = 2_000;
    Server::from_env()?
        .limits(limits)
        .grace_period(Duration::from_secs(10))
        .serve(app)
        .await
}
```

## Addresses

An address is an IP address and a port, and not a host name:

| Address          | Who can connect                                             |
| ---------------- | ----------------------------------------------------------- |
| `127.0.0.1:8443` | Programs on this computer                                   |
| `0.0.0.0:443`    | Everyone who can reach this computer over IPv4              |
| `[::]:443`       | Everyone who can reach it over IPv6 or IPv4                 |

Ports below 1024 need a permission on most systems; see [deployment](./deployment.md). HTTP/3 uses
UDP on the HTTPS port. If that port is taken, the server does not start.

On a loopback address, the server generates a certificate when you give it none. Anywhere else it
needs [a real one](./certificates.md), so that a missing setting cannot put a server that nobody
should trust on a network.

## The development address

`WARDED_DEV_ADDR`, or `development` in code, serves the application over plain HTTP too, for a
browser on this computer. Browsers open `http://127.0.0.1` without a certificate, and treat it as a
secure context, as they do `http://localhost`. The server logs the address to open:

```text
INFO warded_server::server: open http://127.0.0.1:3000/ in a browser
```

It has to be a loopback address, such as `127.0.0.1` or `[::1]`, because nothing on it is
encrypted. The log names the IP address rather than `localhost`, which some systems resolve to
IPv6 first. Things to know about it:

- **Browsers use HTTP/1.1 there.** Clients that begin with HTTP/2 without TLS (h2c), such as
  `curl --http2-prior-knowledge`, can use that. Use HTTPS for HTTP/2 in browsers and for HTTP/3;
  it keeps running at the same time.
- **Only for this computer, also in the browser.** Any page in the browser can make it send
  requests to this address, so the address answers 403 to those that another site could have
  caused:
  - **for another host** than an IP address, `localhost` or a name below `.localhost`, such as
    `app.localhost`. Another site's DNS name can lead to `127.0.0.1` (DNS rebinding), and so do
    names in the hosts file, which are refused too. Open the address that the log names, or use a
    name below `.localhost`.
  - **from another site**, as the browser says with `Sec-Fetch-Site: cross-site`, unless the request
    opens a page with GET or HEAD. Links to the address from elsewhere work; scripts, forms that
    send POST, and `<object>` or `<embed>` on other sites do not.
  - **changes from another origin**, which also covers browsers that send no `Sec-Fetch-Site`:
    methods other than GET, HEAD, OPTIONS and TRACE, and WebSockets, whose `Origin` is not on this
    computer. A page from another computer's IP address may not send them. One from
    `http://localhost:5173` may send them to `http://localhost:3000`, which the browser counts as
    the same site, but not to `http://127.0.0.1:3000`, which is another site to it. A development
    server's proxy, such as Vite's, passes the browser's same-origin requests on and is not
    affected.

  curl and other clients that send none of these fields are not affected. Each refusal is logged
  as a warning.
- **Requests are not secured**, so routes with Warded's `secured()` condition do not match, and
  those with `unsecure()` do.
- **HTML pages reload by themselves** when the server starts again. Each page that a browser opens
  there gets a small script, which waits on an event stream at `/_tower-livereload/event-stream`
  for the server to go away and come back. Fragments that scripts fetch do not get it. Responses are
  not compressed there, so that the script can be added. Turn it off with `live_reload(false)`, for
  tools that bring their own, such as Vite or cargo-leptos.
- **It is apart from `WARDED_HTTP_ADDR` and `serve_http`**, and can be used with either.

In a project from a template, `.cargo/config.toml` sets the variable for the programs that Cargo
starts: `cargo run`, `cargo test` and watchers that run them. A program that is built and started
on its own, as in a [deployment](./deployment.md), does not get it. `cargo run --release` does, so
do not deploy with Cargo. A variable that is set already takes precedence, so
`WARDED_DEV_ADDR= cargo run` turns the address off.

The templates' READMEs show how [bacon](https://dystroy.org/bacon/) restarts the server after each
change: `cargo install --locked bacon` once, then `bacon run-long` in place of `cargo run`.

## Cleartext HTTP

`WARDED_HTTP_ADDR` adds a listener that redirects everything to the same address over HTTPS, with a
308, which keeps the method and body. It names the host that the client asked for.

With `serve_http` in code, the application answers there instead, and routes can tell the difference
with `unsecure()`. That listener also accepts HTTP/2 from clients that start with it (h2c).

The server does not trust headers from proxies. Behind one that ends TLS, all requests on this
address count as insecure, unless [your application trusts it](./deployment.md#a-proxy-that-ends-tls).

## Limits

Limits protect the server from clients, not from your handlers:

| Field                | Meaning                                              | Default    |
| -------------------- | ---------------------------------------------------- | ---------- |
| `connections`        | Open connections of all kinds                        | 10 000     |
| `concurrent_streams` | Requests at the same time on one HTTP/2 or HTTP/3 connection | 128 |
| `head_bytes`         | Size of a request head                               | 32 KiB     |
| `handshake_timeout`  | Time for a TLS or QUIC handshake                     | 10 seconds |
| `head_timeout`       | Time for a request head, on HTTP/2 only the first; idle time on HTTP/1.1   | 30 seconds |
| `idle_timeout`       | Silence on HTTP/2 and HTTP/3 connections             | 30 seconds |

Each connection uses a file descriptor. On Linux, allow more of them than `connections`, for example
with `LimitNOFILE` in [systemd](./deployment.md#systemd).

None of them may be zero, a timeout can be at most a year, a head at most 512 KiB, and there can be
at most 10 000 concurrent streams, because every HTTP/3 connection takes memory for them. Nothing
limits how long a client takes to send a body or to read a response, as long as its connection is
alive. [Limits](./limits.md#connections) adds up what a connection can hold.
[Middleware](./middleware.md) limits bodies, time and busy handlers.

## Stopping

Ctrl+C or `SIGTERM` stops the server from accepting connections. Requests in progress have the grace
period to finish, and then their connections are closed. Ask a second time to close them at once.
Idle connections are closed early, which takes a few seconds when there are HTTP/3 clients, or
others that do not answer.

Make whatever manages the process wait longer than the grace period before it kills it.

## Logging

The server reports through [`tracing`](https://docs.rs/tracing), and a subscriber such as
`tracing-subscriber` decides what happens with it. `warded_server::init_logging()`, which the
templates call first in `main`, prints to the terminal at the `info` level, or as `RUST_LOG` says:

```sh
RUST_LOG=info,warded_server=debug cargo run
```

With `DefaultMiddleware`, which the `api` and `website` templates use, each request is logged at
`info` too.

Failed connections and malformed requests are reported at `debug`, because any client can cause
them. Errors that need your attention, such as a certificate that cannot be reloaded, are at `warn`
and `error`.
