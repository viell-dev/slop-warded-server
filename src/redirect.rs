//! The cleartext listener's answer when it only sends clients to HTTPS.

use axum::{
    extract::Request,
    http::{HeaderValue, StatusCode, header::HOST, header::LOCATION, uri::Authority},
    response::{IntoResponse, Response},
};

/// Redirect every request to the same host and target over HTTPS.
pub(crate) fn to_https(port: u16) -> axum::Router {
    axum::Router::new().fallback(move |request: Request| async move {
        match location(&request, port) {
            // 308 keeps the method and body, unlike 301.
            Some(location) => {
                (StatusCode::PERMANENT_REDIRECT, [(LOCATION, location)]).into_response()
            }
            None => bad_request(),
        }
    })
}

fn bad_request() -> Response {
    (
        StatusCode::BAD_REQUEST,
        "The request names no valid host.\n",
    )
        .into_response()
}

fn location(request: &Request, port: u16) -> Option<HeaderValue> {
    let authority = match request.uri().authority() {
        Some(authority) => authority.clone(),
        None => {
            let mut fields = request.headers().get_all(HOST).iter();
            match (fields.next(), fields.next()) {
                (Some(field), None) => Authority::try_from(field.as_bytes()).ok()?,
                _ => return None,
            }
        }
    };
    if authority.as_str().contains('@') || authority.host().is_empty() {
        return None;
    }
    let target = request
        .uri()
        .path_and_query()
        .map(|target| target.as_str())
        // `OPTIONS *` has no path to keep.
        .filter(|target| target.starts_with('/'))
        .unwrap_or("/");
    let host = authority.host();
    let location = if port == 443 {
        format!("https://{host}{target}")
    } else {
        format!("https://{host}:{port}{target}")
    };
    location.try_into().ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;

    fn locate(uri: &str, hosts: &[&str], port: u16) -> Option<String> {
        let mut request = Request::builder().uri(uri);
        for host in hosts {
            request = request.header(HOST, *host);
        }
        let request = request.body(Body::empty()).unwrap();
        location(&request, port).map(|value| value.to_str().unwrap().to_owned())
    }

    #[test]
    fn keeps_host_and_target_and_replaces_the_port() {
        for (uri, hosts, port, expected) in [
            (
                "/a?b=c",
                &["example.com:8080"][..],
                8443,
                "https://example.com:8443/a?b=c",
            ),
            ("/", &["example.com"], 443, "https://example.com/"),
            ("/", &["[::1]:80"], 8443, "https://[::1]:8443/"),
            ("http://example.com:80/x", &[], 443, "https://example.com/x"),
            // The request target takes precedence over Host.
            (
                "http://example.com/x",
                &["other.example"],
                443,
                "https://example.com/x",
            ),
            ("*", &["example.com"], 443, "https://example.com/"),
        ] {
            assert_eq!(locate(uri, hosts, port).as_deref(), Some(expected), "{uri}");
        }
    }

    #[test]
    fn refuses_a_missing_repeated_or_invalid_host() {
        for hosts in [
            &[][..],
            &["a.example", "b.example"],
            &[""],
            &["a b"],
            &["user@example.com"],
        ] {
            assert_eq!(locate("/", hosts, 443), None, "{hosts:?}");
        }
    }
}
