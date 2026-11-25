---
description: "Fix start-up errors, certificate warnings, missing HTTP/3, unexpected 404 and 500 responses, and slow shutdown."
---

# Troubleshooting

Start with more logging, which names the reason for most of what follows:

```sh
RUST_LOG=info,warded_server=debug,tower_http=debug cargo run
```

## It does not build

| Message                                                  | Cause and fix                                                                 |
| -------------------------------------------------------- | ----------------------------------------------------------------------------- |
| `failed to run custom build command for aws-lc-sys`      | There is no C compiler; see [what you need](./getting-started.md#what-you-need) |
| `package … requires rustc 1.99.1`                        | Run `rustup update`                                                           |
| `the http3 feature needs panic = "unwind"`               | Remove `panic = "abort"` from `Cargo.toml`, or turn off [HTTP/3](./http3.md#turn-it-off) |
| `expected Infallible, found DispatchError<Infallible>`   | Pass Warded routes through `warded_server::app` before `serve`                |
| A handler "does not implement `Handler`"                 | Turn on axum's `macros` feature and add `#[axum::debug_handler]` for a better message |

## It does not start

| Message                                                    | Cause and fix                                                               |
| ---------------------------------------------------------- | --------------------------------------------------------------------------- |
| `cannot listen on … for HTTPS over TCP: Address already in use` | Something uses the port, often another copy of the server              |
| `cannot listen on … for HTTP/3 over UDP: Address already in use` | Something uses the UDP port; choose another, or turn off HTTP/3       |
| `cannot listen on …: Permission denied`                    | Ports below 1024 need a permission; see [deployment](./deployment.md#systemd) |
| `WARDED_TLS_CERT is needed …`                              | It listens beyond this computer, where it needs a [certificate](./certificates.md) |
| `WARDED_HTTPS_ADDR is not an address and port`             | Use an IP address with a port, such as `0.0.0.0:8443`, and not a host name  |
| `cannot read …: No such file or directory`                 | The path is wrong, or relative to another directory than you think          |
| `cannot read …: Permission denied`                         | The program's user may not read the file; see [deployment](./deployment.md#systemd) |
| `cannot read …: no items found`                            | The file holds no certificate, or no private key, in PEM form               |
| `the limit … is zero or too large`                         | See [limits](./configuration.md#limits)                                     |
| `keys may not be consistent: KeyMismatch`                  | The key belongs to another certificate, or the chain is in the wrong order  |

## Clients cannot connect

- **The browser warns about the certificate.** It is the generated one; see [certificates for
  development](./certificates.md#for-development).
- **`localhost` works on this computer, but not from another.** The default address only accepts
  this computer; see [addresses](./configuration.md#addresses).
- **Some clients are refused under load.** The [connection limit](./configuration.md#limits) is
  reached.

## Browsers do not use HTTP/3

Check in this order:

1. `curl --http3-only` reaches the server from the same network. If not, UDP is blocked, or the log
   has the reason.
2. `curl --head` shows an `alt-svc` field with the port that clients use. If the port is wrong, set
   `WARDED_PUBLIC_PORT`.
3. The certificate comes from a public authority; see [in browsers](./http3.md#in-browsers).
4. Load the page again after a few seconds. The first connection is always HTTP/2 or HTTP/1.1.

## A 404 that should not be

- **The path differs**, often by a slash at the end. Paths match exactly.
- **The method differs.** `get()` does not include HEAD; browsers and monitors send it. Use
  `get().or(head())`.
- **A condition on the version or a header failed.** Such routes are skipped, and another may
  answer.
- **The destination is an Axum router** that has its own 404. Warded does not try further routes
  after it.

Warded's [troubleshooting](https://viell-dev.github.io/slop-warded-router/troubleshooting.html)
covers routes in detail.

## A 500

| Log                                                       | Cause and fix                                                             |
| --------------------------------------------------------- | ------------------------------------------------------------------------- |
| `could not select a route … trusted HTTPS context is missing` | A test without the server has to insert [`TransportSecurity`](./testing.md), or the project uses two versions of `warded`; import it from `warded_server` |
| `Params needs a handler that a Warded route selected`     | The handler is in an Axum router; use `Path` there                        |
| `No paths parameters found for matched route`, as the body | The handler uses Axum's `Path` in a Warded route; use `Params`           |
| `the application responded with an informational status`  | The handler returned a 1xx status over HTTP/3                             |

## Requests fail without a response

The handler panicked. Add [`CatchPanicLayer`](./middleware.md) to answer with a 500, and find the
panic in the output.

## Shutdown takes long

Press Ctrl+C again to end it at once.

Requests in progress have 30 seconds. Event streams and other open responses use all of it unless
the application ends them; see [streaming](./streaming.md#server-sent-events). Without those, HTTP/3
clients add about two seconds.

## Report it

If it still looks like a defect, [open an
issue](https://github.com/viell-dev/slop-warded-server/issues) with the version, operating system,
protocol, a small application that shows it, and what you expected.
