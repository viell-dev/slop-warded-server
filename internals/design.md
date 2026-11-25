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

Reconsider h3 if it gets a defect that the server can neither work around nor live with; its
[known ones](../docs/limits.md#http-3) are tracked in
[#2](https://github.com/viell-dev/slop-warded-server/issues/2). The dependency is confined to
`src/http3.rs` and `src/http3/`.

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

## The transport of h3 is our own

h3 reads from QUIC through traits, which `h3-quinn` implements for Quinn. `src/http3/transport.rs`
does it instead, because nothing between the two can bound what is read:

- `src/http3/frames.rs` finds where frames begin and end. Only there can h3 be kept from reading
  further than it decodes, and from holding a frame of any length.
- Each read takes a few kilobytes. `h3-quinn` takes whatever has arrived, up to a megabyte when it
  came in small packets, and Quinn then lets the client send as much again.

It is not an HTTP/3 implementation: h3 still decides what every frame means and where it may come.
The alternative was to wait for an h3 release, which had not come in the 18 months since 0.0.8, and
would not change the second point. It also avoids a panic of `h3-quinn` when a stream is stopped
([h3#361](https://github.com/hyperium/h3/issues/361)).

## Choices that are easy to undo by accident

- **Responses are finished for each connection**, in `src/context.rs`, outside all layers: the content
  for HEAD, and `Content-Length`. Axum does it for its own handlers only, Hyper for HTTP/1.1 and
  HTTP/2 only in part, and h3 not at all.
- **Closing HTTP/3 lingers while data flows.** Closing a QUIC connection discards what Quinn still has
  to send or repeat, and h3 does not report when a response has been received. A fixed delay loses
  responses on slow links.
- **Closing TCP waits for the client.** Closing a socket while the client still sends resets the
  connection, and the client can lose what it has not read. Neither Hyper nor `axum-server` waits.
- **Connection slots are held by the stream**, not the service, so that upgraded connections count.
- **Response streams reset unless finished.** Quinn finishes a stream that is dropped.
- **Unread HTTP/3 bodies are discarded for a while.** The client could be told to stop at once, but
  Chromium then never reports the response, and curl reports an error instead of it. What the stream
  brings is discarded, and not what h3 makes of it, which knows neither when the client last sent
  something nor, with trailers, when it is done. It is taken in order, because Quinn otherwise
  remembers every range that it has received.
- **What is kept of a request is copied.** The pieces that Quinn hands out keep the packets in
  memory that they came in, so a limit on the length of what is kept is not one on its memory.
- **Packets are received apart on Linux.** With receive offload, Quinn keeps whole batches of packets
  in memory for the sake of one, and does not count them.
- **Nothing is read ahead.** What has been read no longer counts against QUIC's windows, so the
  client may send as much again, whether or not a handler has seen it.
- **No `Forwarded` or `X-Forwarded-*` field is read.** A client can send them. Trusting a proxy
  depends on its address and on what it does with each field, so it is a [recipe for the
  application](../docs/deployment.md#a-proxy-that-ends-tls) and not a setting: a setting would have to
  cover every proxy's habits, and one that is nearly right is a security defect. Reconsider it when
  several applications need the same one.
