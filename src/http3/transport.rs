//! Quinn as the transport of h3, with what h3 reads bounded.
//!
//! h3-quinn does this without the bounds. It takes whatever a stream has received, which can be a
//! megabyte at once, and what is taken counts as received: the client may send as much again.

use super::{
    BoxError, code,
    frames::{FrameError, Frames},
};
use crate::linger::LINGER;
use bytes::{Buf, Bytes};
use h3::{
    error::Code,
    quic::{self, ConnectionErrorIncoming, StreamErrorIncoming, StreamId, WriteBuf},
};
use quinn::{ConnectionError, ReadError, WriteError};
use std::{
    pin::Pin,
    sync::Arc,
    task::{Context, Poll, Waker, ready},
};
use tokio::{
    runtime::Handle,
    task::yield_now,
    time::{Instant, timeout_at},
};
use tokio_util::task::{TaskTracker, task_tracker::TaskTrackerToken};

/// How much to take from a stream at once. QUIC no longer counts it as unread, so it is small.
const READ: usize = 4 * 1024;

/// How often to take more before other tasks have their turn, when none of it is for h3.
const READS: usize = 32;

type Opening<T> = Pin<Box<dyn Future<Output = Result<T, ConnectionError>> + Send>>;

/// A QUIC connection whose streams h3 reads frame by frame.
pub(super) struct Transport {
    quic: quinn::Connection,
    head_frame_bytes: u64,
    bodies: Bodies,
    requests: Opening<(quinn::SendStream, quinn::RecvStream)>,
    unidirectional: Opening<quinn::RecvStream>,
    opening: Option<Opening<quinn::SendStream>>,
}

impl Transport {
    /// `head_frame_bytes` is the longest frame on a request stream, other than a DATA frame.
    pub(super) fn new(quic: quinn::Connection, head_frame_bytes: u64, bodies: Bodies) -> Self {
        Self {
            requests: accept_bi(&quic),
            unidirectional: accept_uni(&quic),
            opening: None,
            quic,
            head_frame_bytes,
            bodies,
        }
    }
}

/// The request bodies of a connection whose clients may not be done sending.
#[derive(Clone)]
pub(super) struct Bodies {
    tracker: TaskTracker,
    /// Where the rest of a body is discarded, wherever the application drops it.
    runtime: Handle,
}

impl Bodies {
    pub(super) fn new() -> Self {
        let tracker = TaskTracker::new();
        // This lets waiting end. It does not keep more bodies from being tracked.
        tracker.close();
        Self {
            tracker,
            runtime: Handle::current(),
        }
    }

    /// Wait until there are none.
    pub(super) async fn ended(&self) {
        self.tracker.wait().await;
    }
}

fn accept_bi(quic: &quinn::Connection) -> Opening<(quinn::SendStream, quinn::RecvStream)> {
    let quic = quic.clone();
    Box::pin(async move { quic.accept_bi().await })
}

fn accept_uni(quic: &quinn::Connection) -> Opening<quinn::RecvStream> {
    let quic = quic.clone();
    Box::pin(async move { quic.accept_uni().await })
}

impl quic::Connection<Bytes> for Transport {
    type RecvStream = Receiver;
    type OpenStreams = Self;

    fn poll_accept_recv(
        &mut self,
        cx: &mut Context<'_>,
    ) -> Poll<Result<Receiver, ConnectionErrorIncoming>> {
        let accepted = ready!(self.unidirectional.as_mut().poll(cx));
        self.unidirectional = accept_uni(&self.quic);
        let stream = accepted.map_err(connection_error)?;
        Poll::Ready(Ok(Receiver::new(stream, Frames::unidirectional())))
    }

    fn poll_accept_bidi(
        &mut self,
        cx: &mut Context<'_>,
    ) -> Poll<Result<Request, ConnectionErrorIncoming>> {
        let accepted = ready!(self.requests.as_mut().poll(cx));
        self.requests = accept_bi(&self.quic);
        let (sender, receiver) = accepted.map_err(connection_error)?;
        Poll::Ready(Ok(Request {
            sender: Sender::new(sender),
            receiver: Receiver::new(receiver, Frames::request(self.head_frame_bytes)),
            bodies: self.bodies.clone(),
        }))
    }

