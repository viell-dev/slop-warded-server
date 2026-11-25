# Changelog

Versions follow [Semantic Versioning](https://semver.org). Any prerelease may break compatibility.

## Unreleased

- The [deployment guide](https://viell-dev.github.io/slop-warded-server/deployment.html) shows how an
  application can trust a proxy that ends TLS.

## 0.1.0-alpha.1 - 2026-11-25

The first version, for trying it out. It is not for the open internet: HTTP/3 has
[known weaknesses](https://viell-dev.github.io/slop-warded-server/limits.html#http-3).

- **A library** that serves Warded routes, an Axum application or another Tower service over HTTP/1.1,
  HTTP/2 and HTTP/3, with certificate reload, graceful shutdown, limits, and settings from the
  environment.
- **Templates** for `cargo-generate`: `minimal`, `api` and `website`.
- **A [documentation site](https://viell-dev.github.io/slop-warded-server/)** with guides for each kind
  of project.
