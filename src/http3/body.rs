//! Bodies between h3 streams and `http-body`.
//!
//! The published bridges were not enough when this was written: `h3-axum` 0.3.1 ends a response
//! normally when its body fails, so a truncated response looks complete, and `h3-util` 0.0.6
//! cancels every request whose body the handler leaves unread.

use super::{
    BoxError,
    transport::{Receiver, Sender},
};
use crate::head::declared_length;
use axum::{body::HttpBody, response::Response};
use bytes::{Buf, BufMut, Bytes, BytesMut};
use h3::{error::Code, server::RequestStream};
use http_body::{Frame, SizeHint};
use std::{
    pin::Pin,
    task::{Context, Poll, ready},
};

type ReceiveStream = RequestStream<Receiver, Bytes>;
type SendStream = RequestStream<Sender, Bytes>;

/// How much of a body to gather before the application gets it.
const PIECE: usize = 16 * 1024;

/// A request body that reads its stream only as the application asks for more. The transport takes
/// care of what is left when it is dropped.
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
            let data = ready!(self.poll_data(cx))?;
            if !data.is_empty() {
                return Poll::Ready(Some(Ok(Frame::data(data))));
            }
        }
        if self.next == Next::Trailers {
            let trailers = ready!(self.stream.poll_recv_trailers(cx))?;
            self.next = Next::End;
            return Poll::Ready(trailers.map(|trailers| Ok(Frame::trailers(trailers))));
        }
        Poll::Ready(None)
    }

    /// Gather what has arrived, which is nothing only at the end.
    ///
    /// This copies it, because small pieces keep all of what they were read with in memory. An
    /// application that holds on to a limited body could otherwise hold many times the limit.
    fn poll_data(&mut self, cx: &mut Context<'_>) -> Poll<Result<Bytes, BoxError>> {
        let mut data = BytesMut::new();
        while data.len() < PIECE {
            match self.stream.poll_recv_data(cx)? {
                Poll::Ready(Some(piece)) => {
                    if let Some(remaining) = &mut self.remaining {
                        *remaining = remaining
                            .checked_sub(piece.remaining() as u64)
                            .ok_or("the request body is longer than its Content-Length")?;
                    }
                    data.put(piece);
                }
                Poll::Ready(None) => {
                    self.next = Next::Trailers;
                    // h3 leaves this check (RFC 9114 section 4.1.2) to its caller, and without it
                    // a truncated request looks complete.
                    if self.remaining.is_some_and(|remaining| remaining > 0) {
                        let error = "the request body is shorter than its Content-Length";
                        return Poll::Ready(Err(error.into()));
                    }
                    break;
                }
                Poll::Pending if data.is_empty() => return Poll::Pending,
                Poll::Pending => break,
            }
        }
        Poll::Ready(Ok(data.freeze()))
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
            self.stream.stop_sending(Code::H3_MESSAGE_ERROR);
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
            return Err("the response is shorter than its Content-Length".into());
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
