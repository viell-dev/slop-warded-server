//! HTTP/1.1 and HTTP/2 over TCP, run by `axum-server` and Hyper.

use crate::{
    Error, Limits, Tls,
    context::{Connection, Security},
};
use axum_server::{Control, Handle, accept::Accept, tls_rustls::RustlsConfig};
use std::{
    convert::Infallible,
    io,
    net::{SocketAddr, TcpListener},
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
};
use tokio::{
    io::{AsyncRead, AsyncWrite, ReadBuf},
    net::TcpStream,
    sync::{OwnedSemaphorePermit, Semaphore},
};
use tower::service_fn;

/// What both TCP listeners need to run.
pub(crate) struct Shared {
    pub(crate) limits: Limits,
    pub(crate) slots: Arc<Semaphore>,
    pub(crate) control: Control,
}

/// Serve HTTPS until the shared control stops it and its connections have ended.
pub(crate) async fn serve_https(
    listener: TcpListener,
    tls: &Tls,
    app: axum::Router,
    shared: &Shared,
) -> Result<(), Error> {
    let config = tls.config(
        &[b"h2", b"http/1.1"],
        &[&rustls::version::TLS13, &rustls::version::TLS12],
    )?;
    let listener = axum_server::from_tcp_rustls(listener, RustlsConfig::from_config(config.into()))
        .map_err(stopped(HTTPS))?;
    let mut server = configure(listener, &shared.limits)
        .handle(Handle::with_control(shared.control.clone()))
        .map(|acceptor| {
            acceptor
                .handshake_timeout(shared.limits.handshake_timeout)
                .acceptor(Admit(shared.slots.clone()))
        });
    server = server.accept_error_handler(|peer, error| {
        tracing::debug!(?peer, %error, "a TCP connection did not become HTTPS");
    });
    server
        .serve(connections(app, Security::Https))
        .await
        .map_err(stopped(HTTPS))
}

/// Serve cleartext HTTP until the shared control stops it and its connections have ended.
pub(crate) async fn serve_http(
    listener: TcpListener,
    app: axum::Router,
    shared: &Shared,
) -> Result<(), Error> {
    let listener = axum_server::from_tcp(listener).map_err(stopped(HTTP))?;
    configure(listener, &shared.limits)
        .handle(Handle::with_control(shared.control.clone()))
        .acceptor(Admit(shared.slots.clone()))
        .accept_error_handler(|peer, error| {
            tracing::debug!(?peer, %error, "a TCP connection was not accepted");
        })
        .serve(connections(app, Security::Cleartext))
        .await
        .map_err(stopped(HTTP))
}

const HTTPS: &str = "HTTPS";
const HTTP: &str = "HTTP";

fn stopped(listener: &'static str) -> impl Fn(io::Error) -> Error {
    move |source| Error::Listener { listener, source }
}

fn configure<A>(
    server: axum_server::Server<SocketAddr, A>,
    limits: &Limits,
) -> axum_server::Server<SocketAddr, A> {
    /// Hyper refuses a smaller HTTP/1 buffer.
    const HTTP1_MINIMUM_BUFFER: usize = 8 * 1024;

    let mut server = server.first_request_timeout(limits.head_timeout);
    let builder = server.http_builder();
    builder
        .http1()
        .header_read_timeout(limits.head_timeout)
        .max_buf_size((limits.head_bytes as usize).max(HTTP1_MINIMUM_BUFFER));
    builder
        .http2()
        .max_concurrent_streams(limits.concurrent_streams)
        .max_header_list_size(limits.head_bytes)
        .keep_alive_interval(limits.idle_timeout)
        .keep_alive_timeout(limits.idle_timeout);
    server
}

/// Make each connection's service from its peer address.
fn connections(
    app: axum::Router,
    security: Security,
) -> impl tower::Service<SocketAddr, Response = Connection, Error = Infallible, Future: Send>
+ Clone
+ Send {
    service_fn(move |peer| {
        let connection = Connection::new(app.clone(), peer, security);
        async move { Ok(connection) }
    })
}

/// Takes one of the server's connection slots, or closes the connection when none is free.
#[derive(Clone)]
struct Admit(Arc<Semaphore>);

impl<S> Accept<TcpStream, S> for Admit {
    type Stream = Admitted;
    type Service = S;
    type Future = std::future::Ready<io::Result<(Admitted, S)>>;

    fn accept(&self, stream: TcpStream, service: S) -> Self::Future {
        std::future::ready(match self.0.clone().try_acquire_owned() {
            Ok(slot) => stream.set_nodelay(true).map(|()| {
                let stream = Admitted {
                    stream,
                    _slot: slot,
                };
                (stream, service)
            }),
            Err(_) => Err(io::Error::other("the connection limit is reached")),
        })
    }
}

/// A connection that gives its slot back when it closes, including after a protocol upgrade.
struct Admitted {
    stream: TcpStream,
    _slot: OwnedSemaphorePermit,
}

impl AsyncRead for Admitted {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buffer: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        Pin::new(&mut self.stream).poll_read(cx, buffer)
    }
}

impl AsyncWrite for Admitted {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bytes: &[u8],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.stream).poll_write(cx, bytes)
    }

    fn poll_write_vectored(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        slices: &[io::IoSlice<'_>],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.stream).poll_write_vectored(cx, slices)
    }

    fn is_write_vectored(&self) -> bool {
        self.stream.is_write_vectored()
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.stream).poll_flush(cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.stream).poll_shutdown(cx)
    }
}
