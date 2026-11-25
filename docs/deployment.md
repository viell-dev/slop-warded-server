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
- **A proxy or CDN that ends TLS** speaks HTTP/1.1 or HTTP/2 to this server. It can connect to the
  HTTPS address. Visitors then use the protocols of the proxy, and your routes see the proxy's
  connection instead of theirs.