    fn opener(&self) -> Self {
        Self::new(
            self.quic.clone(),
            self.head_frame_bytes,
            self.bodies.clone(),
        )
    }
}

impl quic::OpenStreams<Bytes> for Transport {
    type BidiStream = Request;
    type SendStream = Sender;

    fn poll_open_bidi(
        &mut self,
        _: &mut Context<'_>,
    ) -> Poll<Result<Request, StreamErrorIncoming>> {
        // Only clients send requests.
        Poll::Ready(Err(misused("a server opens no request streams")))
    }

    fn poll_open_send(
        &mut self,
        cx: &mut Context<'_>,
    ) -> Poll<Result<Sender, StreamErrorIncoming>> {
        let opening = self.opening.get_or_insert_with(|| {
            let quic = self.quic.clone();
            Box::pin(async move { quic.open_uni().await })
        });
        let opened = ready!(opening.as_mut().poll(cx));
        self.opening = None;
        let stream = opened.map_err(connection_lost)?;
        Poll::Ready(Ok(Sender::new(stream)))
    }

    fn close(&mut self, reason: Code, message: &[u8]) {
        self.quic.close(code(reason), message);
    }
}

/// Both directions of a request stream.
pub(super) struct Request {
    sender: Sender,
    receiver: Receiver,
    bodies: Bodies,
}

impl quic::BidiStream<Bytes> for Request {
    type SendStream = Sender;
    type RecvStream = Receiver;

    /// The head has arrived, and the receiver is for the body.
    fn split(mut self) -> (Sender, Receiver) {
        let token = self.bodies.tracker.token();
        self.receiver.body = Some((token, self.bodies.runtime));
        (self.sender, self.receiver)
    }
}

impl quic::RecvStream for Request {
    type Buf = Bytes;

    fn poll_data(
        &mut self,
        cx: &mut Context<'_>,
    ) -> Poll<Result<Option<Bytes>, StreamErrorIncoming>> {
        let data = ready!(self.receiver.poll_data(cx));
        if let Err(StreamErrorIncoming::Unknown(error)) = &data
            && let Some(FrameError::Excessive { .. }) = error.downcast_ref()
        {
            // There will be no response, and Quinn would end it normally.
            quic::SendStream::reset(&mut self.sender, Code::H3_EXCESSIVE_LOAD.value());
        }
        Poll::Ready(data)
    }

    fn stop_sending(&mut self, reason: u64) {
        self.receiver.stop_sending(reason);
    }

    fn recv_id(&self) -> StreamId {
        self.receiver.recv_id()
    }
}

impl quic::SendStream<Bytes> for Request {
    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), StreamErrorIncoming>> {
        self.sender.poll_ready(cx)
    }

    fn send_data<D: Into<WriteBuf<Bytes>>>(&mut self, data: D) -> Result<(), StreamErrorIncoming> {
        self.sender.send_data(data)
    }

    fn poll_finish(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), StreamErrorIncoming>> {
        self.sender.poll_finish(cx)
    }

    fn reset(&mut self, reason: u64) {
        self.sender.reset(reason);
    }

    fn send_id(&self) -> StreamId {
        self.sender.send_id()
    }
}

/// The receiving direction of a stream.
pub(super) struct Receiver {
    /// Only dropping takes it.
    stream: Option<quinn::RecvStream>,
    frames: Frames,
    /// What was taken from the stream and is after the piece that h3 is at.
    unread: Bytes,
    /// For the body of a request, until the client is done or has been told to stop.
    body: Option<(TaskTrackerToken, Handle)>,
}

impl Receiver {
    fn new(stream: quinn::RecvStream, frames: Frames) -> Self {
        Self {
            stream: Some(stream),
            frames,
            unread: Bytes::new(),
            body: None,
        }
    }

    fn poll_read(
        &mut self,
        cx: &mut Context<'_>,
        data: &mut [u8],
    ) -> Poll<Result<usize, ReadError>> {
        let stream = self.stream.as_mut().expect("it is not dropped");
        let read = ready!(stream.poll_read(cx, data));
        if let Ok(0) | Err(_) = read {
            self.body = None;
        }
        Poll::Ready(read)
    }
}

