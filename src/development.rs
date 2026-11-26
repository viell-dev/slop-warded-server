//! The development address: the application over cleartext HTTP on loopback, with live reload.

use axum::{
    body::{Body, Bytes, HttpBody},
    extract::{Request, State},
    http::{
        HeaderMap, HeaderValue, Method, StatusCode, Uri,
        header::{ACCEPT_ENCODING, CONTENT_TYPE, HOST, ORIGIN, RANGE, UPGRADE},
        uri::Authority,
    },
    middleware::{Next, from_fn_with_state},
    response::{IntoResponse, Response},
};
use http_body::Frame;
use std::{
    future::Future,
    net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr},
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
};
use tokio_util::sync::{CancellationToken, WaitForCancellationFutureOwned};
use tower::util::MapRequestLayer;
use tower_livereload::LiveReloadLayer;

/// Where the reload script waits for the server: tower-livereload's default.
pub(crate) const EVENTS: &str = "/_tower-livereload/event-stream";

/// The application as the development address serves it.
///
/// Requests that other sites can make a browser send are refused first, before the application
/// and the reload script's event stream; see [`refusal`]. With live reload, each HTML page that a
/// browser opens gets a script that waits on an event stream for the server to go away and come
/// back, and then reloads the page. The stream ends when the server begins to shut down, so that
/// it does not hold up the shutdown.
pub(crate) fn app(
    app: axum::Router,
    address: SocketAddr,
    live_reload: bool,
    stopping: CancellationToken,
) -> axum::Router {
    let app = if live_reload {
        let reload = LiveReloadLayer::new()
            .request_predicate(is_page as fn(&Request) -> bool)
            .response_predicate(takes_the_script as fn(&Response) -> bool);
        app.layer(reload)
            .layer(from_fn_with_state(stopping, end_at_shutdown))
            .layer(MapRequestLayer::new(uncompressed))
    } else {
        app
    };
    // A development address bound as `[::ffff:127.0.0.1]` is opened as `127.0.0.1`, which reaches
    // the same listener and which the host rule accepts.
    let open: Arc<str> = format!("http://{}/", crate::context::canonical(address)).into();
    app.layer(from_fn_with_state(open, refuse_other_sites))
}

/// Whether the browser opens the response as a whole page, and does not fetch part of one, such
/// as an HTML fragment for a script or a range of bytes. Clients that do not say get the script.
fn is_page(request: &Request) -> bool {
    let document = request
        .headers()
        .get("sec-fetch-dest")
        .is_none_or(|destination| destination == "document");
    document && !request.headers().contains_key(RANGE)
}

/// Whether the script can be added to the response: complete HTML content.
///
/// tower-livereload leaves compressed responses alone in any case.
fn takes_the_script(response: &Response) -> bool {
    let html = response
        .headers()
        .get(CONTENT_TYPE)
        .is_some_and(|value| value.as_bytes().starts_with(b"text/html"));
    let complete = !matches!(
        response.status(),
        StatusCode::PARTIAL_CONTENT
            | StatusCode::NO_CONTENT
            | StatusCode::RESET_CONTENT
            | StatusCode::NOT_MODIFIED
    ) && !response.status().is_informational();
    html && complete
}

/// Ask the application for uncompressed responses, because the script cannot be added to
/// compressed ones. Compression gains nothing on loopback.
fn uncompressed(mut request: Request) -> Request {
    let identity = HeaderValue::from_static("identity");
    request.headers_mut().insert(ACCEPT_ENCODING, identity);
    request
}

/// Why the development address refuses a request.
#[derive(Debug, PartialEq)]
enum Refusal {
    /// It names another host than this computer, or none.
    Host(String),
    /// Another site's page made the browser send it.
    OtherSite(String),
}

