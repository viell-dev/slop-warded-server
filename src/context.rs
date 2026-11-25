//! What every listener does to requests before the application sees them, and to its responses.

use axum::{
    body::{Body, Bytes, HttpBody},
    extract::ConnectInfo,
    http::{
        HeaderMap, HeaderValue, Method, Request, StatusCode, Version,
        header::{CONTENT_LENGTH, COOKIE, TRANSFER_ENCODING},
    },
    response::Response,
};
use std::{
    convert::Infallible,
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
    type Future = <MapResponse<axum::Router, Finish> as Service<Request<Body>>>::Future;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Infallible>> {
        <axum::Router as Service<Request<Body>>>::poll_ready(&mut self.app, cx)
    }

    fn call(&mut self, request: Request<B>) -> Self::Future {
        let mut request = request.map(Body::new);
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
        let finish: Finish = if request.method() == Method::HEAD {
            finish_head
        } else {
            finish
        };
        MapResponse::new(self.app.clone(), finish).call(request)
    }
}

/// Give a response the length of its content, and no content that its status rules out (RFC 9110
/// sections 15.3.5 and 15.4.5).
///
/// Of the protocol implementations, only Hyper's HTTP/1.1 does all of this.
fn finish(mut response: Response) -> Response {
    match response.status() {
        StatusCode::NO_CONTENT | StatusCode::NOT_MODIFIED => {
            *response.body_mut() = Body::empty();
            // Axum's is about the body that it saw, and would be 0 for an empty one.
            response.headers_mut().remove(CONTENT_LENGTH);
        }
        status if status.is_informational() => {}
        _ => {
            let exact = response.body().size_hint().exact();
            let headers = response.headers_mut();
            if let Some(length) = exact
                && !headers.contains_key(CONTENT_LENGTH)
                && !headers.contains_key(TRANSFER_ENCODING)
            {
                headers.insert(CONTENT_LENGTH, length.into());
            }
        }
    }
    response
}

/// Also take the content off the response to a HEAD request.
///
/// Axum does for its handlers, but the application can be any service.
fn finish_head(response: Response) -> Response {
    let mut response = finish(response);
    *response.body_mut() = Body::empty();
    response
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
