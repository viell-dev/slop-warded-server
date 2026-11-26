//! Rules for message heads that the protocol implementations leave to the server.
//!
//! A request is checked before the application and its routes see it, so that they see the same
//! well-formed head whichever protocol carried it.

use http::{
    HeaderMap, HeaderValue, Version,
    header::{CONTENT_LENGTH, HOST},
    request::Parts,
    uri::{Authority, Scheme},
};
use std::net::Ipv6Addr;

/// Refuse a request head that the HTTP specifications call malformed and that Hyper or h3 lets
/// through, with the reason.
///
/// The target of an HTTP/1 request in absolute form replaces its `Host` field, which the
/// application then sees with the target's host (RFC 9112 section 3.2.2).
pub(crate) fn check_request(head: &mut Parts) -> Result<(), &'static str> {
    if head.headers.values().any(has_outer_whitespace) {
        return Err("a field value begins or ends with whitespace");
    }
    check_host(head)
}

/// Whether a field value begins or ends with a space or a tab.
///
/// That makes an HTTP/2 message malformed (RFC 9113 section 8.2.1), and is among the invalid
/// characters of an HTTP/3 one (RFC 9114 section 4.1.2). Hyper's HTTP/1.1 parser removes it, as
/// RFC 9112 section 5.1 says, so trimming it on the other protocols would hide a client's defect
/// only there.
fn has_outer_whitespace(value: &HeaderValue) -> bool {
    matches!(value.as_bytes(), [b' ' | b'\t', ..] | [.., b' ' | b'\t'])
}

/// Check that the request names one valid host.
///
/// - HTTP/1: one `Host` field at most, which HTTP/1.1 requires, and which is a host or empty when
///   the target does not name one (RFC 9112 section 3.2). An empty value is how a client says that
///   the target has no host.
/// - HTTP/2 and HTTP/3: `:authority` or a `Host` field, and the same host in both when the request
///   has both (RFC 9113 section 8.3.1, RFC 9114 section 4.3.1). h3 compares the first `Host` field
///   only, and Hyper none.
/// - No target with user information, which RFC 9110 section 4.2.4 says to treat as an error, and
///   which RFC 9113 and RFC 9114 rule out.
fn check_host(head: &mut Parts) -> Result<(), &'static str> {
    let mut fields = head.headers.get_all(HOST).iter();
    let field = fields.next();
    if fields.next().is_some() {
        return Err("the request has several Host fields");
    }
    let http1 = matches!(head.version, Version::HTTP_10 | Version::HTTP_11);
    let missing = if head.version == Version::HTTP_11 {
        Err("the HTTP/1.1 request has no Host field")
    } else {
        Ok(())
    };
    let Some(target) = head.uri.authority() else {
        return match field {
            Some(field) if http1 && field.is_empty() => Ok(()),
            Some(field) => host(field)
                .map(drop)
                .ok_or("the Host field is not a valid host"),
            None if http1 => missing,
            None => Err("the request names no host"),
        };
    };
    if !is_valid(target) {
        return Err("the request target names an invalid host");
    }
    match field {
        None => missing,
        Some(_) if http1 => {
            let value =
                HeaderValue::try_from(target.as_str()).expect("an authority is a valid value");
            head.headers.insert(HOST, value);
            Ok(())
        }
        Some(field) => match host(field) {
            Some(host) if is_same_host(&host, target, head.uri.scheme()) => Ok(()),
            Some(_) => Err("the Host field names another host than :authority"),
            None => Err("the Host field is not a valid host"),
        },
    }
}

/// The host in a `Host` field value, if it is one.
fn host(value: &HeaderValue) -> Option<Authority> {
    Authority::try_from(value.as_bytes()).ok().filter(is_valid)
}

/// Whether an authority is `host [ ":" port ]` (RFC 3986 section 3.2), without user information.
///
/// `Authority` checks the characters, but takes user information, any text as a port, and anything
/// in brackets.
fn is_valid(authority: &Authority) -> bool {
    let host = authority.host();
    // With user information, the authority does not begin with the host.
    let Some(port) = authority.as_str().strip_prefix(host) else {
        return false;
    };
    let port_is_valid = port.is_empty()
        || port
            .strip_prefix(':')
            .is_some_and(|digits| digits.bytes().all(|digit| digit.is_ascii_digit()));
    let literal_is_valid = host
        .strip_prefix('[')
        .is_none_or(|literal| literal.strip_suffix(']').is_some_and(is_ip_literal));
    port_is_valid && literal_is_valid
}

