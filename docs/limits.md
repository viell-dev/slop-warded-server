# Limits

## HTTP/3

HTTP/3 comes from [h3](https://github.com/hyperium/h3), which calls itself experimental. Its latest
release, 0.0.8, has these open defects:

- **Memory exhaustion.** A client can declare a huge frame and make the server hold all of it in
  memory for as long as it keeps sending
  ([h3#366](https://github.com/hyperium/h3/pull/366)). `Limits::head_timeout` ends this on request
  streams, but not on a connection's control stream. Nothing here bounds it
  ([#3](https://github.com/viell-dev/slop-warded-server/issues/3)).
- **Cancelled requests.** A handler is not cancelled when the client gives up on one request, only
  when the whole connection ends ([h3#352](https://github.com/hyperium/h3/issues/352)).
- **Empty DATA frames.** A request body that contains one fails
  ([h3#372](https://github.com/hyperium/h3/issues/372)).

The server works around two others. It limits decoded heads
([h3#379](https://github.com/hyperium/h3/issues/379)), and it contains a panic that a malformed body
causes ([h3#386](https://github.com/hyperium/h3/issues/386)). Containing it needs unwinding, so the
`http3` feature refuses to compile with `panic = "abort"`.

Set `WARDED_HTTP3=off` to serve without it, or leave out the `http3` feature. WebSockets,
WebTransport and other uses of CONNECT are not supported over HTTP/3, and 0-RTT requests are never
accepted.

## What the application provides

- Limits on request bodies, handler time and concurrent handlers. Axum's extractors limit bodies to
  2 MB by default; reading the body yourself does not.
- Authentication, CORS, security headers such as HSTS, and compression. The `api` and `website`
  templates add compression, logging and 500 responses for panics with `tower-http`.
- Host-based routing. HTTP/1.1 requests name the host in `Host`, and HTTP/2 and HTTP/3 requests in
  the URI; Warded's guide has
  [a host condition](https://viell-dev.github.io/slop-warded-router/custom-matchers.html) for all
  three.

## Not supported yet

- **Obtaining certificates.** Bring your own, for example from Certbot.
- **Running behind a proxy that terminates TLS.** The cleartext listener can serve the application,
  but requests then count as insecure, because headers from a proxy are not trusted.
- **More than one certificate or HTTPS address** in one server.
- **WebSockets over HTTP/2.** They work over HTTP/1.1.

## Connections

The default [`Limits`](../src/limits.rs) are not tuned for any particular machine. An HTTP/2
connection stays open as long as its client answers pings, so idle clients use up the connection
limit. When it is reached, new TCP connections are closed without a response, and new QUIC
connections are refused.

Shutting down waits up to two seconds for each HTTP/3 client to close its connection after its last
response, so that none of it is lost.

## Tested environments

Rust 1.99.1 on Linux and Windows in CI, with real HTTP/1.1, HTTP/2 and HTTP/3 connections over
loopback from Rust clients. Before a release also, on Linux, curl for each protocol and for the
upgrade through `Alt-Svc`, and Chromium over HTTP/3. Real networks, load and other HTTP/3 clients are
untested.
