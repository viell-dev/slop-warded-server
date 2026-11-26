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
/// - HTTP/1: one `Host` field at most, which HTTP/1.1 requires, and which is a host, or empty to say
///   that the target has none (RFC 9112 section 3.2). When the target names a host, it replaces the
///   field's (section 3.2.2), which has to be valid all the same.
/// - HTTP/2 and HTTP/3: the same host in `:authority` and a `Host` field when the request has both
///   (RFC 9113 section 8.3.1, RFC 9114 section 4.3.1). h3 compares the first `Host` field only, and
///   Hyper none. HTTP/3 requires one of them, which h3 checks too. HTTP/2 allows neither, as for an
///   HTTP/1.0 request without `Host` that an intermediary passes on.
/// - No empty host and no user information in a target, which RFC 9110 sections 4.2.1 and 4.2.4
///   rule out for `http` and `https`, and RFC 9113 and RFC 9114 for `:authority`.
fn check_host(head: &mut Parts) -> Result<(), &'static str> {
    let mut fields = head.headers.get_all(HOST).iter();
    let field = fields.next();
    if fields.next().is_some() {
        return Err("the request has several Host fields");
    }
    let field_is_valid = field
        .is_none_or(|field| (is_http1(head.version) && field.is_empty()) || host(field).is_some());
    if !field_is_valid {
        return Err("the Host field is not a valid host");
    }
    match (head.uri.authority(), field) {
        (Some(target), _) if !is_valid(target) => Err("the request target names an invalid host"),
        (_, None) if head.version == Version::HTTP_11 => {
            Err("the HTTP/1.1 request has no Host field")
        }
        (None, None) if head.version == Version::HTTP_3 => Err("the request names no host"),
        (Some(target), Some(_)) if is_http1(head.version) => {
            let value =
                HeaderValue::try_from(target.as_str()).expect("an authority is a valid value");
            head.headers.insert(HOST, value);
            Ok(())
        }
        (Some(target), Some(field)) => {
            let same =
                host(field).is_some_and(|host| is_same_host(&host, target, head.uri.scheme()));
            same.then_some(())
                .ok_or("the Host field names another host than :authority")
        }
        _ => Ok(()),
    }
}

/// Whether HTTP/1 carries a message of this version, with its own framing and `Host` rules.
pub(crate) fn is_http1(version: Version) -> bool {
    matches!(
        version,
        Version::HTTP_09 | Version::HTTP_10 | Version::HTTP_11
    )
}

/// The host in a `Host` field value, if it is one.
fn host(value: &HeaderValue) -> Option<Authority> {
    Authority::try_from(value.as_bytes()).ok().filter(is_valid)
}

/// Whether an authority is `host [ ":" port ]` (RFC 3986 section 3.2), without user information,
/// and with a host.
///
/// `Authority` checks the characters, but takes user information, any text as a port, no host,
/// and brackets anywhere.
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
    let host_is_valid = match host.strip_prefix('[') {
        Some(literal) => literal.strip_suffix(']').is_some_and(is_ip_literal),
        None => !host.is_empty() && !host.contains(['[', ']']),
    };
    port_is_valid && host_is_valid
}

