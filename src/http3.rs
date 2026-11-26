//! HTTP/3 over QUIC, run by Quinn and h3.

mod body;
mod frames;
mod head;
mod transport;

use crate::{
    Error, Limits, Tls,
    context::{Connection, Security},
};
use axum::response::IntoResponse;
use body::{RequestBody, Responder};
use bytes::Bytes;
use h3::{
    error::{Code, StreamError},
    server::RequestResolver,
};
use http::{Method, Request, StatusCode};
use quinn::{VarInt, crypto::rustls::QuicServerConfig};
use std::{io, net::UdpSocket, sync::Arc, time::Duration};
use tokio::{
    sync::{OwnedSemaphorePermit, Semaphore},
    task::JoinSet,
    time::timeout,
};
use tokio_util::sync::CancellationToken;
use tower::ServiceExt;
use transport::{Bodies, Transport};

/// How long a finished connection may send nothing before it is closed.
const LINGER: Duration = Duration::from_secs(2);

/// How long closing every connection may take once they are all told to close.
const FAREWELL: Duration = Duration::from_secs(1);

/// How much of a request a client may send before it is read. This is Quinn's default, which allows
/// 100 Mbit/s over a round trip of 100 ms.
const STREAM_WINDOW: u32 = 1_250_000;

/// How much of all requests of a connection may be unread. Quinn has no limit by default, and for
/// responses it has this one.
const CONNECTION_WINDOW: u32 = 8 * STREAM_WINDOW;

/// How many times larger than `Limits::head_bytes` an encoded head may be. h3 answers one that is
/// decoded to more than that limit, and one that is not decoded gets no response.
const ENCODED_HEAD: u64 = 2;

/// How many request heads of a connection may fail to arrive before it is closed. h3 0.0.8 remembers
/// each one until then (https://github.com/hyperium/h3/pull/365).
const LOST_HEADS: u32 = 1_000;

/// The QUIC endpoint and what its connections need.
pub(crate) struct Http3 {
    pub(crate) endpoint: quinn::Endpoint,
    pub(crate) limits: Limits,
    pub(crate) slots: Arc<Semaphore>,
}

/// Start QUIC on a bound socket. Nothing is accepted until [`Http3::serve`].
pub(crate) fn endpoint(
    socket: UdpSocket,
    tls: &Tls,
    limits: &Limits,
) -> Result<quinn::Endpoint, Error> {
    // `max_early_data_size` stays 0: a replayed 0-RTT request could repeat its effects.
    let config = tls.config(&[b"h3"], &[&rustls::version::TLS13])?;
    let config = QuicServerConfig::try_from(config)
        .map_err(|error| Error::Tls(rustls::Error::General(error.to_string())))?;
    let mut config = quinn::ServerConfig::with_crypto(Arc::new(config));
    let mut transport = quinn::TransportConfig::default();
    transport
        .max_concurrent_bidi_streams(limits.concurrent_streams.into())
        // A client needs three: control and the two for QPACK. Others are ignored extensions.
        .max_concurrent_uni_streams(8_u8.into())
        .stream_receive_window(STREAM_WINDOW.into())
        .receive_window(CONNECTION_WINDOW.into())
        // Nothing reads them.
        .datagram_receive_buffer_size(None)
        .max_idle_timeout(Some(
            limits
                .idle_timeout
                .try_into()
                .expect("`Limits::validate` refuses timeouts that QUIC cannot express"),
        ));
    config.transport_config(Arc::new(transport));
    #[cfg(target_os = "linux")]
    let same_socket = socket.try_clone().map_err(stopped)?;
    let endpoint = quinn::Endpoint::new(
        quinn::EndpointConfig::default(),
        Some(config),
        socket,
        Arc::new(quinn::TokioRuntime),
    )
    .map_err(stopped)?;
    #[cfg(target_os = "linux")]
    receive_packets_apart(&same_socket).map_err(stopped)?;
    Ok(endpoint)
}

/// Turn off that Linux hands over many packets as one, which Quinn turns on.
///
/// Quinn keeps all of them in memory for as long as data from one is unread, and counts only that
/// data. A client could otherwise make unread requests take many times their size.
#[cfg(target_os = "linux")]
fn receive_packets_apart(socket: &UdpSocket) -> io::Result<()> {
    use nix::sys::socket::{getsockopt, setsockopt, sockopt::UdpGroSegment};

    // Linux before 5.0 does not have it, and then this fails.
    let turned_off = setsockopt(socket, UdpGroSegment, &false);
    match getsockopt(socket, UdpGroSegment) {
        Ok(true) => Err(turned_off.err().map_or_else(
            || io::Error::other("UDP receive offload stays on"),
            io::Error::from,
        )),
        Ok(false) | Err(_) => Ok(()),
    }
}

