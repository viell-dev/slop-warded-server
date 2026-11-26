---
description: "Get a certificate for development or the public, give it to the server, and renew it without a restart."
---

# Certificates

HTTPS needs a certificate, which proves to clients that the server owns its name, and a private key.
The server reads both from PEM files. For setting these variables in PowerShell, see
[Shell and curl](./getting-started.md#shell-and-curl).

```sh
WARDED_TLS_CERT=fullchain.pem WARDED_TLS_KEY=privkey.pem cargo run
```

The first file is the chain: the server's own certificate, then the intermediate ones of the
authority that issued it.

## For development

Without files, the server generates a certificate for `localhost` each time it starts. Clients do
not trust it: browsers show a warning, and curl needs `--insecure`.

[mkcert](https://github.com/FiloSottile/mkcert) makes one that this computer trusts:

```sh
mkcert -install
mkcert -cert-file cert.pem -key-file key.pem localhost 127.0.0.1 ::1
WARDED_TLS_CERT=cert.pem WARDED_TLS_KEY=key.pem cargo run
```

In PowerShell, set the two variables as in [Shell and curl](./getting-started.md#shell-and-curl).

Without mkcert, OpenSSL makes one that nothing trusts, but that stays the same when the server
starts again:

```sh
openssl req -x509 -newkey ec -pkeyopt ec_paramgen_curve:prime256v1 -nodes -days 90 \
    -keyout key.pem -out cert.pem -subj /CN=localhost \
    -addext "subjectAltName=DNS:localhost,IP:127.0.0.1,IP:::1"
```

Keep the key out of version control. Browsers then show no warning, but still use HTTP/2; see
[HTTP/3 in browsers](./http3.md#in-browsers).

## For the public

The server does not obtain certificates. [Certbot](https://certbot.eff.org) gets free ones from
Let's Encrypt. For a computer with the public name `example.com` and port 80 free:

```sh
sudo certbot certonly --standalone -d example.com
```

Give the server the files it reports:

```text
WARDED_TLS_CERT=/etc/letsencrypt/live/example.com/fullchain.pem
WARDED_TLS_KEY=/etc/letsencrypt/live/example.com/privkey.pem
```

Only `root` can read them at first. The [deployment](./deployment.md#systemd) page passes them to a
service that runs as another user.

`--standalone` needs port 80 for each renewal. When the server's own redirect uses that port, let
Certbot prove the name through your DNS provider instead, with one of its DNS plugins, or stop the
redirect around the renewal.

## Renewing

Certificates expire, so they are renewed every few weeks. New files take effect for new connections
when the process receives `SIGHUP`, on Unix, for example with `kill -HUP` and its process number.
With Certbot and [systemd](./deployment.md#systemd):

```sh
sudo certbot renew --deploy-hook "systemctl reload my-service"
```

If the new files cannot be used, the server logs why and keeps the previous certificate. Connections
that are open keep theirs.

`Server::serve` listens for the signal. With `serve_until`, [spawn
`reload_on_hangup`](./existing-axum.md#decide-when-to-stop), because `SIGHUP` otherwise ends the
process.

On Windows, or to decide the moment yourself, keep a clone of the `Tls` and reload it from code:

```sh
cargo add tracing
cargo add tokio --features time
```

```rust
use std::time::Duration;
use warded_server::{Error, Tls};

async fn reload_daily(tls: Tls) {
    let mut days = tokio::time::interval(Duration::from_secs(24 * 60 * 60));
    loop {
        days.tick().await;
        if let Err(error) = tls.reload() {
            tracing::error!(%error, "kept the previous certificate");
        }
    }
}

#[tokio::main]
async fn main() -> Result<(), Error> {
    let tls = Tls::self_signed(["localhost"])?;
    tokio::spawn(reload_daily(tls.clone()));
    // Then: Server::new(tls).serve(app).await
    Ok(())
}
```

Use `Tls::pem_files` in place of `Tls::self_signed`, which has no files to read again.

## Several names

One certificate can hold several names, such as `example.com` and `www.example.com`. The server
presents the same one to everybody, so put all names on it.
