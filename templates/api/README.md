# {{project-name}}

A JSON API. It serves HTTP/1.1, HTTP/2 and HTTP/3 with
[Warded Server](https://github.com/viell-dev/slop-warded-server).

```sh
cargo run
curl --insecure --json '{"text": "milk"}' https://localhost:8443/notes
curl --insecure https://localhost:8443/notes
```

## Layout

- `src/main.rs`: starts the server.
- `src/routes.rs`: the routes and the middleware, with their tests below.
- `src/notes.rs`: an example resource, kept in memory. Replace it with yours.
- `src/problem.rs`: errors as JSON.

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
| `RUST_LOG`           | What to log                                | see `main.rs`    |

The generated certificate is for development. No client trusts it, and it is only used while the
server listens on this computer alone. Browsers use HTTP/3 only with a certificate they trust.
