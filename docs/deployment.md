---
description: "Run the server for others: build it, then run it under systemd or in a container, with certificates, ports and firewalls."
---

# Deployment

A project builds to one program. To run it for others, it needs a [certificate](./certificates.md),
its [addresses](./configuration.md#addresses), and open ports. Read [limits](./limits.md) first:
this is an alpha.

## Build it

```sh
cargo build --release
```

The program is in `target/release/`, with your project's name. It needs the same kind of operating
system that built it, and on Linux the same or a newer C library. Projects from the `website`
template also need their `static/` directory beside where they are started; pages are part of the
program.

## Open the ports

| Port | Protocol | For                                |
| ---- | -------- | ---------------------------------- |
| 443  | TCP      | HTTP/1.1 and HTTP/2                |
| 443  | UDP      | HTTP/3                             |
| 80   | TCP      | Redirects from HTTP, if you want them |

Firewalls and routers often open TCP alone. The site then works, but without HTTP/3.

## systemd

This runs the program as a user of its own, which may use ports below 1024 and little else. Create
the user, and put the program and its files in place:

```sh
sudo useradd --system --home-dir /opt/my-site --shell /usr/sbin/nologin my-site
sudo install -D target/release/my-site /opt/my-site/my-site
sudo cp -r static /opt/my-site/
sudo install -d -m 750 -g my-site /etc/my-site
```

Save this as `/etc/systemd/system/my-site.service`:

```ini
[Unit]
Description=My site
Wants=network-online.target
After=network-online.target

[Service]
User=my-site
WorkingDirectory=/opt/my-site
ExecStart=/opt/my-site/my-site
ExecReload=kill -HUP $MAINPID
Environment=WARDED_HTTPS_ADDR=[::]:443
Environment=WARDED_HTTP_ADDR=[::]:80
Environment=WARDED_TLS_CERT=/etc/my-site/fullchain.pem
Environment=WARDED_TLS_KEY=/etc/my-site/privkey.pem
AmbientCapabilities=CAP_NET_BIND_SERVICE
CapabilityBoundingSet=CAP_NET_BIND_SERVICE
NoNewPrivileges=yes
ProtectSystem=strict
ProtectHome=yes
PrivateTmp=yes
LimitNOFILE=20000
TimeoutStopSec=40
Restart=on-failure

[Install]
WantedBy=multi-user.target
```

`TimeoutStopSec` is longer than the server's [grace period](./configuration.md#stopping), so that
requests can finish. `LimitNOFILE` is above its connection limit. An application that writes files
needs a place for them, such as `StateDirectory=my-site` for `/var/lib/my-site`.

Certbot's files belong to `root`. Let it copy each new certificate to where the program may read it,
with this as the executable file `/etc/letsencrypt/renewal-hooks/deploy/my-site`. Certbot runs it
for every certificate that it renews, so it must name the one it is for:

```sh
#!/bin/sh
set -e
if [ "$RENEWED_LINEAGE" != /etc/letsencrypt/live/example.com ]; then
    exit 0
fi
install -m 640 -g my-site "$RENEWED_LINEAGE/fullchain.pem" "$RENEWED_LINEAGE/privkey.pem" /etc/my-site/
if systemctl is-active --quiet my-site; then
    systemctl reload my-site
fi
```

Run it once by hand, then start the service:

```sh
sudo env RENEWED_LINEAGE=/etc/letsencrypt/live/example.com /etc/letsencrypt/renewal-hooks/deploy/my-site
sudo systemctl daemon-reload
sudo systemctl enable --now my-site
journalctl --unit my-site --follow
```

To update, replace the program and run `sudo systemctl restart my-site`. Clients that connect in
between are refused for a moment.

## Containers

For Docker and the like, save this as `Dockerfile` in the project, with its name in place of
`my-site`:

```dockerfile
FROM rust:1.99.1-slim AS build
WORKDIR /build
COPY . .
RUN cargo build --release --locked

FROM debian:trixie-slim
COPY --from=build /build/target/release/my-site /usr/local/bin/my-site
# Only projects with static files need this line.
COPY static /srv/static
WORKDIR /srv
USER nobody
ENV WARDED_HTTPS_ADDR=0.0.0.0:8443 WARDED_PUBLIC_PORT=443
EXPOSE 8443/tcp 8443/udp
CMD ["my-site"]
```

Leave the build directory out of it with a `.dockerignore` file:

```text
target
.git
```

Build and run it, with the certificates from a directory that the container's user can read. The
port is published twice, for TCP and UDP:

```sh
docker build --tag my-site .
docker run --detach --name my-site --restart unless-stopped --stop-timeout 40 \
    --publish 443:8443/tcp --publish 443:8443/udp \
    --volume /srv/my-site/certificates:/certificates:ro \
    --env WARDED_TLS_CERT=/certificates/fullchain.pem \
    --env WARDED_TLS_KEY=/certificates/privkey.pem \
    my-site
```

`WARDED_PUBLIC_PORT` makes the HTTP/3 announcement name the published port. `docker kill
--signal HUP my-site` reloads the certificate, and `docker stop` shuts down gracefully.

Depending on how a container network forwards packets, the address that handlers see can be the
network's own instead of the client's.

## Behind something else

- **A load balancer that passes TCP and UDP through** works, with the certificate on this server.
  Connections must keep reaching the same server, also over UDP.
- **A proxy or CDN that ends TLS** speaks HTTP/1.1 or HTTP/2 to this server. Visitors then use the
  protocols and the certificate of the proxy, and your routes see the proxy's connection instead of
  theirs.

### A proxy that ends TLS

The proxy can connect to the HTTPS address, or to a cleartext one that you add with
[`serve_http`](./configuration.md#cleartext-http). What it connected with says nothing about the
visitor, whose address and security only the proxy knows. It can report them in fields, usually
`X-Forwarded-For` and `X-Forwarded-Proto`. The server reads neither, because any client can send
them too. Your application can, for connections that come from the proxy:

```rust
use axum::{
    body::Body,
    extract::{ConnectInfo, Request},
    http::StatusCode,
    middleware::map_request,
    routing::any,
};
use http_body_util::BodyExt;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use tower::ServiceExt;
use warded_server::warded::{
    Matcher, Router, TransportSecurity,
    matchers::{exact_path, secured},
};

/// The address that the proxy connects from.
const PROXY: IpAddr = IpAddr::V4(Ipv4Addr::new(10, 0, 0, 2));

/// Replace what is known about the proxy's connection by what the proxy says about its client's.
async fn trust_proxy(mut request: Request) -> Result<Request, StatusCode> {
    let peer = request.extensions().get::<ConnectInfo<SocketAddr>>();
    if peer.is_none_or(|peer| peer.0.ip() != PROXY) {
        return Ok(request);
    }
    // More than one value means that the proxy passes on what its client sent.
    let mut protocols = request.headers().get_all("x-forwarded-proto").iter();
    let security = match (protocols.next(), protocols.next()) {
        (Some(protocol), None) if protocol == "https" => TransportSecurity::Https,
        (Some(protocol), None) if protocol == "http" => TransportSecurity::Unsecure,
        _ => TransportSecurity::Unknown,
    };
    // The proxy adds its client at the end, after what that client may have sent.
    let client: IpAddr = request
        .headers()
        .get_all("x-forwarded-for")
        .iter()
        .next_back()
        .and_then(|field| field.to_str().ok()?.rsplit(',').next()?.trim().parse().ok())
        .ok_or(StatusCode::BAD_REQUEST)?;
    let extensions = request.extensions_mut();
    extensions.insert(security);
    // The client's port is not known.
    extensions.insert(ConnectInfo(SocketAddr::new(client.to_canonical(), 0)));
    Ok(request)
}

async fn greet(ConnectInfo(client): ConnectInfo<SocketAddr>) -> String {
    format!("Hello {}", client.ip())
}

fn app() -> axum::Router {
    let routes = Router::new().route(exact_path("/").and(secured()), any(greet));
    warded_server::app(routes).layer(map_request(trust_proxy))
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Both claim that 192.0.2.1 connected with HTTPS, and only the first is the proxy.
    for (peer, status, text) in [
        ([10, 0, 0, 2], 200, "Hello 192.0.2.1"),
        ([203, 0, 113, 9], 404, ""),
    ] {
        let mut request = Request::builder()
            .uri("/")
            .header("x-forwarded-for", "198.51.100.7, 192.0.2.1")
            .header("x-forwarded-proto", "https")
            .body(Body::empty())?;
        // What the server adds to a request on the cleartext address.
        let extensions = request.extensions_mut();
        extensions.insert(ConnectInfo(SocketAddr::from((peer, 40_000))));
        extensions.insert(TransportSecurity::Unsecure);
        let response = app().oneshot(request).await?;
        assert_eq!(response.status(), status);
        assert_eq!(response.into_body().collect().await?.to_bytes(), text);
    }
    Ok(())
}
```

This trusts the proxy completely, so check what yours does, and that nothing else can connect from
its address:

- It must replace `X-Forwarded-Proto` in every request, also when the client sent one.
- It must add the client's address at the end of `X-Forwarded-For`, or replace the field, as an IP
  address without a port. Behind several proxies, the last address is that of another proxy and not
  the visitor's.

Mistakes in the proxy's settings are noticed: a request without an address gets a 400, and when
`X-Forwarded-Proto` is missing or repeated, routes with `secured()` or `unsecure()` fail with a 500.

The layer changes only what handlers get as `ConnectInfo` and `TransportSecurity`, in place of what
the server knew. The URI, the `Host` field and the HTTP version stay those of the proxy's request, and
the fields themselves are still there for other middleware to read.

Warded's guide has [more on transport
security](https://viell-dev.github.io/slop-warded-router/transport-security.html#behind-a-reverse-proxy).