/// Whether to refuse a request that another site could have made a browser send.
///
/// A page from anywhere can make a browser send requests to `http://127.0.0.1:3000`, and a domain
/// whose DNS answer changes to `127.0.0.1` makes them look as if they came from that page's own
/// site (DNS rebinding). The development address therefore answers only these:
///
/// - **For this computer.** Every authority of the request, the target's and the `Host` field's,
///   names a loopback address, `localhost`, or a name below `.localhost`. Rebinding needs a name
///   that the attacker's DNS answers, and browsers answer `localhost` themselves. Other addresses,
///   such as `0.0.0.0`, which also reaches this computer, are not secure contexts, so browsers send
///   no Fetch Metadata there. The listener is on loopback, so only forwarding brings them here.
/// - **Not made by other sites.** Following Fetch Metadata's resource isolation policy, a request
///   that the browser marks as `Sec-Fetch-Site: cross-site` is refused, unless it opens a page with
///   GET or HEAD, and not into `<object>` or `<embed>`. Clients that send no such field, such as
///   curl, are not affected.
/// - **Not changes from other origins.** A request with another method than GET, HEAD, OPTIONS or
///   TRACE, or that asks to upgrade the connection, as WebSockets do, is refused when its `Origin`
///   is not on this computer, as above. Any IP address will not do here, since a page can come from
///   a server's. This covers browsers without Fetch Metadata. A request that the browser marks as
///   `Sec-Fetch-Site: same-origin` is not checked: pages cannot set that field, and a page's own
///   form sends `Origin: null` under `Referrer-Policy: no-referrer`.
fn refusal(method: &Method, uri: &Uri, headers: &HeaderMap) -> Option<Refusal> {
    // `fields::check_request` has refused several Host fields already.
    let host = headers
        .get(HOST)
        .map(|host| String::from_utf8_lossy(host.as_bytes()));
    let target = uri.authority().map(Authority::as_str);
    if host.is_none() && target.is_none() {
        return Some(Refusal::Host("none".to_owned()));
    }
    for authority in host.as_deref().into_iter().chain(target) {
        if !is_this_computer(authority) {
            return Some(Refusal::Host(authority.to_owned()));
        }
    }

    let field = |name: &str| headers.get(name).and_then(|value| value.to_str().ok());
    if field("sec-fetch-site") == Some("cross-site") {
        let opens_page = field("sec-fetch-mode") == Some("navigate")
            && matches!(*method, Method::GET | Method::HEAD)
            && !matches!(field("sec-fetch-dest"), Some("object" | "embed"));
        if !opens_page {
            let site = format!(
                "{} {}",
                field("sec-fetch-mode").unwrap_or("unknown"),
                method
            );
            return Some(Refusal::OtherSite(site));
        }
    }

    let changes = !matches!(
        *method,
        Method::GET | Method::HEAD | Method::OPTIONS | Method::TRACE
    ) || headers.contains_key(UPGRADE);
    // Only the browser sets `Sec-Fetch-Site`. Another client can send it with any `Origin`, but
    // could send any `Origin` without it as well, so trusting the field gives away nothing.
    let same_origin = field("sec-fetch-site") == Some("same-origin");
    if let Some(origin) = headers.get(ORIGIN).filter(|_| changes && !same_origin) {
        let from_here = origin
            .to_str()
            .ok()
            .and_then(|origin| origin.parse::<Uri>().ok())
            .filter(|origin| matches!(origin.scheme_str(), Some("http" | "https")))
            .and_then(|origin| {
                origin
                    .authority()
                    .map(|authority| is_this_computer(authority.as_str()))
            })
            .unwrap_or(false);
        if !from_here {
            let origin = String::from_utf8_lossy(origin.as_bytes()).into_owned();
            return Some(Refusal::OtherSite(origin));
        }
    }
    None
}

/// Whether an authority, such as `localhost:3000`, names this computer, with any port: a loopback
/// address, `localhost`, or a name below `.localhost`, which browsers answer themselves. No other
/// site's DNS can change these. An IPv4-mapped loopback address, such as `[::ffff:127.0.0.1]`,
/// reaches the listener but is not a secure context, so browsers send it no Fetch Metadata, as for
/// `0.0.0.0`.
fn is_this_computer(authority: &str) -> bool {
    let Ok(authority) = authority.parse::<Authority>() else {
        return false;
    };
    if authority.as_str().contains('@') {
        return false;
    }
    let host = authority.host();
    let host = host.strip_suffix('.').unwrap_or(host);
    let address = match host
        .strip_prefix('[')
        .and_then(|host| host.strip_suffix(']'))
    {
        Some(address) => address.parse::<Ipv6Addr>().ok().map(IpAddr::from),
        None => host.parse::<Ipv4Addr>().ok().map(IpAddr::from),
    };
    if let Some(address) = address {
        return address.is_loopback();
    }
    if host.starts_with('[') {
        return false;
    }
    let host = host.to_ascii_lowercase();
    host == "localhost" || host.ends_with(".localhost")
}

