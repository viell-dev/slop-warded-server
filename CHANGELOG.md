# Changelog

Versions follow [Semantic Versioning](https://semver.org). Any prerelease may break compatibility.

## Unreleased

New projects open in a browser at once, and their code holds no wiring
([#18](https://github.com/viell-dev/slop-warded-server/issues/18)).

- **Guides for each template**, from the first start to the first changes, a glossary, and a
  comparison of where to host a Vite build.
- **A development address:** `Server::development` or `WARDED_DEV_ADDR` also serves the application
  over plain HTTP on a loopback address, for a browser on this computer without a certificate.
  Other addresses are refused, because nothing on it is encrypted. Browsers use HTTP/1.1 there, and
  Warded's `secured()` does not hold. Its HTML pages reload by themselves when the server restarts,
  unless `live_reload(false)` turns that off, and its responses are not compressed. See
  [configuration](https://viell-dev.github.io/slop-warded-server/configuration.html#the-development-address).
- **The development address answers only this computer's browser:** a 403 for requests for another
  host than an IP address, `localhost` or a name below `.localhost` (against DNS rebinding), for
  those that the browser marks as made by another site, except to open a page, and for changes
  and WebSockets from an origin that is not on this computer.
- The log says what to open, such as `open http://127.0.0.1:3000/ in a browser`. The `serving` line
  has a `development` field.
- **Helpers**, over `tracing-subscriber`, `tower-http` and Axum, behind new default features:
  `init_logging` (`logging`), `DefaultMiddleware` with tracing, compression and a 500 for panics
  (`middleware`), and `StaticFiles` and `SinglePageApp` (`files`). The library brings 28 more
  crates with all of them.
- `Error::Development` reports a development address that is not loopback.
- **Templates:** each sets `WARDED_DEV_ADDR=127.0.0.1:3000` in `.cargo/config.toml`, for
  `cargo run`, and uses the helpers in place of its own wiring. Their READMEs show how
  [bacon](https://dystroy.org/bacon/) restarts the server after each change.
  - `api` answers `GET /` with a list of its endpoints.
  - `website` is a small site: a menu, three pages, a contact form that the server checks and that
    redirects after success, pages for a 404 and a 500, an icon, and a `bacon.toml`. The page in two
    languages moved to the [website guide](https://viell-dev.github.io/slop-warded-server/website.html#languages).
  - Projects from earlier templates keep working. To take the new start into one, add the
    `.cargo/config.toml` of its template.

Requests that the HTTP specifications call malformed no longer reach the application, so that it
and its routes see the same request on every protocol. They get a 400, which says why, and over
HTTP/3 a stream error with `H3_MESSAGE_ERROR`.

- **Whitespace around a field value**, such as `x-tenant: acme<TAB>`, is refused over HTTP/2 and
  HTTP/3. HTTP/1.1 removes it, as before
  ([#25](https://github.com/viell-dev/slop-warded-server/issues/25)).

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
