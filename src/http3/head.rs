//! What Hyper does to the heads of HTTP/2 messages and h3 leaves to the server.

use axum::{body::Body, response::Response};
use http::{
    HeaderMap, HeaderName, HeaderValue, StatusCode,
    header::{CONNECTION, DATE, TE, TRANSFER_ENCODING, UPGRADE},
};
use std::time::SystemTime;

/// Fields about an HTTP/1.1 connection, which make an HTTP/3 message malformed (RFC 9114 section
/// 4.2).
const CONNECTION_FIELDS: [HeaderName; 5] = [
    CONNECTION,
    TRANSFER_ENCODING,
    UPGRADE,
    HeaderName::from_static("keep-alive"),
    HeaderName::from_static("proxy-connection"),
];

/// The length that a request declares for its body.
///
/// Fails when the head is malformed. An application could otherwise forward it as an HTTP/1.1
/// message that means something else.
pub(super) fn content_length(headers: &HeaderMap) -> Result<Option<u64>, &'static str> {
    if CONNECTION_FIELDS
        .iter()
        .any(|name| headers.contains_key(name))
    {
        return Err("the request head has a field about an HTTP/1.1 connection");
    }
    if headers.get_all(TE).iter().any(|value| value != "trailers") {
        return Err("the request head has a TE field other than `trailers`");
    }
    crate::fields::declared_length(headers)
}

/// Make the application's response one that HTTP/3 can carry.
pub(super) fn prepare(mut response: Response) -> Response {
    let status = response.status();
    // The one final response cannot be informational, and 101 does not exist in HTTP/3.
    if status.is_informational() {
        tracing::error!(%status, "the application responded with an informational status");
        response = Response::new(Body::empty());
        *response.status_mut() = StatusCode::INTERNAL_SERVER_ERROR;
    }
    let headers = response.headers_mut();
    // Middleware written for HTTP/1.1 may add them.
    for name in CONNECTION_FIELDS {
        headers.remove(name);
    }
    headers.entry(DATE).or_insert_with(|| {
        HeaderValue::try_from(httpdate::fmt_http_date(SystemTime::now()))
            .expect("a formatted date is a valid value")
    });
    response
}

#[cfg(test)]
mod tests {
    use super::*;

    fn headers(fields: &[(&'static str, &'static str)]) -> HeaderMap {
        let mut headers = HeaderMap::new();
        for (name, value) in fields {
            headers.append(*name, HeaderValue::from_static(value));
        }
        headers
    }

    #[test]
    fn content_length_is_read_from_well_formed_heads() {
        for (fields, expected) in [
            (&[][..], None),
            (&[("te", "trailers")], None),
            (&[("content-length", "0")], Some(0)),
            (&[("content-length", "12")], Some(12)),
            (
                &[("content-length", "12"), ("content-length", "12")],
                Some(12),
            ),
        ] {
            assert_eq!(content_length(&headers(fields)), Ok(expected), "{fields:?}");
        }
    }

    #[test]
    fn malformed_heads_are_refused() {
        for fields in [
            &[("connection", "close")][..],
            &[("transfer-encoding", "chunked")],
            &[("upgrade", "websocket")],
            &[("keep-alive", "timeout=5")],
            &[("proxy-connection", "keep-alive")],
            &[("te", "gzip")],
            &[("te", "trailers"), ("te", "gzip")],
            &[("content-length", "")],
            &[("content-length", "+1")],
            &[("content-length", "1, 1")],
            &[("content-length", "99999999999999999999")],
            &[("content-length", "1"), ("content-length", "2")],
        ] {
            assert!(content_length(&headers(fields)).is_err(), "{fields:?}");
        }
    }
}
