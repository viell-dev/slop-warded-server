# {{project-name}}

A minimal server with one route. It serves HTTP/1.1, HTTP/2 and HTTP/3 with
[Warded Server](https://github.com/viell-dev/slop-warded-server).

## Start

```sh
cargo run
```

When the log says `open http://127.0.0.1:3000/ in a browser`, open that address. It is plain HTTP
for this computer alone, so there is no certificate to accept. HTTPS, with HTTP/2 and HTTP/3, runs
at the same time on `https://localhost:8443/`, with a certificate that no client trusts:

```sh
curl http://127.0.0.1:3000/
curl --insecure --http2 https://localhost:8443/
```

Stop it with Ctrl+C. To restart the server after each change, install
[bacon](https://dystroy.org/bacon/) once, and run it in place of `cargo run`. Quit it with `q`.

```sh
cargo install --locked bacon
bacon run-long
```

The page is text, and not HTML, so reload it in the browser yourself.

## Layout

- `src/main.rs`: the routes and the server.
- `.cargo/config.toml`: the development address for `cargo run`.

## Configuration

Set these environment variables to run it elsewhere:

| Variable             | Meaning                                    | Default                           |
| -------------------- | ------------------------------------------ | --------------------------------- |
| `WARDED_HTTPS_ADDR`  | HTTPS address, over TCP and UDP            | `127.0.0.1:8443`                  |
| `WARDED_HTTP_ADDR`   | Cleartext address that redirects to HTTPS  | none                              |
| `WARDED_DEV_ADDR`    | Plain HTTP address for this computer       | `127.0.0.1:3000` with `cargo run` |
| `WARDED_TLS_CERT`    | PEM certificate chain file                 | generated                         |
| `WARDED_TLS_KEY`     | PEM private key file                       | generated                         |
| `WARDED_HTTP3`       | `on` or `off`                              | `on`                              |
| `WARDED_PUBLIC_PORT` | HTTPS port that clients use, if it differs | the bound port                    |
| `RUST_LOG`           | What to log                                | `info`                            |

`.cargo/config.toml` sets `WARDED_DEV_ADDR` for the programs that Cargo starts, and not for one
that is deployed. A variable that you set yourself takes precedence; set it empty to turn the
address off. It has to be a loopback address, since nothing on it is encrypted.

The generated certificate is for development. No client trusts it, and it is only used while the
server listens on this computer alone. Browsers use HTTP/3 only with a certificate they trust.