/// Whether the inside of brackets is an IPv6 address, or an address of a later version
/// (`IPvFuture` in RFC 3986 section 3.2.2), whose other characters `Authority` has checked.
fn is_ip_literal(literal: &str) -> bool {
    if literal.contains(['[', ']']) {
        return false;
    }
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

    const SEVERAL: &str = "the request has several Host fields";
    const MISSING: &str = "the HTTP/1.1 request has no Host field";
    const INVALID: &str = "the Host field is not a valid host";
    const TARGET: &str = "the request target names an invalid host";
    const OTHER: &str = "the Host field names another host than :authority";
    const NONE: &str = "the request names no host";

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
            (
                Version::HTTP_11,
                "/",
                &[("host", "a-b_c~d!$&'()*+,;=.example")],
            ),
            (Version::HTTP_11, "*", &[("host", "localhost")]),
            (Version::HTTP_10, "/", &[]),
            (Version::HTTP_10, "/", &[("host", "")]),
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
            let checked = check_request(&mut head);
            assert_eq!(checked, Ok(()), "{version:?} {uri} {fields:?}");
        }
        for (version, uri, fields, reason) in [
            (Version::HTTP_11, "/", &[][..], MISSING),
            (Version::HTTP_11, "*", &[], MISSING),
            (
                Version::HTTP_11,
                "/",
                &[("host", "a"), ("host", "b")],
                SEVERAL,
            ),
            (
                Version::HTTP_11,
                "/",
                &[("host", "a"), ("host", "a")],
                SEVERAL,
            ),
            (
                Version::HTTP_10,
                "/",
                &[("host", "a"), ("host", "b")],
                SEVERAL,
            ),
            (Version::HTTP_11, "/", &[("host", "not a host/x")], INVALID),
            (Version::HTTP_10, "/", &[("host", "not a host/x")], INVALID),
            (
                Version::HTTP_11,
                "/",
                &[("host", "user@localhost")],
                INVALID,
            ),
            (Version::HTTP_11, "/", &[("host", "localhost:abc")], INVALID),
            (Version::HTTP_11, "/", &[("host", "localhost:+1")], INVALID),
            (Version::HTTP_11, "/", &[("host", "localhost:1:2")], INVALID),
            (Version::HTTP_11, "/", &[("host", ":80")], INVALID),
            (Version::HTTP_11, "/", &[("host", ":")], INVALID),
            (Version::HTTP_11, "/", &[("host", "a[b]")], INVALID),
            (Version::HTTP_11, "/", &[("host", "a[b]:80")], INVALID),
            (Version::HTTP_11, "/", &[("host", "[bogus]")], INVALID),
            (Version::HTTP_11, "/", &[("host", "[v.x]")], INVALID),
            (Version::HTTP_11, "/", &[("host", "[v1.[x]]")], INVALID),
            (Version::HTTP_11, "/", &[("host", "[::1")], INVALID),
            (Version::HTTP_11, "/", &[("host", "::1")], INVALID),
            (Version::HTTP_11, "http://localhost/", &[], MISSING),
            (
                Version::HTTP_11,
                "http://localhost/",
                &[("host", "a"), ("host", "b")],
                SEVERAL,
            ),
            // The field is replaced, but has to be valid.
            (
                Version::HTTP_11,
                "http://localhost/",
                &[("host", "not a host/x")],
                INVALID,
            ),
            (
                Version::HTTP_11,
                "http://localhost/",
                &[("host", "user@localhost")],
                INVALID,
            ),
            (
                Version::HTTP_11,
                "http://user@localhost/",
                &[("host", "localhost")],
                TARGET,
            ),
            (
                Version::HTTP_11,
                "http://localhost:abc/",
                &[("host", "localhost")],
                TARGET,
            ),
            (
                Version::HTTP_11,
                "http://:80/",
                &[("host", "localhost")],
                TARGET,
            ),
            (
                Version::HTTP_11,
                "http://a[b]/",
                &[("host", "localhost")],
                TARGET,
            ),
            (Version::HTTP_10, "http://:80/", &[], TARGET),
        ] {
            let mut head = head(version, uri, fields);
            let checked = check_request(&mut head);
            assert_eq!(checked, Err(reason), "{version:?} {uri} {fields:?}");
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
            ("/", &[("host", "LOCALHOST.:8080")], Some("LOCALHOST.:8080")),
        ] {
            let mut head = head(Version::HTTP_11, uri, fields);
            check_request(&mut head).unwrap();
            let hosts: Vec<_> = head.headers.get_all(HOST).iter().collect();
            assert_eq!(hosts, Vec::from_iter(host), "{uri} {fields:?}");
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
                let checked = check_request(&mut head);
                assert_eq!(checked, Ok(()), "{version:?} {uri} {fields:?}");
            }
            for (uri, fields, reason) in [
                ("/", &[("host", "")][..], INVALID),
                ("/", &[("host", "not a host/x")], INVALID),
                ("/", &[("host", ":443")], INVALID),
                ("https://localhost/", &[("host", "other.example")], OTHER),
                ("https://localhost/", &[("host", "localhost:8443")], OTHER),
                ("https://localhost/", &[("host", "localhost:99999")], OTHER),
                ("https://localhost/", &[("host", "localhost:0")], OTHER),
                ("http://localhost/", &[("host", "localhost:443")], OTHER),
                ("https://localhost/", &[("host", "localhost.")], OTHER),
                ("https://localhost/", &[("host", "user@localhost")], INVALID),
                ("https://localhost/", &[("host", "")], INVALID),
                (
                    "https://localhost/",
                    &[("host", "localhost"), ("host", "other.example")],
                    SEVERAL,
                ),
                ("https://user@localhost/", &[], TARGET),
                ("https://:443/", &[], TARGET),
                ("https://a[b]/", &[], TARGET),
            ] {
                let mut head = head(version, uri, fields);
                let checked = check_request(&mut head);
                assert_eq!(checked, Err(reason), "{version:?} {uri} {fields:?}");
            }
        }
        // As an HTTP/1.0 request without Host that an intermediary passes on.
        let mut relayed = head(Version::HTTP_2, "/", &[]);
        assert_eq!(check_request(&mut relayed), Ok(()));
        let mut relayed = head(Version::HTTP_3, "/", &[]);
        assert_eq!(check_request(&mut relayed), Err(NONE));
    }
}
