# Changelog

Versions follow [Semantic Versioning](https://semver.org). Any prerelease may break compatibility.

## Unreleased

- The [deployment guide](https://viell-dev.github.io/slop-warded-server/deployment.html) shows how an
  application can trust a proxy that ends TLS.
- A response over TCP is no longer cut off when the client sends something as the connection closes,
  which HTTP/2 clients do at a shutdown
  ([#10](https://github.com/viell-dev/slop-warded-server/issues/10)).
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
