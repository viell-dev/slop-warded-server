# {{project-name}}

A minimal server with one route. It serves HTTP/1.1, HTTP/2 and HTTP/3 with
[Warded Server](https://github.com/viell-dev/slop-warded-server).

## Start

See [requirements and shells][requirements], then run this from the project directory.

```sh
cargo run
```

The first build can take a few minutes. The startup `WARN` about a certificate that clients do
not trust is expected. The development address uses plain HTTP.

Open the address printed in the log, `http://127.0.0.1:3000/`. The project's
`.cargo/config.toml` sets `WARDED_DEV_ADDR` for this address.
You should see `Hello`. Stop with Ctrl+C. After an edit, run
`cargo run` again and refresh.

Follow the [template guide](https://viell-dev.github.io/slop-warded-server/template-minimal.html)
for the first changes, tests and deployment.

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

See [Configuration](https://viell-dev.github.io/slop-warded-server/configuration.html) for
details about these settings.

[requirements]: https://viell-dev.github.io/slop-warded-server/getting-started.html#what-you-need