fn stopped(source: io::Error) -> Error {
    Error::Listener {
        listener: "HTTP/3",
        source,
    }
}

impl Http3 {
    /// Serve until `stop`, then drain until every connection ends or `abort`.
    pub(crate) async fn serve(
        self,
        app: axum::Router,
        stop: CancellationToken,
        abort: CancellationToken,
    ) -> Result<(), Error> {
        let mut connections = JoinSet::new();
        let result = loop {
            tokio::select! {
                () = stop.cancelled() => break Ok(()),
                Some(_) = connections.join_next() => {}
                incoming = self.endpoint.accept() => match incoming {
                    Some(incoming) => self.admit(incoming, &app, &stop, &mut connections),
                    None => break Err(stopped(io::Error::other("the endpoint closed"))),
                },
            }
        };
        loop {
            tokio::select! {
                () = abort.cancelled() => break connections.abort_all(),
                ended = connections.join_next() => if ended.is_none() { break },
                // Without an answer, clients would keep trying to connect until they time out.
                Some(incoming) = self.endpoint.accept() => incoming.refuse(),
            }
        }
        self.endpoint
            .close(code(Code::H3_NO_ERROR), b"shutting down");
        let _ = timeout(FAREWELL, self.endpoint.wait_idle()).await;
        result
    }

    fn admit(
        &self,
        incoming: quinn::Incoming,
        app: &axum::Router,
        stop: &CancellationToken,
        connections: &mut JoinSet<()>,
    ) {
        let Ok(slot) = self.slots.clone().try_acquire_owned() else {
            incoming.refuse();
            return;
        };
        // When slots run low, make clients prove that they receive at their address, so that
        // spoofed addresses cannot take the rest.
        let scarce = self.slots.available_permits() < self.limits.connections / 2;
        if scarce && incoming.may_retry() {
            let _ = incoming.retry();
            return;
        }
        let connection = QuicConnection {
            app: app.clone(),
            limits: self.limits.clone(),
            stop: stop.clone(),
            _slot: slot,
        };
        connections.spawn(async move {
            let peer = incoming.remote_address();
            if let Err(error) = connection.serve(incoming).await {
                tracing::debug!(%peer, error, "an HTTP/3 connection ended");
            }
        });
    }
}

type BoxError = Box<dyn std::error::Error + Send + Sync>;

fn code(code: Code) -> VarInt {
    VarInt::from_u64(code.value()).expect("HTTP/3 codes fit a QUIC integer")
}

struct QuicConnection {
    app: axum::Router,
    limits: Limits,
    stop: CancellationToken,
    _slot: OwnedSemaphorePermit,
}

impl QuicConnection {
    async fn serve(&self, incoming: quinn::Incoming) -> Result<(), BoxError> {
        let bodies = Bodies::new();
        let handshake = async {
            let quic = incoming.accept()?.await?;
            // This waits until the client lets the server open its control stream.
            let connection = h3::server::builder()
                // h3 has no limit by default, which lets a small request decode to a huge one
                // (https://github.com/hyperium/h3/issues/379).
                .max_field_section_size(self.limits.head_bytes.into())
                .build(Transport::new(
                    quic.clone(),
                    u64::from(self.limits.head_bytes) * ENCODED_HEAD,
                    bodies.clone(),
                ))
                .await?;
            Ok::<_, BoxError>((quic, connection))
        };
        let (quic, mut connection) = timeout(self.limits.handshake_timeout, handshake).await??;

        // Dropping the set aborts requests whose connection is gone.
        let mut requests = JoinSet::new();
        let mut lost_heads = 0;
        let mut draining = false;
        loop {
            let accepted = {
                // Kept while requests end, because h3 can be in the middle of writing a GOAWAY, which
                // it does once the client has said goodbye and has all its responses. Stopping can
                // still interrupt that, when there is nothing left to say it for.
                let mut accept = std::pin::pin!(connection.accept());
                loop {
                    tokio::select! {
                        accepted = &mut accept => break Some(accepted?),
                        () = self.stop.cancelled(), if !draining => break None,
                        // A panicking handler ends its own request only.
                        Some(ended) = requests.join_next() => {
                            if let Ok(Head::Lost) = ended {
                                lost_heads += 1;
                            }
                            if lost_heads > LOST_HEADS && !draining {
                                break None;
                            }
                        }
                        () = farewell(&quic, &bodies), if draining && requests.is_empty() => {
                            return Ok(());
                        }
                    }
                }
            };
            match accepted {
                None => {
                    draining = true;
                    // The GOAWAY that this sends tells the client that requests from its number
                    // on are not processed. With 0, h3 names the last request that it accepted.
                    connection.shutdown(1).await?;
                }
                // h3 itself rejects only the ones after the number in the GOAWAY.
                Some(Some(resolver)) if draining => {
                    requests.spawn(reject_request(resolver, self.limits.head_timeout));
                }
                Some(Some(resolver)) => {
                    let app = Connection::new(
                        self.app.clone(),
                        // The address changes when the client migrates.
                        quic.remote_address(),
                        Security::Https,
                    );
                    requests.spawn(serve_request(app, resolver, self.limits.head_timeout));
                }
                // The client announced that it has no more requests, and all are answered.
                Some(None) => break,
            }
        }
        while requests.join_next().await.is_some() {}
        farewell(&quic, &bodies).await;
        Ok(())
    }
}

