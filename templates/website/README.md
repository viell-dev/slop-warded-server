# {{project-name}}

A website with server-rendered pages and static files. It serves HTTP/1.1, HTTP/2 and HTTP/3 with
[Warded Server](https://github.com/viell-dev/slop-warded-server).

```sh
cargo run
curl --insecure https://localhost:8443/
```

## Layout

- `src/main.rs`: the routes, the middleware and the server.
- `src/pages.rs`: the pages, from the [Askama](https://askama.rs) templates in `templates/`.
- `static/`: files served below `/static/`. Start the server where this directory is.

## Configuration

Set these environment variables to run it elsewhere:

| Variable             | Meaning                                    | Default          |
| -------------------- | ------------------------------------------ | ---------------- |
| `WARDED_HTTPS_ADDR`  | HTTPS address, over TCP and UDP            | `127.0.0.1:8443` |
| `WARDED_HTTP_ADDR`   | Cleartext address that redirects to HTTPS  | none             |
| `WARDED_TLS_CERT`    | PEM certificate chain file                 | generated        |
| `WARDED_TLS_KEY`     | PEM private key file                       | generated        |
| `WARDED_HTTP3`       | `on` or `off`                              | `on`             |
| `WARDED_PUBLIC_PORT` | HTTPS port that clients use, if it differs | the bound port   |
| `RUST_LOG`           | What to log                                | `info`           |

The generated certificate is for development. No client trusts it, and it is only used while the
server listens on this computer alone. Browsers use HTTP/3 only with a certificate they trust.
