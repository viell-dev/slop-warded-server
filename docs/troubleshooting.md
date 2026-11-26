---
description: "Fix start-up errors, certificate warnings, missing HTTP/3, unexpected 404 and 500 responses, and slow shutdown."
---

# Troubleshooting

Start with more logging, which names the reason for most of what follows. For PowerShell, see
[Shell and curl](./getting-started.md#shell-and-curl).

```sh
RUST_LOG=info,warded_server=debug,tower_http=debug cargo run
```

## It does not build

| Message                                                               | Cause and fix                                                                            |
| --------------------------------------------------------------------- | ---------------------------------------------------------------------------------------- |
| `failed to run custom build command for aws-lc-sys`                   | There is no C compiler; see [what you need](./getting-started.md#what-you-need)          |
| `package … requires rustc 1.99.1`                                     | Run `rustup update`                                                                      |
| `Access is denied. (os error 5)` when rebuilding on Windows           | Stop the running server first so Cargo can replace its executable                        |
| `the http3 feature needs panic = "unwind"`                            | Remove `panic = "abort"` from `Cargo.toml`, or turn off [HTTP/3](./http3.md#turn-it-off) |
| `expected Infallible` (or `!`), `found DispatchError<…>`              | Pass Warded routes through `warded_server::app` before `serve`                           |
| `AddExtension<Router, ConnectInfo<…>>: IntoResponse` is not satisfied | Pass the router itself; the client's address is there without it                         |
| `MethodRouter<…>: Service<…>` is not satisfied                        | The handler uses `State`: add `.with_state(state)` to the destination                    |
| `fn(…) -> impl Future … {…}: Service<…>` is not satisfied             | Wrap it: `any(handler)`                                                                  |
| `Result<PathMatcher, …>: Matcher` is not satisfied                    | Add `?` after `path(…)`                                                                  |
| `the name get is defined multiple times`                              | `axum::routing` and `matchers` both have it; import one under another name               |
| A handler "does not implement `Handler`"                              | Turn on axum's `macros` feature and add `#[axum::debug_handler]` for a better message    |

## It does not start

| Message                                                                | Cause and fix                                                                                                                              |
| ---------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------ |
| `cannot listen on … for HTTPS over TCP: Address already in use`        | Something uses the port, often another copy of the server                                                                                  |
| `cannot listen on … for development over HTTP: Address already in use` | Another project uses `127.0.0.1:3000`; stop it, or set `WARDED_DEV_ADDR` to another port                                                   |
| `cannot listen on … for HTTP/3 over UDP: Address already in use`       | Something uses the UDP port; choose another, or turn off HTTP/3                                                                            |
| `cannot listen on …: Permission denied`                                | Ports below 1024 need a permission; see [deployment](./deployment.md#systemd)                                                              |
| `WARDED_TLS_CERT is needed …`                                          | It listens beyond this computer, where it needs a [certificate](./certificates.md)                                                         |
| `WARDED_HTTPS_ADDR is not an address and port`                         | Use an IP address with a port, such as `0.0.0.0:8443`, and not a host name                                                                 |
| `WARDED_DEV_ADDR must be a loopback address …`                         | It has no TLS; use `127.0.0.1` or `[::1]`, and HTTPS for others; see [the development address](./configuration.md#the-development-address) |
| `the development address … must be a loopback address …`               | The same, for `Server::development` in code                                                                                                |
| `cannot read …: I/O error: No such file or directory`                  | The path is wrong, or relative to another directory than you think                                                                         |
| `cannot read …: I/O error: Permission denied`                          | The program's user may not read the file; see [deployment](./deployment.md#systemd)                                                        |
| `cannot read …: no items found`                                        | The file holds no certificate, or no private key, in PEM form                                                                              |
| `the limit … is zero or too large`                                     | See [limits](./configuration.md#limits)                                                                                                    |
| `keys may not be consistent: KeyMismatch`                              | The key belongs to another certificate, or the chain is in the wrong order                                                                 |

## Clients cannot connect

- **The browser warns about the certificate.** It is the generated one. Open the address that the
  log names with `open http://…` instead, or see [certificates for
  development](./certificates.md#for-development).
- **A 403 that says `The development address answers only requests for an IP address or
  localhost`.** The browser used another name for this computer, such as one from the hosts file.
  Open the address that the log names, or a name below `.localhost`; see [the development
  address](./configuration.md#the-development-address).
- **A 403 that says `The development address answers no requests that other sites make`.** A page
  from another site, or another origin, sent the request. Load the page from the development
  address itself.
- **The log names no `http://` address.** The project has no
  [development address](./configuration.md#the-development-address): its `.cargo/config.toml` is
  missing, it was started without Cargo, or `WARDED_DEV_ADDR` is set to something else.
- **`localhost` works on this computer, but not from another.** The default address only accepts
  this computer; see [addresses](./configuration.md#addresses).
- **Clients speak plain HTTP to the HTTPS port.** curl says `Received HTTP/0.9 when not allowed`,
  and browsers show an empty or invalid response. Use `https://`.
- **Some clients are refused under load.** The [connection limit](./configuration.md#limits) is
  reached.

## Browsers do not use HTTP/3

Check in this order:

1. From the same network, try `curl --http3-only` with a
   [curl build that supports HTTP/3](./getting-started.md#shell-and-curl). If it cannot reach the
   server, UDP is blocked, or the log has the reason.
2. `curl --head` shows an `alt-svc` field with the port that clients use. If the port is wrong, set
   `WARDED_PUBLIC_PORT`.
3. The certificate comes from a public authority; see [in browsers](./http3.md#in-browsers).
4. Load the page again after a few seconds. The first connection is always HTTP/2 or HTTP/1.1.

## The page does not reload

- **It is not on the development address.** Only pages from the `http://` address that the log
  names reload, and not those over HTTPS.
- **It is not HTML.** Text and JSON stay as they are; reload them yourself.
- **Nothing restarted the server.** The page reloads when the server starts again. Run
  [bacon](https://dystroy.org/bacon/) with `bacon run-long`, which restarts it after each change.
  Files outside `src/` count only if `bacon.toml` names them in `watch`, and files that Git ignores
  never do, also where the project is inside an ignored directory of another repository.
- **A script fetched it.** The reload script is added to pages that the browser opens, and not to
  HTML that scripts fetch, or that is compressed already.

## A 404 that should not be

- **The path differs**, often by a slash at the end. Paths match exactly.
- **The method differs.** `get()` does not include HEAD; browsers and monitors send it. Use
  `get().or(head())`.
- **A condition on the version or a header failed.** Such routes are skipped, and another may
  answer. On the development address, the version is HTTP/1.1, and `secured()` does not hold.
- **The pattern is not one.** `path("/orders/:id")` and `exact_path("/orders/{id}")` are literal
  text; write `path("/orders/{id}")`.
- **The destination is an Axum router** that has its own 404. Warded does not try further routes
  after it.

Warded's [troubleshooting](https://viell-dev.github.io/slop-warded-router/troubleshooting.html)
covers routes in detail.

## A 400 or 500 from an extractor

| Response or log                                                                         | Cause and fix                                                                                                           |
| --------------------------------------------------------------------------------------- | ----------------------------------------------------------------------------------------------------------------------- |
| `invalid path: missing field …`                                                         | The fields of the type in `Params` are not the names in the pattern                                                     |
| `invalid path: invalid type: map, …`                                                    | `Params` takes a struct, also for one capture                                                                           |
| `invalid path: id: invalid digit found in string`                                       | A capture is not a number. A fixed path such as `/notes/count` goes [above the pattern](./requests.md#declaring-routes) |
| `Invalid URL: missing field …`, or a 500 with `Wrong number of path arguments for Path` | The handler uses Axum's `Path` in a Warded route; use `Params`                                                          |
| `Params needs a handler that a Warded route selected`                                   | The handler is in an Axum router; use `Path` there                                                                      |
| `Missing request extension`                                                             | A test without the server has to insert what it adds; see [testing](./testing.md)                                       |

## Other 500s

| Log                                                           | Cause and fix                                                                                                                                             |
| ------------------------------------------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `could not select a route … trusted HTTPS context is missing` | A test without the server has to insert [`TransportSecurity`](./testing.md), or the project uses two versions of `warded`; import it from `warded_server` |
| `the application responded with an informational status`      | The handler returned a 1xx status over HTTP/3                                                                                                             |

## Requests fail without a response

The handler panicked. Add [`CatchPanicLayer`](./middleware.md) to answer with a 500, and find the
panic in the output.

## Shutdown takes long

Press Ctrl+C again to end it at once.

Requests in progress have 30 seconds. Event streams and other open responses use all of it unless
the application ends them; see [streaming](./streaming.md#server-sent-events). Without those, HTTP/3
clients add about two seconds.

## The server ends without a word

`SIGHUP` ends a program that uses `serve_until` and does not handle it; see
[existing Axum applications](./existing-axum.md#decide-when-to-stop). Closing a terminal sends it.

## Errors in the log that are normal

At the `debug` level, `an HTTP/3 connection ended` with `aborted by peer` is how most HTTP/3
clients, curl among them, close their connection.

## Report it

If it still looks like a defect, [open an
issue](https://github.com/viell-dev/slop-warded-server/issues) with the version, operating system,
protocol, a small application that shows it, and what you expected.
