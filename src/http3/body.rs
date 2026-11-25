//! Bodies between h3 streams and `http-body`.
//!
//! The published bridges were not enough when this was written: `h3-axum` 0.3.1 ends a response
//! normally when its body fails, so a truncated response looks complete, and `h3-util` 0.0.6
//! cancels every request whose body the handler leaves unread.

use super::{BoxError, head::declared_length};
use axum::{body::HttpBody, response::Response};
use bytes::{Buf, Bytes};
use h3::{error::Code, server::RequestStream};
use http_body::{Frame, SizeHint};
use std::{
    panic::{AssertUnwindSafe, catch_unwind},
    pin::Pin,
    task::{Context, Poll, Waker, ready},
};

type ReceiveStream = RequestStream<h3_quinn::RecvStream, Bytes>;
type SendStream = RequestStream<h3_quinn::SendStream<Bytes>, Bytes>;

/// A request body that reads its stream only as the application asks for more.
pub(super) struct RequestBody {
    stream: ReceiveStream,
    next: Next,
    /// What the declared `Content-Length` leaves unread.
    remaining: Option<u64>,
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum Next {
    Data,
    Trailers,
    End,
}

impl RequestBody {
    pub(super) fn new(stream: ReceiveStream, length: Option<u64>) -> Self {
        Self {
            stream,
            next: Next::Data,
            remaining: length,
        }
    }

    fn poll(&mut self, cx: &mut Context<'_>) -> Poll<Option<Result<Frame<Bytes>, BoxError>>> {
        if self.next == Next::Data {
            if let Some(mut data) = ready!(self.stream.poll_recv_data(cx))? {
                let data = data.copy_to_bytes(data.remaining());
                if let Some(remaining) = &mut self.remaining {
                    *remaining = remaining
                        .checked_sub(data.len() as u64)
                        .ok_or("the request body is longer than its Content-Length")?;
                }
                return Poll::Ready(Some(Ok(Frame::data(data))));
            }
            self.next = Next::Trailers;
            // h3 leaves this check (RFC 9114 section 4.1.2) to its caller, and without it a
            // truncated request looks complete.
            if self.remaining.is_some_and(|remaining| remaining > 0) {
                Err("the request body is shorter than its Content-Length")?;
            }
        }
        if self.next == Next::Trailers {
            // A stream that ends inside a DATA frame makes h3 0.0.8 panic here
            // (https://github.com/hyperium/h3/issues/386). Any client can send one, so treat it
            // as the malformed body that it is. Remove this with the fixed h3 release.
            let trailers = catch_unwind(AssertUnwindSafe(|| self.stream.poll_recv_trailers(cx)))
                .map_err(|_| "the request body ends inside a DATA frame")?;
            let trailers = ready!(trailers)?;
            self.next = Next::End;
            return Poll::Ready(trailers.map(|trailers| Ok(Frame::trailers(trailers))));
        }
        Poll::Ready(None)
    }
}

impl HttpBody for RequestBody {
    type Data = Bytes;
    type Error = BoxError;

    fn poll_frame(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Bytes>, BoxError>>> {
        let frame = ready!(self.poll(cx));
        if matches!(frame, Some(Err(_))) {
            self.next = Next::End;
        }
        Poll::Ready(frame)
    }

    fn is_end_stream(&self) -> bool {
        self.next == Next::End
    }

    fn size_hint(&self) -> SizeHint {
        self.remaining
            .map_or_else(SizeHint::new, SizeHint::with_exact)
    }
}

impl Drop for RequestBody {
    /// Notice the end of a body that the application did not read, usually an empty one.
    ///
    /// Quinn otherwise asks the client to stop sending it. It does so with a code that HTTP/3
    /// does not define, which clients must take for `H3_NO_ERROR` (RFC 9114 section 8). That
    /// code cannot be sent itself: h3-quinn 0.0.10 panics when asked to stop a stream with a read
    /// in progress (https://github.com/hyperium/h3/issues/361), and does not say if there is one.
    fn drop(&mut self) {
        if self.next == Next::Data && !std::thread::panicking() {
            let mut cx = Context::from_waker(Waker::noop());
            let _ = self.stream.poll_recv_data(&mut cx);
        }
    }
}

/// Sends one response.
///
/// Unless the response is finished, dropping this resets the stream. Quinn would end it normally,
/// and the client would take a truncated response for a complete one.
pub(super) struct Responder {
    stream: SendStream,
    finished: bool,
}

impl Responder {
    pub(super) fn new(stream: SendStream) -> Self {
        Self {
            stream,
            finished: false,
        }
    }

    /// Refuse the request without a response.
    pub(super) fn reset(mut self, code: Code) {
        self.stream.stop_stream(code);
        self.finished = true;
    }

    /// Send a response, whose content has the length that it declares unless it is to `HEAD`.
    pub(super) async fn send(mut self, response: Response, to_head: bool) -> Result<(), BoxError> {
        let (parts, body) = response.into_parts();
        let declared = declared_length(&parts.headers)?;
        let mut remaining = declared.filter(|_| !to_head);
        let head = http::Response::from_parts(parts, ());
        self.stream.send_response(head).await?;
        let mut body = std::pin::pin!(body);
        while let Some(frame) = std::future::poll_fn(|cx| body.as_mut().poll_frame(cx)).await {
            match frame?.into_data() {
                Ok(data) => {
                    if let Some(remaining) = &mut remaining {
                        *remaining = remaining
                            .checked_sub(data.len() as u64)
                            .ok_or("the response is longer than its Content-Length")?;
                    }
                    // Some clients read an empty DATA frame as the end.
                    if !data.is_empty() {
                        self.stream.send_data(data).await?;
                    }
                }
                Err(frame) => {
                    if let Ok(trailers) = frame.into_trailers() {
                        self.stream.send_trailers(trailers).await?;
                    }
                }
            }
        }
        if remaining.is_some_and(|remaining| remaining > 0) {
            Err("the response is shorter than its Content-Length")?;
        }
        self.stream.finish().await?;
        self.finished = true;
        Ok(())
    }
}

impl Drop for Responder {
    fn drop(&mut self) {
        if !self.finished {
            self.stream.stop_stream(Code::H3_INTERNAL_ERROR);
        }
    }
}
