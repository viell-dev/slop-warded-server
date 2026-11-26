//! Rules for message heads that the protocol implementations leave to the server.
//!
//! A request is checked before the application and its routes see it, so that they see the same
//! well-formed head whichever protocol carried it.

use http::{HeaderValue, request::Parts};

/// Refuse a request head that the HTTP specifications call malformed and that Hyper or h3 lets
/// through, with the reason.
pub(crate) fn check(head: &mut Parts) -> Result<(), &'static str> {
    if head.headers.values().any(has_outer_whitespace) {
        return Err("a field value begins or ends with whitespace");
    }
    Ok(())
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

#[cfg(test)]
mod tests {
    use super::*;
    use http::{Request, Version};

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
            assert!(check(&mut head).is_err(), "{value:?}");
        }
        for value in ["a", "a b", "a\tb", "", "\"a \""] {
            let mut head = head(Version::HTTP_2, "https://localhost/", &[("x-a", value)]);
            assert_eq!(check(&mut head), Ok(()), "{value:?}");
        }
    }
}
