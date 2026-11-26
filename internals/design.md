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
| Live reload      | `tower-livereload`     | A small Tower layer, maintained, with no new dependencies    |
| Helpers          | `tower-http`, `tracing-subscriber` | What Axum projects use already                   |

Salvo supports all three protocols but has its own request and handler types, so Warded and Tower
applications would need adapters. `tokio-quiche` has a more mature HTTP/3, but brings a second TLS
library built with C++ tools, its own certificate handling, and another interface to bridge. A proxy
such as Caddy hides the client's protocol from the application, and Warded routes on it.

Reconsider h3 if it gets a defect that the server can neither work around nor live with; its
[known ones](../docs/limits.md#http-3) are tracked in
[#2](https://github.com/viell-dev/slop-warded-server/issues/2). The dependency is confined to
`src/http3.rs` and `src/http3/`.

## Starting in the browser

A new project should behave like one from `npm create vue`: open in a browser at once, reload after
an edit, and hold code that is the user's own.

### The development address

The certificate that the server generates makes browsers warn, and breaks the live reload of tools
such as cargo-leptos, whose script then uses `wss:`. A browser on the same computer needs no
certificate on loopback: `http://127.0.0.1` and `http://localhost` are secure contexts. So the
development address serves the application over cleartext, on loopback only. Anything else is
refused rather than warned about, because a cleartext listener on a network address is how a
development setting becomes a deployed one.

- **Off unless asked for.** The library cannot tell development from deployment: a release build
  runs on developers' computers too, and a debug build can be deployed. So the templates ask, with
  `WARDED_DEV_ADDR` in `.cargo/config.toml`. Cargo gives it to the programs that it starts, and not
  to a program started on its own; a variable that is set already wins, also an empty one. The
  alternative, `if cfg!(debug_assertions)` in every `main`, repeats wiring in each project and
  guesses from the profile. `cargo run --release` on a server still gets the address, which the
  documentation warns about.
- **A listener of its own**, beside `redirect_http` and `serve_http`, which are for clients on the
  network or a proxy and keep their meaning.
- **The IP address in the log**, rather than `localhost`, which some systems resolve to IPv6 first,
  where an IPv4 listener refuses. Binding both families instead needs a second socket, which fails
  where IPv6 is off, as in some containers.
- **Loopback is not a boundary in the browser.** Any page can make the browser send requests to
  `127.0.0.1`, and a DNS name whose answer changes to `127.0.0.1` makes them same-origin for that
  page (DNS rebinding). So the address refuses, before the application and the reload stream,
  requests for other hosts than IP addresses, `localhost` and names below `.localhost`, as Vite's
  `server.allowedHosts` does; rebinding needs a name that the attacker's DNS answers. It also
  follows Fetch Metadata's resource isolation policy, and, for browsers without it, refuses changes
  and WebSockets whose `Origin` is not on this computer: a loopback address, `localhost` or a name
  below it. Any IP address does for the host, which the browser connected to, but not for the
  origin, since pages come from servers' addresses too. Names from the hosts file are refused with
  the rest: a setting for them waits until someone needs one.

### Live reload

`tower-livereload` adds a script to HTML pages, which keeps an event stream open, and reloads the
page when the stream comes back after it was gone. A restart is what brings it back; the server
does not watch files. It is applied by the library on the development address alone, so that HTTPS
serves what a deployment serves.

- **Only whole pages.** Not a range (`Range`, 206), whose length and offsets the script would
  break, and not responses without content.
- **No compression there.** The script can only be added to an uncompressed page, and the
  application can be any service, whose compression the library cannot reach into. So requests on
  the development address ask for `identity`. Compression gains nothing on loopback.
- **Only for pages that a browser opens** (`Sec-Fetch-Dest: document`, or none), so that HTML
  fragments that scripts fetch stay as they are.
- **The stream ends at shutdown.** Otherwise it is a request in progress, and Ctrl+C, or a watcher
  that sends `SIGTERM`, waits for the whole grace period.
- **`live_reload(false)`** turns it off for tools that bring their own, such as Vite or
  cargo-leptos.

Rebuilding and restarting is a watcher's job. [bacon](https://dystroy.org/bacon/) is the one the
templates document. Its `run-long` job does exactly this, it installs with `cargo install` on Linux,
macOS and Windows, and a `bacon.toml` in the project names the directories to watch beyond `src/`.
watchexec 2.8 does the same, and stops the program with `SIGTERM` rather than killing it, but
reads no project file that says what to watch and run, so every user would type it.

### Thin helpers

`init_logging`, `DefaultMiddleware`, `StaticFiles` and `SinglePageApp` replace wiring that the
templates and guides repeated. Each is a few lines over `tracing-subscriber`, `tower-http`, Axum and
Warded, with that library's behavior, and the layers stay usable on their own. They add no routing
of their own: routes stay Warded's, and `into_service` and `any` stay, because hiding them would
need another way to declare routes. They are default features, which a project that wires its own
can turn off.

- **`DefaultMiddleware` returns a boxed service**, so that `tower-http`'s types stay out of this
  crate's API. Its tracing logs at `info`, which `init_logging` shows by default. Panics are logged
  once, whatever the response.
- **`init_logging` uses colours only in a terminal**, so that a journal or a file gets plain text.
- **`StaticFiles` takes the prefix again**, which the route's pattern already has. Warded's captures
  are kept by name, not by position, so the destination cannot find the rest of the path without a
  name to agree on.
- **`SinglePageApp` serves files at the top of the build before `index.html`**, so that a browser
  that opens `/favicon.svg` gets it. Files in `assets/` are kept for a year, unless they are
  missing, which a later build may change, or HTML, which keeps its name and gets `no-cache`. HTML
  is told by the name that the file system uses, which a 304 has too: decoded as `ServeDir` decodes
  it, and without what Windows ignores, which is case, trailing dots and spaces, and a stream after
  `:`. The content type of a 200 counts too.

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
