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
        let finish: Finish = match *request.method() {
            Method::HEAD => finish_head,
            Method::CONNECT => finish_connect,
            _ => finish,
        };
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

/// Give a response the length of its content, and no content that its status rules out (RFC 9110
/// sections 15.3.5, 15.3.6 and 15.4.5).
///
/// Of the protocol implementations, only Hyper's HTTP/1.1 does most of this. It sends a length that
/// the application declares for content of known length, which it checks only in debug builds, by
/// panicking.
fn finish(mut response: Response) -> Response {
    match response.status() {
        StatusCode::NO_CONTENT | StatusCode::NOT_MODIFIED => {
            *response.body_mut() = Body::empty();
            // Axum's is about the body that it saw, and would be 0 for an empty one.
            response.headers_mut().remove(CONTENT_LENGTH);
        }
        StatusCode::RESET_CONTENT => {
            *response.body_mut() = Body::empty();
            frame_by_length(response.headers_mut(), 0);
        }
        status if status.is_informational() => {}
        _ => match response.body().size_hint().exact() {
            Some(length) => declare_length(response.headers_mut(), length),
            None => keep_declared_length(response.headers_mut()),
        },
    }
    response
}

/// Take the content off the response to a HEAD request, and keep what is known of its length.
///
/// Axum does for its handlers, but the application can be any service.
fn finish_head(mut response: Response) -> Response {
    match response.body().size_hint().exact() {
        // Taken off already, so the application's length is that of the content to GET.
        None | Some(0) => keep_declared_length(response.headers_mut()),
        Some(length) => declare_length(response.headers_mut(), length),
    }
    *response.body_mut() = Body::empty();
    if let StatusCode::NO_CONTENT | StatusCode::NOT_MODIFIED = response.status() {
        response.headers_mut().remove(CONTENT_LENGTH);
    }
    response
}

/// A successful response to CONNECT begins a tunnel, and has neither content nor a length.
fn finish_connect(response: Response) -> Response {
    if response.status().is_success() {
        response
    } else {
        finish(response)
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
    frame_by_length(headers, length);
}

/// Keep the length that the application declared for content of unknown length, as one field, if
/// it is valid. The protocols check the content against it as it goes out.
fn keep_declared_length(headers: &mut HeaderMap) {
    match fields::declared_length(headers) {
        Ok(None) => {}
        Ok(Some(length)) => frame_by_length(headers, length),
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

/// Make a length the response's only framing.
///
/// A `Transfer-Encoding` goes, which is about an HTTP/1.1 connection: RFC 9112 section 6.1 rules out
/// both together, and HTTP/2 and HTTP/3 have none. Hyper closes an HTTP/1.1 connection without a
/// response when the application declares both.
fn frame_by_length(headers: &mut HeaderMap, length: u64) {
    headers.remove(TRANSFER_ENCODING);
    headers.insert(CONTENT_LENGTH, length.into());
}

/// An IPv4 client of a dual-stack listener has an IPv4 address, not an IPv4-mapped IPv6 one.
fn canonical(address: SocketAddr) -> SocketAddr {
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