/// Wait until the client is done sending, and then as `linger` does.
async fn farewell(quic: &quinn::Connection, bodies: &Bodies) {
    bodies.ended().await;
    linger(quic).await;
}

/// Wait until the client closes the connection, or has been sent everything.
///
/// Finishing a response only hands the end of it to Quinn, and closing the connection discards
/// what Quinn still has to send or repeat. h3 does not report when the client has it all.
async fn linger(quic: &quinn::Connection) {
    loop {
        let sent = quic.stats().frame_tx.stream;
        if timeout(LINGER, quic.closed()).await.is_ok() || quic.stats().frame_tx.stream == sent {
            return;
        }
    }
}

type Resolver = RequestResolver<Transport, Bytes>;
type RequestStream = h3::server::RequestStream<transport::Request, Bytes>;

/// What became of the head of a request.
enum Head {
    Arrived,
    /// It is incomplete, and h3 does not forget the request.
    Lost,
}

/// Read the head of a request. Without one, the request has been answered as far as possible.
async fn resolve(
    resolver: Resolver,
    head_timeout: Duration,
) -> Result<(Request<()>, RequestStream), Head> {
    let error = match timeout(head_timeout, resolver.resolve_request()).await {
        Ok(Ok(request)) => return Ok(request),
        Ok(Err(error)) => error,
        Err(_) => {
            tracing::debug!("an HTTP/3 request head did not arrive in time");
            return Err(Head::Lost);
        }
    };
    tracing::debug!(%error, "an HTTP/3 request head was refused");
    Err(match error {
        StreamError::HeaderTooBig { .. } => Head::Arrived,
        StreamError::StreamError { code, .. } if code == Code::H3_MESSAGE_ERROR => Head::Arrived,
        _ => Head::Lost,
    })
}

/// Tell the client that it can safely repeat this request elsewhere.
async fn reject_request(resolver: Resolver, head_timeout: Duration) -> Head {
    match resolve(resolver, head_timeout).await {
        Ok((_, mut stream)) => {
            stream.stop_sending(Code::H3_REQUEST_REJECTED);
            stream.stop_stream(Code::H3_REQUEST_REJECTED);
            Head::Arrived
        }
        Err(head) => head,
    }
}

async fn serve_request(app: Connection, resolver: Resolver, head_timeout: Duration) -> Head {
    let (head, stream) = match resolve(resolver, head_timeout).await {
        Ok(request) => request,
        Err(head) => return head,
    };
    if let Err(error) = respond(app, head, stream).await {
        tracing::debug!(error, "an HTTP/3 request ended early");
    }
    Head::Arrived
}

async fn respond(
    app: Connection,
    head: Request<()>,
    stream: RequestStream,
) -> Result<(), BoxError> {
    let (send, mut receive) = stream.split();
    let responder = Responder::new(send);
    let (mut parts, ()) = head.into_parts();
    let checked =
        crate::head::check(&mut parts).and_then(|()| head::content_length(&parts.headers));
    let body = match checked {
        Ok(length) => RequestBody::new(receive, length),
        Err(malformed) => {
            receive.stop_sending(Code::H3_MESSAGE_ERROR);
            responder.reset(Code::H3_MESSAGE_ERROR);
            return Err(malformed.into());
        }
    };
    let to_head = parts.method == Method::HEAD;
    let response = if parts.method == Method::CONNECT {
        // There is no way to give the application the stream for a tunnel.
        StatusCode::NOT_IMPLEMENTED.into_response()
    } else {
        let Ok(response) = app.oneshot(Request::from_parts(parts, body)).await;
        response
    };
    responder.send(head::prepare(response), to_head).await
}

#[cfg(all(test, target_os = "linux", feature = "self-signed"))]
mod tests {
    use super::*;
    use nix::sys::socket::{getsockopt, sockopt::UdpGroSegment};

    #[tokio::test]
    async fn packets_are_received_apart() {
        let socket = crate::listener::udp(([127, 0, 0, 1], 0).into()).unwrap();
        let same_socket = socket.try_clone().unwrap();
        let tls = Tls::self_signed(["localhost"]).unwrap();
        let _endpoint = endpoint(socket, &tls, &Limits::default()).unwrap();
        assert_ne!(getsockopt(&same_socket, UdpGroSegment), Ok(true));
    }
}
