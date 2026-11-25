---
description: "How clients find HTTP/3, what it needs from the network, how to check it with curl and a browser, and how to turn it off."
---

# HTTP/3

HTTP/3 runs over QUIC, on UDP, instead of over TCP. It connects with fewer round trips, a lost
packet delays only the request it belongs to, and connections survive a change of network. Your
handlers work the same way on it.

::: warning
The library beneath it is experimental and has [known weaknesses](./limits.md#http-3).
:::

## How clients get there

A browser does not know in advance that a server has HTTP/3. It connects with HTTP/2, and the
responses over HTTPS announce HTTP/3 in a field:

```http
Alt-Svc: h3=":8443"; ma=86400
```

The browser tries that for later connections, for a day after the last announcement, and stays on
HTTP/2 when it does not work. So HTTP/1.1 and HTTP/2 are always needed as well.

## What it needs

- **UDP on the HTTPS port.** Open it in firewalls, and forward it wherever TCP is forwarded.
- **The same port from outside.** If clients use another port than the one the server listens on,
  set `WARDED_PUBLIC_PORT` to theirs.
- **A certificate that clients accept for it**; see below.

## Check that it works

With a curl that has HTTP/3, which `curl --version` lists among its features:

```sh
curl --insecure --http3-only --include https://localhost:8443/
```

To follow the announcement as a browser does, keep what curl learns in a file and ask twice. This
prints `2` and then `3`:

```sh
for attempt in first second; do
    curl --insecure --alt-svc alt-svc.txt --silent --output /dev/null \
        --write-out '%{http_version}\n' https://localhost:8443/
done
```

### In browsers

In the developer tools, the network view has a protocol column, with `h3` for HTTP/3.

Browsers are strict about certificates for HTTP/3. Chromium, for one, accepts only those of public
authorities, so a certificate that you made your computer trust leaves it on HTTP/2. To test
locally, start it with a profile of its own and permission for your certificate. With `cert.pem`
from [mkcert](./certificates.md#for-development):

```sh
key=$(openssl x509 -in cert.pem -pubkey -noout | openssl pkey -pubin -outform der |
    openssl dgst -sha256 -binary | base64)
chromium --user-data-dir="$(mktemp -d)" --origin-to-force-quic-on=localhost:8443 \
    --ignore-certificate-errors-spki-list="$key" https://localhost:8443/
```

## Route on it

The protocol is a condition like any other:

```rust
use axum::{
    body::Body,
    http::{Request, Version},
    routing::any,
};
use http_body_util::BodyExt;
use tower::ServiceExt;
use warded_server::warded::{
    Matcher, Router,
    matchers::{exact_path, version},
};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let routes = Router::new()
        .route(
            exact_path("/").and(version(Version::HTTP_3)),
            any(|| async { "over HTTP/3" }),
        )
        .route(exact_path("/"), any(|| async { "over something else" }));
    let request = Request::builder()
        .uri("/")
        .version(Version::HTTP_3)
        .body(Body::empty())?;
    let response = warded_server::app(routes).oneshot(request).await?;
    assert_eq!(response.into_body().collect().await?.to_bytes(), "over HTTP/3");
    Ok(())
}
```

The same page should normally be available on every protocol, because clients choose and change it
on their own.

## Turn it off

`WARDED_HTTP3=off`, or `Server::http3(false)`, stops listening on UDP and announcing it. Browsers
that heard of it before fall back to HTTP/2. To leave its code out, turn off the `http3` feature:

```toml
[dependencies.warded-server]
git = "https://github.com/viell-dev/slop-warded-server"
tag = "0.1.0-alpha.1"
default-features = false
features = ["self-signed", "warded"]
```

With the feature on, a program cannot be built with `panic = "abort"`; [limits](./limits.md#http-3)
explains why.
