//! HTTP/1.1 and HTTP/2 over TCP, run by `axum-server` and Hyper.

use crate::{
    Error, Limits, Tls,
    context::{Connection, Security},
    linger::{LINGER, Linger},
};
use axum_server::{Control, Handle, accept::Accept, tls_rustls::RustlsConfig};
use std::{
    convert::Infallible,
    future::Future,
    io,
    net::{SocketAddr, TcpListener},
    pin::Pin,
    sync::Arc,
    task::{Context, Poll, ready},
};
use tokio::{
    io::{AsyncRead, AsyncWrite, ReadBuf},
    net::TcpStream,
    sync::{OwnedSemaphorePermit, Semaphore},
    time::{Instant, Sleep, sleep},
};
use tokio_util::sync::{CancellationToken, WaitForCancellationFutureOwned};
use tower::service_fn;

/// What both TCP listeners need to run.
pub(crate) struct Shared {
    pub(crate) limits: Limits,
    pub(crate) slots: Arc<Semaphore>,
    pub(crate) control: Control,
    pub(crate) abort: CancellationToken,
}

impl Shared {
    fn admit(&self) -> Admit {
        Admit {
            slots: self.slots.clone(),
            abort: self.abort.clone(),
            linger: LINGER,
        }
    }
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
                .acceptor(shared.admit())
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
        .acceptor(shared.admit())
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
struct Admit {
    slots: Arc<Semaphore>,
    abort: CancellationToken,
    linger: Linger,
}

impl<S> Accept<TcpStream, S> for Admit {
    type Stream = Admitted;
    type Service = S;
    type Future = std::future::Ready<io::Result<(Admitted, S)>>;

    fn accept(&self, stream: TcpStream, service: S) -> Self::Future {
        std::future::ready(match self.slots.clone().try_acquire_owned() {
            Ok(slot) => stream.set_nodelay(true).map(|()| {
                // Every connection polls a token of its own, so that they do not share a lock.
                let abort = self.abort.child_token();
                let stream = Admitted {
                    stream,
                    abort_read: Box::pin(abort.clone().cancelled_owned()),
                    abort_write: Box::pin(abort.cancelled_owned()),
                    linger: self.linger,
                    closing: Closing::Open,
                    _slot: slot,
                };
                (stream, service)
            }),
            Err(_) => Err(io::Error::other("the connection limit is reached")),
        })
    }
}

/// A connection that gives its slot back when it closes, and fails when the server aborts.
///
/// Neither depends on `axum-server`, which loses sight of a connection that is upgraded.
struct Admitted {
    stream: TcpStream,
    abort_read: Pin<Box<WaitForCancellationFutureOwned>>,
    abort_write: Pin<Box<WaitForCancellationFutureOwned>>,
    linger: Linger,
    closing: Closing,
    _slot: OwnedSemaphorePermit,
}

/// How far the server's side of a connection is closed.
enum Closing {
    Open,
    /// It sends no more, and takes what the client sends until the timer ends, which is never after
    /// the deadline.
    Lingering {
        timer: Pin<Box<Sleep>>,
        deadline: Instant,
    },
    Closed,
}

impl Admitted {
    fn poll_aborted(
        abort: Pin<&mut WaitForCancellationFutureOwned>,
        cx: &mut Context<'_>,
    ) -> io::Result<()> {
        match abort.poll(cx) {
            Poll::Ready(()) => Err(io::ErrorKind::ConnectionAborted.into()),
            Poll::Pending => Ok(()),
        }
    }
}

impl AsyncRead for Admitted {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buffer: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        Self::poll_aborted(self.abort_read.as_mut(), cx)?;
        Pin::new(&mut self.stream).poll_read(cx, buffer)
    }
}

