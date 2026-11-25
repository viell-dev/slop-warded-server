# Configuration

`Server::from_env` reads environment variables. `Server::new` and its methods set the same things in
code, and more: [`Limits`](../src/limits.rs), the grace period, and a cleartext listener that serves
the application.

| Variable             | Meaning                                    | Default          |
| -------------------- | ------------------------------------------ | ---------------- |
| `WARDED_HTTPS_ADDR`  | HTTPS address, over TCP and UDP            | `127.0.0.1:8443` |
| `WARDED_HTTP_ADDR`   | Cleartext address that redirects to HTTPS  | none             |
| `WARDED_TLS_CERT`    | PEM certificate chain file                 | see below        |
| `WARDED_TLS_KEY`     | PEM private key file                       | see below        |
| `WARDED_HTTP3`       | `on` or `off`                              | `on`             |
| `WARDED_PUBLIC_PORT` | HTTPS port that clients use, if it differs | the bound port   |

An address is an IP address and a port. `[::]:443` accepts IPv4 as well as IPv6.

## Certificates

Set `WARDED_TLS_CERT` to the chain, with the server's certificate first, and `WARDED_TLS_KEY` to its
key. Certbot calls them `fullchain.pem` and `privkey.pem`. The server does not obtain certificates.

After renewing them, send the process `SIGHUP`, or call `Tls::reload` from code. New connections get
the new certificate and established ones keep theirs. If the files cannot be used, the server logs
why and keeps the previous certificate.

Without either variable the server generates a certificate for `localhost` at each start. It does so
only while HTTPS listens on a loopback address, so a missing setting cannot put an untrusted
certificate on a network.

## HTTP/3

HTTP/3 uses UDP on the same port as HTTPS. Open that port for UDP in firewalls and forward it along
with TCP. Clients that cannot reach it keep using HTTP/2.

When forwarding changes the port, set `WARDED_PUBLIC_PORT` to the one that clients use, for UDP and
TCP alike. The advertisement and the redirects name it.

Check it with a curl that supports HTTP/3:

```sh
curl --insecure --http3-only --include https://localhost:8443/
```

Browsers first connect with HTTP/2, learn about HTTP/3 from the response, and use it for later
connections. They are stricter about certificates there. Chromium accepts only certificates from
public authorities for HTTP/3, so a certificate that you made your computer trust still leaves it on
HTTP/2. To test HTTP/3 locally, start it with a profile of its own and these flags:

```sh
key=$(openssl x509 -in cert.pem -pubkey -noout | openssl pkey -pubin -outform der |
    openssl dgst -sha256 -binary | base64)
chromium --user-data-dir="$(mktemp -d)" --origin-to-force-quic-on=localhost:8443 \
    --ignore-certificate-errors-spki-list="$key" https://localhost:8443/
```

## Stopping

Ctrl+C or `SIGTERM` stops the server from accepting connections. Requests in progress have 30 seconds
to finish before their connections are closed. Idle connections are told to close at once.

Set another period with `Server::grace_period`, and make a service manager wait longer than that
before it kills the process.

## Logging

The server reports through [`tracing`](https://docs.rs/tracing) and logs nothing by itself. The
templates install `tracing-subscriber`, which `RUST_LOG` configures. Failed connections and malformed
requests are reported at the `debug` level, because any client can cause them.
