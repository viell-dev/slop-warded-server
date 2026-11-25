//! HTTP/3 over QUIC, run by Quinn and h3.

mod body;

use crate::{
    Error, Limits, Tls,
    context::{Connection, Security},
};
use body::{RequestBody, Responder};
use bytes::Bytes;
use h3::server::RequestResolver;
use http::{
    HeaderMap, HeaderValue, Request, Response,
    header::{CONNECTION, DATE, TE, TRANSFER_ENCODING, UPGRADE},
};
use quinn::{VarInt, crypto::rustls::QuicServerConfig};
use std::{
    io,
    net::UdpSocket,
    sync::Arc,
    time::{Duration, SystemTime},
};
use tokio::{
    sync::{OwnedSemaphorePermit, Semaphore},
    task::JoinSet,
    time::{sleep, timeout},
};
use tokio_util::sync::CancellationToken;
use tower::ServiceExt;

/// How long a finished connection waits for its client to receive everything and close.
const LINGER: Duration = Duration::from_secs(2);

/// How long closing every connection may take once they are all told to close.
const FAREWELL: Duration = Duration::from_secs(1);

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
        .max_idle_timeout(limits.idle_timeout.try_into().ok());
    config.transport_config(Arc::new(transport));
    quinn::Endpoint::new(
        quinn::EndpointConfig::default(),
        Some(config),
        socket,
        Arc::new(quinn::TokioRuntime),
    )
    .map_err(stopped)
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
        let code = VarInt::from_u64(h3::error::Code::H3_NO_ERROR.value());
        self.endpoint.close(
            code.expect("HTTP/3 codes fit a QUIC integer"),
            b"shutting down",
        );
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

struct QuicConnection {
    app: axum::Router,
    limits: Limits,
    stop: CancellationToken,
    _slot: OwnedSemaphorePermit,
}

impl QuicConnection {
    async fn serve(&self, incoming: quinn::Incoming) -> Result<(), BoxError> {
        let quic = timeout(self.limits.handshake_timeout, incoming.accept()?).await??;
        let mut connection = h3::server::builder()
            // h3 has no limit by default, which lets a small request decode to a huge one
            // (https://github.com/hyperium/h3/issues/379).
            .max_field_section_size(self.limits.head_bytes.into())
            .build(h3_quinn::Connection::new(quic.clone()))
            .await?;

        // Dropping the set aborts requests whose connection is gone.
        let mut requests = JoinSet::new();
        let mut draining = false;
        loop {
            tokio::select! {
                () = self.stop.cancelled(), if !draining => {
                    draining = true;
                    connection.shutdown(0).await?;
                }
                // A panicking handler ends its own request only.
                Some(_) = requests.join_next() => {}
                // Closing right after the last response could lose the end of it in transit, so
                // give the client time to close first.
                () = sleep(LINGER), if draining && requests.is_empty() => return Ok(()),
                accepted = connection.accept() => match accepted? {
                    Some(resolver) => {
                        let app = Connection::new(
                            self.app.clone(),
                            // The address changes when the client migrates.
                            quic.remote_address(),
                            Security::Https,
                        );
                        requests.spawn(serve_request(app, resolver, self.limits.head_timeout));
                    }
                    None => return Ok(()),
                },
            }
        }
    }
}

async fn serve_request(
    app: Connection,
    resolver: RequestResolver<h3_quinn::Connection, Bytes>,
    head_timeout: Duration,
) {
    if let Err(error) = try_serve_request(app, resolver, head_timeout).await {
        tracing::debug!(error, "an HTTP/3 request ended early");
    }
}

async fn try_serve_request(
    app: Connection,
    resolver: RequestResolver<h3_quinn::Connection, Bytes>,
    head_timeout: Duration,
) -> Result<(), BoxError> {
    let (head, stream) = timeout(head_timeout, resolver.resolve_request()).await??;
    let (send, receive) = stream.split();
    let responder = Responder::new(send);
    let (parts, ()) = head.into_parts();
    let body = RequestBody::new(receive, &parts.headers);
    let Ok(response) = app.oneshot(Request::from_parts(parts, body)).await;

    let (mut parts, body) = response.into_parts();
    prepare(&mut parts.headers);
    responder.send(Response::from_parts(parts, ()), body).await
}

/// Do for an HTTP/3 response head what Hyper does for HTTP/2.
fn prepare(headers: &mut HeaderMap) {
    // Fields about an HTTP/1.1 connection make an HTTP/3 message malformed (RFC 9114 section
    // 4.2). Middleware written for HTTP/1.1 may add them.
    for name in [
        CONNECTION,
        TRANSFER_ENCODING,
        UPGRADE,
        TE,
        http::HeaderName::from_static("keep-alive"),
        http::HeaderName::from_static("proxy-connection"),
    ] {
        headers.remove(name);
    }
    headers.entry(DATE).or_insert_with(|| {
        HeaderValue::try_from(httpdate::fmt_http_date(SystemTime::now()))
            .expect("a formatted date is a valid value")
    });
}