impl quic::RecvStream for Receiver {
    type Buf = Bytes;

    fn poll_data(
        &mut self,
        cx: &mut Context<'_>,
    ) -> Poll<Result<Option<Bytes>, StreamErrorIncoming>> {
        for _ in 0..READS {
            match self.frames.next(&mut self.unread) {
                Ok(None) => {}
                Ok(piece) => return Poll::Ready(Ok(piece)),
                Err(error) => {
                    self.stop_sending(Code::H3_EXCESSIVE_LOAD.value());
                    return Poll::Ready(Err(unknown(error)));
                }
            }
            // Copied, because Quinn's own pieces keep the packets in memory that they came in.
            let mut data = [0; READ];
            match ready!(self.poll_read(cx, &mut data)).map_err(read_error)? {
                0 => return Poll::Ready(self.frames.end().map_err(unknown)),
                length => self.unread = Bytes::copy_from_slice(&data[..length]),
            }
        }
        cx.waker().wake_by_ref();
        Poll::Pending
    }

    fn stop_sending(&mut self, reason: u64) {
        self.body = None;
        if let Some(stream) = &mut self.stream {
            // A stream that has ended needs no more.
            let _ = stream.stop(code(reason.into()));
        }
    }

    fn recv_id(&self) -> StreamId {
        id(self.stream.as_ref().expect("it is not dropped").id())
    }
}

impl Drop for Receiver {
    /// Let the client finish a body that the application did not read.
    fn drop(&mut self) {
        if self.body.is_some() {
            // Most requests have no body, and end with this.
            let _ = self.poll_read(&mut Context::from_waker(Waker::noop()), &mut [0; READ]);
        }
        if let (Some((token, runtime)), Some(stream)) = (self.body.take(), self.stream.take()) {
            let deadline = Instant::now() + LINGER.longest;
            runtime.spawn(async move {
                discard(stream, deadline).await;
                drop(token);
            });
        }
    }
}

/// Take what the client still sends, for a while, and then tell it to stop.
///
/// It may be told at once (RFC 9114 section 4.1), but curl and Chromium then fail or hang, although
/// they get a complete response.
async fn discard(mut stream: quinn::RecvStream, deadline: Instant) {
    while Instant::now() < deadline {
        let silence = Instant::now() + LINGER.silence;
        // In order, because Quinn otherwise remembers every range that it has received, without a
        // limit on their number.
        let read = timeout_at(silence.min(deadline), stream.read_chunk(usize::MAX, true));
        match read.await {
            // Other tasks have their turn, however fast the client sends.
            Ok(Ok(Some(_))) => yield_now().await,
            _ => break,
        }
    }
    let _ = stream.stop(code(Code::H3_NO_ERROR));
}

/// The sending direction of a stream.
pub(super) struct Sender {
    stream: quinn::SendStream,
    writing: Option<WriteBuf<Bytes>>,
}

impl Sender {
    fn new(stream: quinn::SendStream) -> Self {
        Self {
            stream,
            writing: None,
        }
    }
}

impl quic::SendStream<Bytes> for Sender {
    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), StreamErrorIncoming>> {
        if let Some(data) = &mut self.writing {
            while data.has_remaining() {
                let written = Pin::new(&mut self.stream).poll_write(cx, data.chunk());
                data.advance(ready!(written).map_err(write_error)?);
            }
            self.writing = None;
        }
        Poll::Ready(Ok(()))
    }

    fn send_data<D: Into<WriteBuf<Bytes>>>(&mut self, data: D) -> Result<(), StreamErrorIncoming> {
        if self.writing.is_some() {
            return Err(misused("the stream is not ready for more data"));
        }
        self.writing = Some(data.into());
        Ok(())
    }

    fn poll_finish(&mut self, _: &mut Context<'_>) -> Poll<Result<(), StreamErrorIncoming>> {
        Poll::Ready(self.stream.finish().map_err(unknown))
    }

    fn reset(&mut self, reason: u64) {
        // A stream that has ended cannot be reset any more.
        let _ = self.stream.reset(code(reason.into()));
    }

    fn send_id(&self) -> StreamId {
        id(self.stream.id())
    }
}

