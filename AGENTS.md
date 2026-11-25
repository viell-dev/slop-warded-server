# Agent instructions

Read [README.md](./README.md), [CONTRIBUTING.md](./CONTRIBUTING.md), [limits](./docs/limits.md) and the
[design notes](./internals/design.md) before changing behavior. This repository is maintained by
agents; useful verification and honest compatibility claims are essential.

## Scope

- Assemble existing server, TLS, QUIC and HTTP libraries into one easy starting point. Do not write
  a protocol implementation, an async runtime or another web framework.
- Keep the server usable with ordinary Tower and Axum applications as well as Warded routes. Warded
  stays usable without this server.
- A synthetic request with an HTTP version set is a router unit test, not transport evidence. Claims
  about HTTP/1.1, HTTP/2 or HTTP/3 need a real connection of that kind.
- Defects found in Warded go to [its issues](https://github.com/viell-dev/slop-warded-router/issues),
  linked from here.

## Changes

- Use a task branch from `main` and deliver by pull request. Prefer rebase integration without
  rewriting shared history.
- Follow Rust ecosystem conventions, the package lint configuration and `cargo fmt`. Prefer narrowly
  scoped `#[expect(..., reason = "...")]` over blanket lint suppression.
- Keep human-facing Markdown prose near 100 columns.
- Use `.github/labels.json` as the label catalog. Every issue and pull request needs one `Type:`
  label and at most one `Status:` label. `Needs:` identifies the person who must act.
- A test of a defence should fail when the defence is removed. Check that once, when adding it.

## Documentation

- `README.md` and `docs/` are for users, rustdoc is the API reference, and `internals/` is for
  maintainers. Put each fact in one of them.
- The environment variables are listed in rustdoc, `docs/configuration.md` and each template's
  README; change them together.
- Keep known weaknesses in `docs/limits.md` current, and link the upstream reports.
- Record user-facing changes in the changelog.
