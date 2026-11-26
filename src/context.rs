//! What every listener does to requests before the application sees them, and to its responses.

use crate::fields;
use axum::{
    body::{Body, Bytes, HttpBody},
    extract::ConnectInfo,
    http::{
        HeaderMap, HeaderValue, Method, Request, StatusCode, Version,
        header::{CONTENT_LENGTH, COOKIE, TRANSFER_ENCODING},
    },
    response::{IntoResponse, Response},
};
use futures_util::future::Either;
use std::{
    convert::Infallible,
    future::{Ready, ready},
    net::SocketAddr,
    task::{Context, Poll},
};
use tower::{Service, util::MapResponse};

/// How the connection that carried a request was secured.
#[derive(Clone, Copy, Debug)]
pub(crate) enum Security {
    /// TLS over TCP, or QUIC.
    Https,
    /// Plain TCP.
    Cleartext,
}

type Finish = fn(Response) -> Response;

type Routed = <MapResponse<axum::Router, Finish> as Service<Request<Body>>>::Future;

/// The application as seen by one connection.
#[derive(Clone)]
pub(crate) struct Connection {
    app: axum::Router,
    peer: SocketAddr,
    security: Security,
}

impl Connection {
    pub(crate) fn new(app: axum::Router, peer: SocketAddr, security: Security) -> Self {
        Self {
            app,
            peer: canonical(peer),
            security,
        }
    }
}

impl<B> Service<Request<B>> for Connection
where
    B: HttpBody<Data = Bytes> + Send + 'static,
    B::Error: Into<axum::BoxError>,
{
    type Response = Response;
    type Error = Infallible;
    /// A refusal, or the application's response.
    type Future = Either<Ready<Result<Response, Infallible>>, Routed>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Infallible>> {
        <axum::Router as Service<Request<Body>>>::poll_ready(&mut self.app, cx)
    }

    fn call(&mut self, request: Request<B>) -> Self::Future {
        let finish = finisher(request.method(), request.version());
        let (mut parts, body) = request.into_parts();
        // Before the application and its routes. The HTTP/3 bridge refuses such heads itself, as
        // stream errors.
        if parts.version != Version::HTTP_3
            && let Err(reason) = fields::check_request(&mut parts)
        {
            tracing::debug!(reason, "a malformed request was refused");
            let refusal = (
                StatusCode::BAD_REQUEST,
                format!("Malformed request: {reason}.\n"),
            );
            return Either::Left(ready(Ok(finish(refusal.into_response()))));
        }
        let mut request = Request::from_parts(parts, Body::new(body));
        let extensions = request.extensions_mut();
        extensions.insert(ConnectInfo(self.peer));
        #[cfg(feature = "warded")]
        extensions.insert(match self.security {
            Security::Https => warded::TransportSecurity::Https,
            Security::Cleartext => warded::TransportSecurity::Unsecure,
        });
        #[cfg(not(feature = "warded"))]
        let _ = self.security;
        if matches!(request.version(), Version::HTTP_2 | Version::HTTP_3) {
            join_cookies(request.headers_mut());
        }
        Either::Right(MapResponse::new(self.app.clone(), finish).call(request))
    }
}

/// How to finish the response to a request: by its method, and whether HTTP/1 carries it.
fn finisher(method: &Method, version: Version) -> Finish {
    match (method, fields::is_http1(version)) {
        (&Method::HEAD, true) => finish_head::<true>,
        (&Method::HEAD, false) => finish_head::<false>,
        (&Method::CONNECT, true) => finish_connect::<true>,
        (&Method::CONNECT, false) => finish_connect::<false>,
        (_, true) => finish::<true>,
        (_, false) => finish::<false>,
    }
}

/// Give a response the framing of its content, and no content or framing that its status rules
/// out (RFC 9110 sections 8.6, 15.2, 15.3.5, 15.3.6 and 15.4.5). An informational status becomes
/// a 500, apart from a 101 over HTTP/1. `HTTP1` says whether HTTP/1 carries it.
///
/// Of the protocol implementations, only Hyper's HTTP/1.1 does most of this. It sends a length that
/// the application declares for content of known length, which it checks only in debug builds, by
/// panicking.
fn finish<const HTTP1: bool>(mut response: Response) -> Response {
    let status = response.status();
    // The one final response cannot be informational. Hyper answers 500 for one over HTTP/1.1 and
    // closes the connection, and resets the stream over HTTP/2. A 101 is how Hyper upgrades an
    // HTTP/1.1 connection, and HTTP/2 and HTTP/3 have none.
    if status.is_informational() && !(HTTP1 && status == StatusCode::SWITCHING_PROTOCOLS) {
        tracing::error!(%status, "the application responded with an informational status");
        response = StatusCode::INTERNAL_SERVER_ERROR.into_response();
    }
    match response.status() {
        StatusCode::NO_CONTENT | StatusCode::NOT_MODIFIED => {
            *response.body_mut() = Body::empty();
            // Axum's is about the body that it saw, and would be 0 for an empty one.
            response.headers_mut().remove(CONTENT_LENGTH);
        }
        StatusCode::RESET_CONTENT => {
            *response.body_mut() = Body::empty();
            let headers = response.headers_mut();
            headers.remove(TRANSFER_ENCODING);
            headers.insert(CONTENT_LENGTH, 0.into());
        }
        status if status.is_informational() => without_content(&mut response),
        _ => {
            let length = response.body().size_hint().exact();
            frame::<HTTP1>(response.headers_mut(), length);
        }
    }
    response
}