async fn refuse_other_sites(
    State(open): State<Arc<str>>,
    request: Request,
    next: Next,
) -> Response {
    let Some(refusal) = refusal(request.method(), request.uri(), request.headers()) else {
        return next.run(request).await;
    };
    let reason = match refusal {
        Refusal::Host(host) => {
            tracing::warn!(%host, "the development address refused a request for another host");
            format!(
                "The development address answers only requests for this computer by a loopback \
                 address or localhost, which no other site can point here. Open {open} instead.\n"
            )
        }
        Refusal::OtherSite(site) => {
            tracing::warn!(%site, "the development address refused a request from another site");
            "The development address answers no requests that other sites make, except to open a \
             page.\n"
                .to_owned()
        }
    };
    (StatusCode::FORBIDDEN, reason).into_response()
}

async fn end_at_shutdown(
    State(stopping): State<CancellationToken>,
    request: Request,
    next: Next,
) -> Response {
    let events = request.uri().path() == EVENTS;
    let response = next.run(request).await;
    if !events {
        return response;
    }
    response.map(|body| {
        Body::new(UntilStopped {
            body,
            stopped: Box::pin(stopping.cancelled_owned()),
        })
    })
}

/// A body that ends when the server begins to shut down.
///
/// The reload script then tries again until the server is back, and reloads the page.
struct UntilStopped {
    body: Body,
    stopped: Pin<Box<WaitForCancellationFutureOwned>>,
}

impl HttpBody for UntilStopped {
    type Data = Bytes;
    type Error = axum::Error;