/// Whether the inside of brackets is an IPv6 address, or an address of a later version
/// (`IPvFuture` in RFC 3986 section 3.2.2), whose characters `Authority` has checked.
fn is_ip_literal(literal: &str) -> bool {
    let future = literal
        .strip_prefix(['v', 'V'])
        .and_then(|future| future.split_once('.'))
        .is_some_and(|(version, address)| {
            !version.is_empty()
                && version.bytes().all(|digit| digit.is_ascii_hexdigit())
                && !address.is_empty()
        });
    future || literal.parse::<Ipv6Addr>().is_ok()
}

/// Whether two valid authorities name the same host and port, in any case and with the default port
/// of the scheme left out or not (RFC 3986 section 6.2.3).
fn is_same_host(one: &Authority, other: &Authority, scheme: Option<&Scheme>) -> bool {
    let default = if scheme == Some(&Scheme::HTTPS) {
        Some("443")
    } else if scheme == Some(&Scheme::HTTP) {
        Some("80")
    } else {
        None
    };
    one.host().eq_ignore_ascii_case(other.host()) && port(one, default) == port(other, default)
}

/// The port of a valid authority without leading zeros, or the default when it names none.
///
/// Not `Authority::port_u16`, which has none for a port above 65 535 either.
fn port<'a>(authority: &'a Authority, default: Option<&'a str>) -> Option<&'a str> {
    let digits = authority.as_str()[authority.host().len()..].trim_start_matches(':');
    if digits.is_empty() {
        return default;
    }
    match digits.trim_start_matches('0') {
        "" => Some("0"),
        significant => Some(significant),
    }
}

/// The one length that the `Content-Length` fields of a head agree on.
pub(crate) fn declared_length(headers: &HeaderMap) -> Result<Option<u64>, &'static str> {
    let mut lengths = headers.get_all(CONTENT_LENGTH).iter();
    let Some(length) = lengths.next() else {
        return Ok(None);
    };
    if lengths.any(|other| other != length) {
        return Err("the head has different Content-Length fields");
    }
    let digits = length.to_str().unwrap_or_default();
    if !digits.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err("the head has an invalid Content-Length");
    }
    let length = digits.parse();
    length
        .map(Some)
        .map_err(|_| "the head has an invalid Content-Length")
}

#[cfg(test)]
mod tests {
    use super::*;
    use http::Request;

    fn head(version: Version, uri: &str, fields: &[(&'static str, &'static str)]) -> Parts {
        let mut request = Request::builder().version(version).uri(uri);
        for (name, value) in fields {
            request = request.header(*name, HeaderValue::from_static(value));
        }
        request.body(()).unwrap().into_parts().0
    }

    #[test]
    fn values_with_whitespace_around_them_are_refused() {
        for value in [" a", "a ", "\ta", "a\t", " ", "\t"] {
            let mut head = head(Version::HTTP_2, "https://localhost/", &[("x-a", value)]);
            assert!(check_request(&mut head).is_err(), "{value:?}");
        }
        for value in ["a", "a b", "a\tb", "", "\"a \""] {
            let mut head = head(Version::HTTP_2, "https://localhost/", &[("x-a", value)]);
            assert_eq!(check_request(&mut head), Ok(()), "{value:?}");
        }
    }

    #[test]
    fn http1_requests_name_one_valid_host() {
        for (version, uri, fields) in [
            (Version::HTTP_11, "/", &[("host", "localhost")][..]),
            (Version::HTTP_11, "/", &[("host", "")]),
            (Version::HTTP_11, "/", &[("host", "localhost:8080")]),
            (Version::HTTP_11, "/", &[("host", "localhost:")]),
            (Version::HTTP_11, "/", &[("host", "LOCALHOST.:8080")]),
            (Version::HTTP_11, "/", &[("host", "192.0.2.1")]),
            (Version::HTTP_11, "/", &[("host", "[::1]:8080")]),
            (Version::HTTP_11, "/", &[("host", "[::ffff:192.0.2.1]")]),
            (Version::HTTP_11, "/", &[("host", "[v1.fe:x]")]),
            (Version::HTTP_11, "/", &[("host", "a-b_c~d.example")]),
            (Version::HTTP_11, "*", &[("host", "localhost")]),
            (Version::HTTP_10, "/", &[]),
            (Version::HTTP_10, "http://localhost/", &[]),
            (Version::HTTP_11, "http://localhost/", &[("host", "other")]),
            (Version::HTTP_11, "http://localhost/", &[("host", "")]),
            (
                Version::HTTP_11,
                "example.com:443",
                &[("host", "example.com")],
            ),
        ] {
            let mut head = head(version, uri, fields);
            assert_eq!(
                check_request(&mut head),
                Ok(()),
                "{version:?} {uri} {fields:?}"
            );
        }
        for (version, uri, fields) in [
            (Version::HTTP_11, "/", &[][..]),
            (Version::HTTP_11, "/", &[("host", "a"), ("host", "b")]),
            (Version::HTTP_11, "/", &[("host", "a"), ("host", "a")]),
            (Version::HTTP_10, "/", &[("host", "a"), ("host", "b")]),
            (Version::HTTP_11, "/", &[("host", "not a host/x")]),
            (Version::HTTP_11, "/", &[("host", "user@localhost")]),
            (Version::HTTP_11, "/", &[("host", "localhost:abc")]),
            (Version::HTTP_11, "/", &[("host", "localhost:+1")]),
            (Version::HTTP_11, "/", &[("host", "localhost:1:2")]),
            (Version::HTTP_11, "/", &[("host", "[bogus]")]),
            (Version::HTTP_11, "/", &[("host", "[v.x]")]),
            (Version::HTTP_11, "/", &[("host", "[::1")]),
            (Version::HTTP_11, "/", &[("host", "::1")]),
            (Version::HTTP_11, "http://localhost/", &[]),
            (
                Version::HTTP_11,
                "http://localhost/",
                &[("host", "a"), ("host", "b")],
            ),
            (
                Version::HTTP_11,
                "http://user@localhost/",
                &[("host", "localhost")],
            ),
            (
                Version::HTTP_11,
                "http://localhost:abc/",
                &[("host", "localhost")],
            ),
        ] {
            let mut head = head(version, uri, fields);
            assert!(
                check_request(&mut head).is_err(),
                "{version:?} {uri} {fields:?}"
            );
        }
    }