/// Take the content off the response to a HEAD request, and keep what is known of its framing.
///
/// Axum does for its handlers, but the application can be any service.
fn finish_head<const HTTP1: bool>(mut response: Response) -> Response {
    let status = response.status();
    let no_content = matches!(
        status,
        StatusCode::NO_CONTENT | StatusCode::RESET_CONTENT | StatusCode::NOT_MODIFIED
    );
    if status.is_informational() || no_content {
        return finish::<HTTP1>(response);
    }
    let length = match response.body().size_hint().exact() {
        // Taken off already, so the application's length is that of the content to GET.
        None | Some(0) => None,
        length => length,
    };
    frame::<HTTP1>(response.headers_mut(), length);
    *response.body_mut() = Body::empty();
    response
}

/// A successful response to CONNECT begins a tunnel, and has neither content nor framing (RFC 9110
/// section 8.6, RFC 9112 section 6.1). Hyper closes the connection without a response when it has.
fn finish_connect<const HTTP1: bool>(mut response: Response) -> Response {
    if response.status().is_success() {
        without_content(&mut response);
        response
    } else {
        finish::<HTTP1>(response)
    }
}

/// Take off the content of a response that can have none, and the fields that would frame it.
///
/// Hyper sends a `Content-Length` of an informational response, and panics in debug builds when it
/// is not the length of the content.
fn without_content(response: &mut Response) {
    *response.body_mut() = Body::empty();
    let headers = response.headers_mut();
    headers.remove(CONTENT_LENGTH);
    headers.remove(TRANSFER_ENCODING);
}

/// Frame content of this length, or of an unknown one.
///
/// Over HTTP/1, a `Transfer-Encoding` of the application's is its choice of framing, which takes
/// precedence over a length (RFC 9112 section 6.3). Hyper then sends the content in chunks, with
/// its trailers and any other coding. Without the `Content-Length` that Axum's handlers add beside
/// it, Hyper does not close the connection. HTTP/2 and HTTP/3 have no `Transfer-Encoding`.
fn frame<const HTTP1: bool>(headers: &mut HeaderMap, length: Option<u64>) {
    if HTTP1 && headers.contains_key(TRANSFER_ENCODING) {
        headers.remove(CONTENT_LENGTH);
        return;
    }
    headers.remove(TRANSFER_ENCODING);
    match length {
        Some(length) => declare_length(headers, length),
        None => keep_declared_length(headers),
    }
}

/// Declare the length of the content, in place of any other that the application declared.
///
/// Hyper would send the application's, which the client then waits for or cuts the content at.
fn declare_length(headers: &mut HeaderMap, length: u64) {
    if headers.contains_key(CONTENT_LENGTH) && fields::declared_length(headers) != Ok(Some(length))
    {
        let declared: Vec<_> = headers.get_all(CONTENT_LENGTH).iter().collect();
        tracing::warn!(
            ?declared,
            length,
            "the application declared a Content-Length that its content does not have"
        );
    }
    headers.insert(CONTENT_LENGTH, length.into());
}

/// Keep the length that the application declared for content of unknown length, as one field, if
/// it is valid. The protocols check the content against it as it goes out.
fn keep_declared_length(headers: &mut HeaderMap) {
    match fields::declared_length(headers) {
        Ok(None) => {}
        Ok(Some(length)) => {
            headers.insert(CONTENT_LENGTH, length.into());
        }
        Err(_) => {
            let declared: Vec<_> = headers.get_all(CONTENT_LENGTH).iter().collect();
            tracing::warn!(
                ?declared,
                "the application declared an invalid Content-Length"
            );
            headers.remove(CONTENT_LENGTH);
        }
    }
}

/// An IPv4-mapped address in its IPv4 form, so that an IPv4 client of a dual-stack listener has an
/// IPv4 address.
pub(crate) fn canonical(address: SocketAddr) -> SocketAddr {
    SocketAddr::new(address.ip().to_canonical(), address.port())
}

/// Give the application one `Cookie` field, as HTTP/1.1 does.
///
/// HTTP/2 and HTTP/3 clients may split the field to compress it better. RFC 9113 section 8.2.3
/// and RFC 9114 section 4.2.1 require joining it before an application sees it; Hyper and h3
/// leave that to the server.
fn join_cookies(headers: &mut HeaderMap) {
    let mut fields = headers.get_all(COOKIE).iter();
    let (Some(first), Some(_)) = (fields.next(), fields.next()) else {
        return;
    };
    let mut joined = first.as_bytes().to_vec();
    for field in headers.get_all(COOKIE).iter().skip(1) {
        joined.extend_from_slice(b"; ");
        joined.extend_from_slice(field.as_bytes());
    }
    let joined =
        HeaderValue::from_bytes(&joined).expect("joined valid field values are a valid value");
    headers.insert(COOKIE, joined);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_cookies_are_joined_in_order() {
        let mut headers = HeaderMap::new();
        for cookie in ["a=1", "b=2; c=3", "d=4"] {
            headers.append(COOKIE, HeaderValue::from_static(cookie));
        }
        join_cookies(&mut headers);
        let cookies: Vec<_> = headers.get_all(COOKIE).iter().collect();
        assert_eq!(cookies, ["a=1; b=2; c=3; d=4"]);
    }

    #[test]
    fn mapped_addresses_become_ipv4() {
        let mapped = "[::ffff:192.0.2.1]:443".parse().unwrap();
        assert_eq!(canonical(mapped), "192.0.2.1:443".parse().unwrap());
        let native = "[2001:db8::1]:443".parse().unwrap();
        assert_eq!(canonical(native), native);
    }
}
