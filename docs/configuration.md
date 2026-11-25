---
description: "Every environment variable and setting: addresses, certificates, HTTP/3, limits, shutdown and logging."
---

# Configuration

`Server::from_env()` reads environment variables, and the methods of `Server` set the rest in code.
Use either or both:

| Variable             | Method           | Meaning                                    | Default          |
| -------------------- | ---------------- | ------------------------------------------ | ---------------- |
| `WARDED_HTTPS_ADDR`  | `https`          | HTTPS address, over TCP and UDP            | `127.0.0.1:8443` |
| `WARDED_HTTP_ADDR`   | `redirect_http`  | Cleartext address that redirects to HTTPS  | none             |
| `WARDED_TLS_CERT`    | `Server::new`    | PEM certificate chain file                 | generated        |
| `WARDED_TLS_KEY`     | `Server::new`    | PEM private key file                       | generated        |
| `WARDED_HTTP3`       | `http3`          | `on` or `off`                              | `on`             |
| `WARDED_PUBLIC_PORT` | `public_port`    | HTTPS port that clients use, if it differs | the bound port   |
|                      | `serve_http`     | Cleartext address that serves the application | none          |
|                      | `limits`         | [Limits](#limits) on clients               | see below        |
|                      | `grace_period`   | [Time to finish](#stopping) when stopping  | 30 seconds       |

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

None of them may be zero, a timeout can be at most a year, and there can be at most 10 000
concurrent streams, because every HTTP/3 connection takes memory for them. Nothing limits how long a
client takes to send a body or to read a response, as long as its connection is alive.
[Middleware](./middleware.md) limits bodies, time and busy handlers.

## Stopping

Ctrl+C or `SIGTERM` stops the server from accepting connections. Requests in progress have the grace
period to finish, and then their connections are closed. Ask a second time to close them at once.
Idle connections are closed early, which takes a few seconds when there are HTTP/3 clients, or
others that do not answer.

Make whatever manages the process wait longer than the grace period before it kills it.

## Logging

The server reports through [`tracing`](https://docs.rs/tracing), and a subscriber such as
`tracing-subscriber` decides what happens with it. The templates install one that reads `RUST_LOG`:

```sh
RUST_LOG=info,warded_server=debug cargo run
```

Failed connections and malformed requests are reported at `debug`, because any client can cause
them. Errors that need your attention, such as a certificate that cannot be reloaded, are at `warn`
and `error`.