    #[test]
    fn the_target_of_an_http1_request_is_its_host() {
        for (uri, fields, host) in [
            (
                "http://localhost:8080/",
                &[("host", "other")][..],
                Some("localhost:8080"),
            ),
            ("http://localhost/", &[("host", "")], Some("localhost")),
            (
                "http://localhost/",
                &[("host", "not a host/x")],
                Some("localhost"),
            ),
            ("/", &[("host", "LOCALHOST.:8080")], Some("LOCALHOST.:8080")),
        ] {
            let mut head = head(Version::HTTP_11, uri, fields);
            check_request(&mut head).unwrap();
            let hosts: Vec<_> = head.headers.get_all(HOST).iter().collect();
            assert_eq!(
                hosts,
                host.into_iter().collect::<Vec<_>>(),
                "{uri} {fields:?}"
            );
        }
        // HTTP/1.0 needs no field, and none is added.
        let mut head = head(Version::HTTP_10, "http://localhost/", &[]);
        check_request(&mut head).unwrap();
        assert!(!head.headers.contains_key(HOST));
    }

    #[test]
    fn http2_and_http3_hosts_agree_with_the_authority() {
        for version in [Version::HTTP_2, Version::HTTP_3] {
            for (uri, fields) in [
                ("https://localhost/", &[][..]),
                ("https://localhost/", &[("host", "localhost")]),
                ("https://localhost/", &[("host", "LocalHost")]),
                ("https://localhost/", &[("host", "localhost:443")]),
                ("https://localhost:443/", &[("host", "localhost")]),
                ("http://localhost/", &[("host", "localhost:80")]),
                ("https://localhost:8443/", &[("host", "localhost:8443")]),
                ("https://localhost:8443/", &[("host", "localhost:08443")]),
                ("https://localhost/", &[("host", "localhost:")]),
                ("https://[::1]/", &[("host", "[::1]:443")]),
                ("/", &[("host", "localhost")]),
            ] {
                let mut head = head(version, uri, fields);
                assert_eq!(
                    check_request(&mut head),
                    Ok(()),
                    "{version:?} {uri} {fields:?}"
                );
            }
            for (uri, fields) in [
                ("/", &[][..]),
                ("/", &[("host", "")]),
                ("/", &[("host", "not a host/x")]),
                ("https://localhost/", &[("host", "other.example")]),
                ("https://localhost/", &[("host", "localhost:8443")]),
                ("https://localhost/", &[("host", "localhost:99999")]),
                ("https://localhost/", &[("host", "localhost:0")]),
                ("http://localhost/", &[("host", "localhost:443")]),
                ("https://localhost/", &[("host", "localhost.")]),
                ("https://localhost/", &[("host", "user@localhost")]),
                ("https://localhost/", &[("host", "")]),
                (
                    "https://localhost/",
                    &[("host", "localhost"), ("host", "other.example")],
                ),
                ("https://user@localhost/", &[]),
            ] {
                let mut head = head(version, uri, fields);
                assert!(
                    check_request(&mut head).is_err(),
                    "{version:?} {uri} {fields:?}"
                );
            }
        }
    }
}