    fn poll_frame(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Bytes>, axum::Error>>> {
        if self.stopped.as_mut().poll(cx).is_ready() {
            return Poll::Ready(None);
        }
        Pin::new(&mut self.body).poll_frame(cx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_whole_pages_get_the_script() {
        for (field, value, expected) in [
            (None, "", true),
            (Some("sec-fetch-dest"), "document", true),
            (Some("sec-fetch-dest"), "empty", false),
            (Some("sec-fetch-dest"), "iframe", false),
            (Some("sec-fetch-dest"), "script", false),
            (Some("range"), "bytes=0-2", false),
        ] {
            let mut request = Request::new(Body::empty());
            if let Some(field) = field {
                let value = HeaderValue::from_static(value);
                request.headers_mut().insert(field, value);
            }
            assert_eq!(is_page(&request), expected, "{field:?} {value}");
        }
        for (status, expected) in [
            (StatusCode::OK, true),
            (StatusCode::NOT_FOUND, true),
            (StatusCode::PARTIAL_CONTENT, false),
            (StatusCode::NOT_MODIFIED, false),
            (StatusCode::NO_CONTENT, false),
        ] {
            let mut response = (status, "<p>").into_response();
            let html = HeaderValue::from_static("text/html; charset=utf-8");
            response.headers_mut().insert(CONTENT_TYPE, html);
            assert_eq!(takes_the_script(&response), expected, "{status}");
        }
        let text = (StatusCode::OK, "text").into_response();
        assert!(!takes_the_script(&text));
    }

    #[test]
    fn only_loopback_addresses_and_localhost_are_this_computer() {
        for authority in [
            "127.0.0.1",
            "127.0.0.1:5173",
            "127.0.0.1.:3000",
            "127.0.0.2",
            "[::1]:5173",
            "localhost",
            "LOCALHOST:3000",
            "localhost.:3000",
            "localhost:5173",
            "app.localhost",
            "App.LocalHost.",
        ] {
            assert!(is_this_computer(authority), "{authority}");
        }
        for authority in [
            "",
            "192.0.2.42",
            "192.0.2.42:3000",
            "192.168.1.20:3000",
            "0.0.0.0:3000",
            "[2001:db8::1]",
            "[::ffff:192.0.2.42]",
            "[::ffff:127.0.0.1]:5173",
            // The form Chromium writes in `Host` for that address.
            "[::ffff:7f00:1]:5173",
            "[::]",
            "[::]:3000",
            "attacker.example",
            "attacker.example.:3000",
            "localhost.attacker.example",
            "127.0.0.1.nip.io",
            "localhostx",
            "user@localhost",
            "[::1",
            "[fe80::1%25eth0]",
            "[localhost]",
            "127.1",
        ] {
            assert!(!is_this_computer(authority), "{authority}");
        }
    }

    fn check(
        method: Method,
        uri: &str,
        fields: &[(&'static str, &'static str)],
    ) -> Option<Refusal> {
        let uri: Uri = uri.parse().unwrap();
        let mut headers = HeaderMap::new();
        for (name, value) in fields {
            headers.append(*name, HeaderValue::from_static(value));
        }
        refusal(&method, &uri, &headers)
    }

    #[test]
    fn requests_from_other_sites_are_refused() {
        let host = ("host", "localhost:3000");
        let cross = ("sec-fetch-site", "cross-site");
        let navigate = ("sec-fetch-mode", "navigate");
        let same_origin = ("sec-fetch-site", "same-origin");
        for (method, fields) in [
            (Method::GET, vec![host]),
            (Method::GET, vec![host, ("sec-fetch-site", "same-origin")]),
            (Method::POST, vec![host, ("sec-fetch-site", "same-site")]),
            (Method::GET, vec![host, ("sec-fetch-site", "none")]),
            (Method::GET, vec![host, cross, navigate]),
            (
                Method::HEAD,
                vec![host, cross, navigate, ("sec-fetch-dest", "iframe")],
            ),
            (
                Method::POST,
                vec![host, ("origin", "http://127.0.0.1:5173")],
            ),
            (
                Method::POST,
                vec![host, ("origin", "http://localhost:5173")],
            ),
            // An origin is checked only where the request changes something.
            (
                Method::GET,
                vec![host, ("origin", "https://attacker.example")],
            ),
            // The site's own form under `Referrer-Policy: no-referrer`, as Chromium sends it.
            (Method::POST, vec![host, ("origin", "null"), same_origin]),
            // No browser sends this, since pages cannot set `Sec-Fetch-Site`. Other clients can
            // send any origin anyway, so the rule does not refuse it either.
            (
                Method::POST,
                vec![host, ("origin", "http://attacker.example"), same_origin],
            ),
        ] {
            assert_eq!(
                check(method.clone(), "/", &fields),
                None,
                "{method} {fields:?}"
            );
        }
        for (method, fields) in [
            (Method::GET, vec![host, cross]),
            (Method::GET, vec![host, cross, ("sec-fetch-mode", "cors")]),
            (Method::POST, vec![host, cross, navigate]),
            (
                Method::GET,
                vec![host, cross, navigate, ("sec-fetch-dest", "embed")],
            ),
            (
                Method::GET,
                vec![host, cross, navigate, ("sec-fetch-dest", "object")],
            ),
            (
                Method::POST,
                vec![host, ("origin", "https://attacker.example")],
            ),
            (Method::DELETE, vec![host, ("origin", "null")]),
            (Method::POST, vec![host, ("origin", "null")]),
            (
                Method::POST,
                vec![host, ("origin", "null"), ("sec-fetch-site", "same-site")],
            ),
            (Method::PUT, vec![host, ("origin", "file:///home")]),
            // A page can come from any server's IP address.
            (Method::POST, vec![host, ("origin", "http://192.0.2.42")]),
            (
                Method::POST,
                vec![host, ("origin", "http://[2001:db8::1]:8080")],
            ),
            (
                Method::GET,
                vec![
                    host,
                    ("upgrade", "websocket"),
                    ("origin", "http://192.0.2.42"),
                ],
            ),
            (
                Method::GET,
                vec![
                    host,
                    ("upgrade", "websocket"),
                    ("origin", "https://attacker.example"),
                ],
            ),
        ] {
            let refused = check(method.clone(), "/", &fields);
            assert!(
                matches!(refused, Some(Refusal::OtherSite(_))),
                "{method} {fields:?}: {refused:?}"
            );
        }
    }

    #[test]
    fn every_authority_must_be_this_computer() {
        assert_eq!(check(Method::GET, "http://localhost:3000/", &[]), None);
        for host in ["127.0.0.1:3000", "[::1]:3000"] {
            assert_eq!(check(Method::GET, "/", &[("host", host)]), None, "{host}");
        }
        for (uri, fields) in [
            ("/", vec![]),
            ("/", vec![("host", "attacker.example")]),
            ("/", vec![("host", "")]),
            ("http://attacker.example/", vec![]),
            // `fields::check_request` makes the field name the target's host first, so this one
            // stays as defence in depth.
            ("http://localhost/", vec![("host", "attacker.example")]),
            ("http://attacker.example/", vec![("host", "localhost")]),
            // IP addresses that are not loopback, which only forwarding could bring here.
            ("/", vec![("host", "0.0.0.0:3000")]),
            ("/", vec![("host", "[::]:3000")]),
            ("/", vec![("host", "192.168.1.20:3000")]),
            ("/", vec![("host", "[::ffff:192.0.2.42]:3000")]),
            ("/", vec![("host", "[::ffff:127.0.0.1]:3000")]),
            ("http://0.0.0.0:3000/", vec![]),
        ] {
            let refused = check(Method::GET, uri, &fields);
            assert!(
                matches!(refused, Some(Refusal::Host(_))),
                "{uri} {fields:?}"
            );
        }
    }
}