fn id(stream: quinn::StreamId) -> StreamId {
    u64::from(stream)
        .try_into()
        .expect("QUIC and HTTP/3 have the same stream ids")
}

/// A mistake in h3's use of this module. It ends the connection.
fn misused(reason: &str) -> StreamErrorIncoming {
    StreamErrorIncoming::ConnectionErrorIncoming {
        connection_error: ConnectionErrorIncoming::InternalError(reason.to_owned()),
    }
}

fn unknown(error: impl Into<BoxError>) -> StreamErrorIncoming {
    StreamErrorIncoming::Unknown(error.into())
}

fn connection_error(error: ConnectionError) -> ConnectionErrorIncoming {
    match error {
        ConnectionError::ApplicationClosed(close) => ConnectionErrorIncoming::ApplicationClose {
            error_code: close.error_code.into(),
        },
        ConnectionError::TimedOut => ConnectionErrorIncoming::Timeout,
        error => ConnectionErrorIncoming::Undefined(Arc::new(error)),
    }
}

fn connection_lost(error: ConnectionError) -> StreamErrorIncoming {
    StreamErrorIncoming::ConnectionErrorIncoming {
        connection_error: connection_error(error),
    }
}

fn read_error(error: ReadError) -> StreamErrorIncoming {
    match error {
        ReadError::Reset(reason) => StreamErrorIncoming::StreamTerminated {
            error_code: reason.into(),
        },
        ReadError::ConnectionLost(error) => connection_lost(error),
        error => unknown(error),
    }
}

fn write_error(error: WriteError) -> StreamErrorIncoming {
    match error {
        WriteError::Stopped(reason) => StreamErrorIncoming::StreamTerminated {
            error_code: reason.into(),
        },
        WriteError::ConnectionLost(error) => connection_lost(error),
        error => unknown(error),
    }
}

#[cfg(all(test, feature = "self-signed"))]
mod tests {
    use super::*;
    use crate::{Limits, Tls, http3::endpoint, listener};
    use quinn::crypto::rustls::QuicClientConfig;
    use rustls::RootCertStore;
    use std::time::Duration;
    use tokio::time::timeout;

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn discarding_ends_however_fast_the_client_sends() {
        let tls = Tls::self_signed(["localhost"]).unwrap();
        let socket = listener::udp(([127, 0, 0, 1], 0).into()).unwrap();
        let server = endpoint(socket, &tls, &Limits::default()).unwrap();

        let mut roots = RootCertStore::empty();
        roots.add(tls.certificate()).unwrap();
        let mut config = rustls::ClientConfig::builder_with_provider(Arc::new(
            rustls::crypto::aws_lc_rs::default_provider(),
        ))
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_root_certificates(roots)
        .with_no_client_auth();
        config.alpn_protocols = vec![b"h3".to_vec()];
        let config = QuicClientConfig::try_from(config).unwrap();
        let config = quinn::ClientConfig::new(Arc::new(config));
        let client = quinn::Endpoint::client(([127, 0, 0, 1], 0).into()).unwrap();
        let connecting = client
            .connect_with(config, server.local_addr().unwrap(), "localhost")
            .unwrap();
        let (connection, accepted) =
            tokio::join!(connecting, async { server.accept().await.unwrap().await });
        let (connection, accepted) = (connection.unwrap(), accepted.unwrap());

        let (mut send, _receive) = connection.open_bi().await.unwrap();
        let sending = tokio::spawn(async move {
            while send.write_all(&[0; 100_000]).await.is_ok() {}
            send.stopped().await
        });
        let (_send, receive) = accepted.accept_bi().await.unwrap();
        let deadline = Instant::now() + Duration::from_millis(300);
        timeout(Duration::from_secs(5), discard(receive, deadline))
            .await
            .unwrap();
        assert!(Instant::now() >= deadline);
        let stopped = timeout(Duration::from_secs(5), sending).await.unwrap();
        assert_eq!(stopped.unwrap().unwrap(), Some(code(Code::H3_NO_ERROR)));
    }
}