impl AsyncWrite for Admitted {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bytes: &[u8],
    ) -> Poll<io::Result<usize>> {
        Self::poll_aborted(self.abort_write.as_mut(), cx)?;
        Pin::new(&mut self.stream).poll_write(cx, bytes)
    }

    fn poll_write_vectored(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        slices: &[io::IoSlice<'_>],
    ) -> Poll<io::Result<usize>> {
        Self::poll_aborted(self.abort_write.as_mut(), cx)?;
        Pin::new(&mut self.stream).poll_write_vectored(cx, slices)
    }

    fn is_write_vectored(&self) -> bool {
        self.stream.is_write_vectored()
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.stream).poll_flush(cx)
    }

    /// Close the sending side, and wait for the client to close its own.
    ///
    /// Closing a connection with data from the client unread, or that is still to come, makes the
    /// operating system reset it, and the client can lose what it has not read of the response
    /// (RFC 9112 section 9.6). HTTP/2 clients in particular send as they read.
    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let this = &mut *self;
        loop {
            match &mut this.closing {
                Closing::Open => {
                    ready!(Pin::new(&mut this.stream).poll_shutdown(cx))?;
                    this.closing = Closing::Lingering {
                        timer: Box::pin(sleep(this.linger.silence.min(this.linger.longest))),
                        deadline: Instant::now() + this.linger.longest,
                    };
                }
                Closing::Lingering { timer, deadline } => {
                    Self::poll_aborted(this.abort_write.as_mut(), cx)?;
                    let over = timer.as_mut().poll(cx).is_ready();
                    let mut discarded = [0; 4096];
                    let mut buffer = ReadBuf::new(&mut discarded);
                    match Pin::new(&mut this.stream).poll_read(cx, &mut buffer) {
                        Poll::Ready(Ok(())) if !over && !buffer.filled().is_empty() => {
                            let silence = Instant::now() + this.linger.silence;
                            timer.as_mut().reset(silence.min(*deadline));
                        }
                        Poll::Pending if !over => return Poll::Pending,
                        // The client has closed, or the wait is over.
                        _ => this.closing = Closing::Closed,
                    }
                }
                Closing::Closed => return Poll::Ready(Ok(())),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        time::timeout,
    };

    const SHORT: Duration = Duration::from_millis(300);
    const LONG: Duration = Duration::from_secs(10);

    /// Both ends of a connection, with the server's admitted.
    async fn connection(silence: Duration, longest: Duration) -> (Admitted, TcpStream) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let client = TcpStream::connect(listener.local_addr().unwrap());
        let (client, accepted) = tokio::join!(client, listener.accept());
        let admit = Admit {
            slots: Arc::new(Semaphore::new(1)),
            abort: CancellationToken::new(),
            linger: Linger { silence, longest },
        };
        let (server, ()) = admit.accept(accepted.unwrap().0, ()).await.unwrap();
        (server, client.unwrap())
    }

    #[tokio::test]
    async fn closing_waits_for_the_client() {
        let (mut server, mut client) = connection(LONG, LONG).await;
        let mut closing = Box::pin(server.shutdown());
        assert!(timeout(SHORT, &mut closing).await.is_err());
        assert_eq!(client.read(&mut [0; 1]).await.unwrap(), 0);
        client.write_all(b"more").await.unwrap();
        assert!(timeout(SHORT, &mut closing).await.is_err());
        drop(client);
        timeout(LONG / 2, closing).await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn closing_waits_no_longer_than_a_silence() {
        let (mut server, mut client) = connection(SHORT, LONG).await;
        timeout(LONG / 2, server.shutdown()).await.unwrap().unwrap();
        // It does not begin again.
        client.write_all(b"more").await.unwrap();
        let mut again = Box::pin(server.shutdown());
        let at_once = std::future::poll_fn(|cx| Poll::Ready(again.as_mut().poll(cx))).await;
        assert!(at_once.is_ready());
    }

    #[tokio::test]
    async fn closing_waits_no_longer_than_the_deadline() {
        // For a client that sends something and is then silent.
        let (mut server, mut client) = connection(LONG, 3 * SHORT).await;
        let mut closing = Box::pin(server.shutdown());
        assert!(timeout(SHORT, &mut closing).await.is_err());
        client.write_all(b"more").await.unwrap();
        timeout(LONG / 2, closing).await.unwrap().unwrap();

        // For one that is never silent.
        let (mut server, mut client) = connection(SHORT, 3 * SHORT).await;
        let sending = tokio::spawn(async move {
            while client.write_all(b"more").await.is_ok() {
                tokio::time::sleep(SHORT / 10).await;
            }
        });
        timeout(LONG / 2, server.shutdown()).await.unwrap().unwrap();
        sending.abort();
    }
}
