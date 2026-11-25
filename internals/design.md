# Design

Why the server is built this way. The [README](../README.md) and [limits](../docs/limits.md) own what
it does.

## A library and templates

The project was to make it easy to start a server without choosing and wiring libraries. The
candidates were guides, an installer script, a template repository with a branch for each kind of
project, and a library.

- **Guides** leave every project to repeat the wiring and to find its mistakes again.
- **A template alone** copies the wiring into each project. The part that matters most, HTTP/3, is
  also the part that needs fixes most often, and a copy does not get them.
- **Branches** for the kinds of project drift apart, and each needs its own checks.
- **An installer script** is a program to maintain for every shell and platform.

So the wiring is a library, which a dependency update fixes, and the templates hold only what a
project is meant to change: its routes, middleware and handlers. `cargo-generate` already asks
questions and fills in templates, like `npm create`, and takes them from directories of this
repository. One commit then changes and checks the library and the templates together.

The templates depend on the library by release tag, because it has no registry release. Their
manifests are named `Cargo.toml.liquid`, which `cargo-generate` renames: Cargo searches a Git
dependency for manifests, and warns in every project about ones that still hold placeholders.

## Which libraries

| Part             | Choice                 | Why                                                          |
| ---------------- | ---------------------- | ------------------------------------------------------------ |
| Applications     | Axum and Tower         | The widest choice of extractors, middleware and frameworks   |
| HTTP/1.1, HTTP/2 | `axum-server`, Hyper   | Lifecycle and TLS around the standard implementation         |
| TLS              | rustls with aws-lc-rs  | One implementation and certificate for TCP and QUIC          |
| QUIC             | Quinn                  | Works with rustls; the most used                             |
| HTTP/3           | h3                     | The only Rust implementation that fits the rest; see below   |

Salvo supports all three protocols but has its own request and handler types, so Warded and Tower
applications would need adapters. `tokio-quiche` has a more mature HTTP/3, but brings a second TLS
library built with C++ tools, its own certificate handling, and another interface to bridge. A proxy
such as Caddy hides the client's protocol from the application, and Warded routes on it.

Reconsider h3 if its [known defects](../docs/limits.md#http3) are not released by the beta stage. The
dependency is confined to `src/http3.rs` and `src/http3/`.

## The HTTP/3 bridge is our own

Published bridges from h3 to Axum were inspected and not used:

- `h3-axum` 0.3.1 ends the stream normally when a response body fails, so that the client takes a
  truncated response for a complete one. One malformed request ends its whole connection, and its
  connection driver cannot be drained.
- `axum-h3` and `h3-util` 0.0.6 leave h3's decoded head size unlimited, cancel requests whose body
  the handler does not read, and abandon connections at shutdown.

Both are small; so is the replacement in `src/http3/body.rs` and `src/http3/head.rs`. It also does what
Hyper does for HTTP/2 and h3 leaves to its caller: checking request heads and `Content-Length`, and
cleaning response heads.

## Choices that are easy to undo by accident

- **The application is wrapped in an Axum router** before anything else, because that router supplies
  HEAD and `Content-Length` handling, which Hyper does for HTTP/1.1 and HTTP/2 only in part and h3
  not at all.
- **Closing HTTP/3 lingers while data flows.** Closing a QUIC connection discards what Quinn still has
  to send or repeat, and h3 does not report when a response has been received. A fixed delay loses
  responses on slow links.
- **Connection slots are held by the stream**, not the service, so that upgraded connections count.
- **Response streams reset unless finished.** Quinn finishes a stream that is dropped.
- **No `Forwarded` or `X-Forwarded-*` field is read.** A client can send them.
