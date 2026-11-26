---
description: "Known weaknesses of HTTP/3, what your application must provide, what is not supported yet, and what has been tested."
---

# Limits

## HTTP/3

HTTP/3 comes from [h3](https://github.com/hyperium/h3), which calls itself experimental. Its latest
release, 0.0.8, has these open defects:

- **Cancelled requests.** A handler is not cancelled when the client gives up on one request, only
  when the whole connection ends ([h3#352](https://github.com/hyperium/h3/issues/352)).
- **Malformed heads.** h3 accepts pseudo-header fields that are repeated or come after other fields,
  and uses the last of each. The server refuses fields about the connection, unclear lengths,
  whitespace around values and hosts that disagree, but cannot see these.

The server works around others, most of which let a client fill its memory. It gives h3 one frame at
a time, and refuses one that declares too much
([h3#366](https://github.com/hyperium/h3/pull/366),
[h3#344](https://github.com/hyperium/h3/pull/344)). It limits decoded heads
([h3#379](https://github.com/hyperium/h3/issues/379)), and shuts down a connection on which more
than 1 000 request heads did not arrive ([h3#365](https://github.com/hyperium/h3/pull/365)). h3 has
also panicked at what clients send ([h3#386](https://github.com/hyperium/h3/issues/386)). Such a
panic ends one request or connection, which needs unwinding, so the `http3` feature refuses to
compile with `panic = "abort"`.

`head_bytes` counts the fields of a head once they are decoded. A head above it gets a 431, as on
the other protocols. One that is above twice as much while it is still encoded is refused before it
arrives, and its request fails without a response.

`WARDED_HTTP3=off` [turns HTTP/3 off](./http3.md#turn-it-off). WebSockets, WebTransport and other
uses of CONNECT are not supported over HTTP/3, and 0-RTT requests are never accepted.

## What the application provides

| Concern                                  | Where                                                                  |
| ---------------------------------------- | ---------------------------------------------------------------------- |
| Limits on bodies, time and busy handlers | [Middleware](./middleware.md). Axum's extractors limit bodies to 2 MB  |
| Authentication and sessions              | [Middleware or handlers](./json-api.md#require-authentication-on-some-routes) |
| CORS and security headers                | [Middleware](./middleware.md#security-headers)                         |
| Compression, logging, 500 for panics     | [`DefaultMiddleware`](./middleware.md), in the `api` and `website` templates |
| Trust in a proxy that ends TLS           | [Middleware](./deployment.md#a-proxy-that-ends-tls)                    |
| Databases, background work, e-mail       | Libraries of your choice                                               |

## Not supported yet

- **Obtaining certificates.** Bring your own, for example from
  [Certbot](./certificates.md#for-the-public)
  ([#4](https://github.com/viell-dev/slop-warded-server/issues/4)).
- **More than one certificate or HTTPS address** in one server.
- **WebSockets over HTTP/2.** They work over HTTP/1.1.
- **HEAD with compression** for files and for [some destinations](./middleware.md#the-response-to-head).
  The head of the response is that of the uncompressed one.
- **Whitespace in trailers.** In the trailers of an HTTP/2 or HTTP/3 request, a field value can
  begin or end with a space or a tab, which HTTP/1.1 removes, and which the server refuses in heads.
- **A handler's own `Content-Length` for HEAD.** Where the server knows the length of the content,
  it replaces one that a handler declared, and logs a warning. A response to HEAD without content
  keeps the length that the application declared, which the server cannot check: `any(handler)`
  takes the content off, and `handler.into_service()` keeps it, as in [the response to
  HEAD](./middleware.md#the-response-to-head).
- **gRPC** has not been tried.
- **`If-Range` for files.** `StaticFiles`, `SinglePageApp` and `ServeDir` come from tower-http 0.7.1,
  which answers a range even when `If-Range` names an older version of the file, so a resumed
  download of a file that changed mixes the two. Its main branch has the fix
  ([tower-http#754](https://github.com/tower-rs/tower-http/pull/754)), which no release has yet.

## Connections

`Limits::head_timeout` covers every request head on HTTP/1.1 and HTTP/3, but only the first one of a
connection on HTTP/2.

A WebSocket fails at the end of a shutdown when its handler next uses it. The handler owns it, so
the connection stays open until the handler drops it, or the program ends.

Nothing limits how long a client takes to send a request body or to read a response, as long as its
connection is alive. Each such client holds a connection.

The default [limits](./configuration.md#limits) are not tuned for any particular machine. An HTTP/2
connection stays open as long as its client answers pings, so idle clients use up the connection
limit. When it is reached, new TCP connections are closed without a response, and new QUIC
connections are refused.

One HTTP/3 connection can make the server hold this much:

- 10 MB of requests that handlers have not read. At that point the client can send nothing more on
  the connection, new requests included, until a handler reads or ends. Data in small packets takes
  up to two and a half times as much memory, and 32 KiB more for each request;
- twice `head_bytes` for the head or trailers of each concurrent request, as they arrive, and 4 KiB
  of its body;
- 10 MB of responses that the client has not confirmed.

What handlers keep comes on top, so [limit bodies](./middleware.md) that they collect.

At the end of a TCP connection the server waits until the client closes it too, because the client
could otherwise lose the end of a response. It stops waiting when the client has been silent for
five seconds, or after 30 seconds. At the end of an HTTP/3 connection the server waits until it has
sent nothing for two seconds, or the client closes it, so that the last response is not lost.

When a handler answers without reading the body of an HTTP/3 request, the server takes and discards
the rest, because some clients could otherwise miss the response. It tells the client to stop when it
has been silent for five seconds, or after 30 seconds. A shutdown waits for that within its grace
period, before it closes the connection as above.

## The development address

The [development address](./configuration.md#the-development-address) is for a browser on the same
computer, and not for others:

- It serves plain HTTP, on loopback addresses only. Browsers use HTTP/1.1 there; h2c clients can
  use HTTP/2. HTTP/2 in browsers and HTTP/3 need HTTPS, and Warded's `secured()` condition does not
  hold there.
- It answers only requests for an IP address, `localhost` or a name below `.localhost`. A name for
  this computer in the hosts file gets a 403, as do requests that other sites make, except to open
  a page; see [configuration](./configuration.md#the-development-address). There is no setting
  for other names.
- Its responses are not compressed, so that pages can get the reload script.
- Pages reload when the server starts again, and not when a file changes by itself. A watcher such
  as bacon has to restart it, also for files that the server reads, such as those in `static/`.
- `.cargo/config.toml` applies to every program that Cargo starts, `cargo run --release` included.
  Start a deployed program without Cargo.

## Tested environments

- **In CI:** Rust 1.99.1 on Linux and Windows, with real HTTP/1.1, HTTP/2 and HTTP/3 connections
  over loopback from Rust clients, and each template's project on Linux, started with `cargo run`.
  The website template's pages are used in Google Chrome, also across a restart.
- **Before a release, on Linux:** curl with each protocol and with the upgrade through `Alt-Svc`,
  and Chromium over HTTP/3.
- **Not tested:** macOS, real networks, load, other HTTP/3 clients, and applications built with
  Leptos, Dioxus or Tonic.
