# Changelog

Versions follow [Semantic Versioning](https://semver.org). Any prerelease may break compatibility.

## 0.1.0-alpha.2 - 2026-11-25

HTTP/3 no longer lets a client fill the memory, and fewer responses are lost. The server is still not
for the open internet: this is an alpha, and HTTP/3 rests on an experimental library with
[known weaknesses](https://viell-dev.github.io/slop-warded-server/limits.html#http-3).

- **Security:** what an HTTP/3 client can make the server hold in memory is bounded
  ([#3](https://github.com/viell-dev/slop-warded-server/issues/3)). Frames that declare too much are
  refused before they arrive, and a connection has at most 10 MB that handlers have not read yet.
  [Limits](https://viell-dev.github.io/slop-warded-server/limits.html#connections) adds it up.
- HTTP/3 clients such as Chromium and curl get the response to an upload that the handler does not
  read ([#15](https://github.com/viell-dev/slop-warded-server/issues/15)). The rest of the upload is
  discarded, until the client has been silent for five seconds or for 30 seconds, and a shutdown waits
  for that within its grace period, as it does for bodies that the application still has.
- Over HTTP/3, a client that need not send the rest of a body is told so with a code that says why.
- On Linux, UDP receive offload is turned off for HTTP/3, because of the memory that it can take. Fast
  uploads are somewhat slower.
- HTTP/3 request bodies may contain empty DATA frames.
- `Limits::head_bytes` can be at most 512 KiB.
- The [deployment guide](https://viell-dev.github.io/slop-warded-server/deployment.html) shows how an
  application can trust a proxy that ends TLS.
- A response over TCP is no longer cut off when the client sends something as the connection closes,
  which HTTP/2 clients do at a shutdown
  ([#10](https://github.com/viell-dev/slop-warded-server/issues/10)). The server waits for the client
  to close, so a connection can keep its place among `Limits::connections` for up to 30 seconds more,
  and a shutdown can take five seconds longer.
- Port 0 finds a port that is free for TCP and UDP also where Windows reserves many of one of them.
- **Templates:** pages of the `website` template answer HEAD with the head of the compressed response
  that GET gets ([#7](https://github.com/viell-dev/slop-warded-server/issues/7)). In a project made
  from it, replace `any(handler)` by `handler.into_service()`; see
  [middleware](https://viell-dev.github.io/slop-warded-server/middleware.html#the-response-to-head).

## 0.1.0-alpha.1 - 2026-11-25

The first version, for trying it out. It is not for the open internet: HTTP/3 has
[known weaknesses](https://viell-dev.github.io/slop-warded-server/limits.html#http-3).

- **A library** that serves Warded routes, an Axum application or another Tower service over HTTP/1.1,
  HTTP/2 and HTTP/3, with certificate reload, graceful shutdown, limits, and settings from the
  environment.
- **Templates** for `cargo-generate`: `minimal`, `api` and `website`.
- **A [documentation site](https://viell-dev.github.io/slop-warded-server/)** with guides for each kind
  of project.
