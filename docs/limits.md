---
description: "Known weaknesses of HTTP/3, what your application must provide, what is not supported yet, and what has been tested."
---

# Limits

## HTTP/3

HTTP/3 comes from [h3](https://github.com/hyperium/h3), which calls itself experimental. Its latest
release, 0.0.8, has these open defects:

- **Memory exhaustion.** A client can declare a huge frame and make the server hold all of it in
  memory for as long as it keeps sending ([h3#366](https://github.com/hyperium/h3/pull/366)).
  `Limits::head_timeout` ends this until the head of a request has arrived. It does not end it for
  what follows, such as trailers and unknown frames, nor on a connection's control stream. Nothing
  else here bounds it ([#3](https://github.com/viell-dev/slop-warded-server/issues/3)).
- **Cancelled requests.** A handler is not cancelled when the client gives up on one request, only
  when the whole connection ends ([h3#352](https://github.com/hyperium/h3/issues/352)).
- **Request heads that never arrive.** The server gives up on each one, but h3 remembers it until
  the connection ends ([h3#365](https://github.com/hyperium/h3/pull/365), fixed there but not
  released).
- **Malformed heads.** h3 accepts pseudo-header fields that are repeated or come after other fields,
  and uses the last of each. The server refuses fields about the connection and unclear lengths, but
  cannot see these.
- **Empty DATA frames.** A request body that contains one fails
  ([h3#372](https://github.com/hyperium/h3/issues/372)).

The server works around others. It limits decoded heads
([h3#379](https://github.com/hyperium/h3/issues/379)), and it contains a panic that a malformed body
causes ([h3#386](https://github.com/hyperium/h3/issues/386)), although Rust still reports that
panic. Containing it needs unwinding, so the `http3` feature refuses to compile with
`panic = "abort"`.

`WARDED_HTTP3=off` [turns HTTP/3 off](./http3.md#turn-it-off). WebSockets, WebTransport and other
uses of CONNECT are not supported over HTTP/3, and 0-RTT requests are never accepted.

## What the application provides

| Concern                                  | Where                                                                  |
| ---------------------------------------- | ---------------------------------------------------------------------- |
| Limits on bodies, time and busy handlers | [Middleware](./middleware.md). Axum's extractors limit bodies to 2 MB  |
| Authentication and sessions              | [Middleware or handlers](./json-api.md#require-authentication-on-some-routes) |
| CORS and security headers                | [Middleware](./middleware.md#security-headers)                         |
| Compression, logging, 500 for panics     | [Middleware](./middleware.md), in the `api` and `website` templates    |
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
- **gRPC** has not been tried.

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

At the end of a TCP connection the server waits until the client closes it too, because the client
could otherwise lose the end of a response. It stops waiting when the client has been silent for
five seconds, or after 30 seconds. At the end of an HTTP/3 connection the server waits until it has sent nothing for two seconds, or
the client closes it, so that the last response is not lost.

## Tested environments

- **In CI:** Rust 1.99.1 on Linux and Windows, with real HTTP/1.1, HTTP/2 and HTTP/3 connections
  over loopback from Rust clients, and each template's project on Linux.
- **Before a release, on Linux:** curl with each protocol and with the upgrade through `Alt-Svc`,
  and Chromium over HTTP/3.
- **Not tested:** macOS, real networks, load, other HTTP/3 clients, and applications built with
  Leptos, Dioxus or Tonic.
