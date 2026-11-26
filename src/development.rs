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
    let open: Arc<str> = format!("http://{address}/").into();
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
    /// It names another host than an IP address or `localhost`, or none, or several.
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
/// - **For an IP address or `localhost`.** Every authority of the request, the target's and the
///   `Host` field's, names an IP address, `localhost`, or a name below `.localhost`. Rebinding needs
///   a name that the attacker's DNS answers, and browsers answer `localhost` themselves.
/// - **Not made by other sites.** Following Fetch Metadata's resource isolation policy, a request
///   that the browser marks as `Sec-Fetch-Site: cross-site` is refused, unless it opens a page with
///   GET or HEAD, and not into `<object>` or `<embed>`. Clients that send no such field, such as
///   curl, are not affected.
/// - **Not changes from other origins.** For browsers without Fetch Metadata, a request with another
///   method than GET, HEAD, OPTIONS or TRACE, or that asks to upgrade the connection, as WebSockets
///   do, is refused when its `Origin` is not on this computer: a loopback address, `localhost`, or
///   a name below `.localhost`. Any IP address will not do here, since a page can come from a
///   server's.
fn refusal(method: &Method, uri: &Uri, headers: &HeaderMap) -> Option<Refusal> {
    let mut hosts = headers.get_all(HOST).iter();
    let host = hosts
        .next()
        .map(|host| String::from_utf8_lossy(host.as_bytes()));
    if hosts.next().is_some() {
        return Some(Refusal::Host("several".to_owned()));
    }
    let target = uri.authority().map(Authority::as_str);
    if host.is_none() && target.is_none() {
        return Some(Refusal::Host("none".to_owned()));
    }
    for authority in host.as_deref().into_iter().chain(target) {
        if fixed_host(authority).is_none() {
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
    if let Some(origin) = headers.get(ORIGIN).filter(|_| changes) {
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

/// A host that no other site's DNS can change.
#[derive(Debug, PartialEq)]
enum FixedHost {
    Address(IpAddr),
    /// `localhost`, or a name below `.localhost`, which browsers answer themselves.
    Localhost,
}

/// The host of an authority, such as `localhost:3000`, if no other site's DNS can change it: an IP
/// address, `localhost`, or a name below `.localhost`, with any port.
fn fixed_host(authority: &str) -> Option<FixedHost> {
    let authority = authority.parse::<Authority>().ok()?;
    if authority.as_str().contains('@') {
        return None;
    }
    let host = authority.host();
    let host = host.strip_suffix('.').unwrap_or(host);
    if let Some(address) = host
        .strip_prefix('[')
        .and_then(|host| host.strip_suffix(']'))
    {
        return address
            .parse::<Ipv6Addr>()
            .ok()
            .map(|address| FixedHost::Address(address.into()));
    }
    if let Ok(address) = host.parse::<Ipv4Addr>() {
        return Some(FixedHost::Address(address.into()));
    }
    let host = host.to_ascii_lowercase();
    (host == "localhost" || host.ends_with(".localhost")).then_some(FixedHost::Localhost)
}

/// Whether an authority names this computer: a loopback address, also as an IPv4-mapped one,
/// `localhost`, or a name below `.localhost`.
fn is_this_computer(authority: &str) -> bool {
    match fixed_host(authority) {
        Some(FixedHost::Address(address)) => address.to_canonical().is_loopback(),
        Some(FixedHost::Localhost) => true,
        None => false,
    }
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
                "The development address answers only requests for an IP address or localhost, \
                 which no other site can point at this computer. Open {open} instead.\n"
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
    fn only_names_that_no_other_site_controls_are_fixed() {
        for authority in [
            "127.0.0.1",
            "127.0.0.1:3000",
            "127.0.0.1.:3000",
            "[::1]:3000",
            "192.0.2.1",
            "localhost",
            "LOCALHOST:3000",
            "localhost.:3000",
            "app.localhost:3000",
            "App.LocalHost.",
        ] {
            assert!(fixed_host(authority).is_some(), "{authority}");
        }
        for authority in [
            "",
            "attacker.example",
            "attacker.example.:3000",
            "localhost.attacker.example",
            "127.0.0.1.nip.io",
            "localhostx",
            "user@localhost",
            "[::1",
            "[fe80::1%25eth0]",
            "127.1",
        ] {
            assert_eq!(fixed_host(authority), None, "{authority}");
        }
    }

    #[test]
    fn only_loopback_addresses_and_localhost_are_this_computer() {
        for authority in [
            "127.0.0.1:5173",
            "127.0.0.2",
            "[::1]:5173",
            "[::ffff:127.0.0.1]:5173",
            "localhost:5173",
            "app.localhost",
        ] {
            assert!(is_this_computer(authority), "{authority}");
        }
        for authority in [
            "192.0.2.42",
            "192.0.2.42:3000",
            "0.0.0.0:3000",
            "[2001:db8::1]",
            "[::ffff:192.0.2.42]",
            "[::]",
            "attacker.example",
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
        for (uri, fields) in [
            ("/", vec![]),
            ("/", vec![("host", "attacker.example")]),
            ("/", vec![("host", "localhost"), ("host", "localhost")]),
            ("http://attacker.example/", vec![]),
            ("http://localhost/", vec![("host", "attacker.example")]),
            ("http://attacker.example/", vec![("host", "localhost")]),
        ] {
            let refused = check(Method::GET, uri, &fields);
            assert!(
                matches!(refused, Some(Refusal::Host(_))),
                "{uri} {fields:?}"
            );
        }
    }
}
