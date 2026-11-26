//! The development address, over real connections.

mod common;

use axum::{
    Extension, Json,
    body::Body,
    handler::HandlerWithoutStateExt,
    http::{HeaderValue, Request, StatusCode, Version, header},
    response::Html,
};
use common::{Authority, Client, Running, VERSIONS, chunk, get, soon, text};
use http_body_util::BodyExt;
use std::{
    net::{Ipv4Addr, SocketAddr, TcpListener},
    time::{Duration, Instant},
};
use warded::{
    Matcher, Router, TransportSecurity,
    matchers::{exact_path, path, secured, unsecure},
};
use warded_server::{DefaultMiddleware, Error, StaticFiles, app};

const PAGE: &str = "<!doctype html><title>A page</title><p>Hello</p>";
const EVENTS: &str = "/_tower-livereload/event-stream";
const ANY_PORT: SocketAddr = SocketAddr::new(std::net::IpAddr::V4(Ipv4Addr::LOCALHOST), 0);

/// A page, data, and the connection's security, behind the default middleware that compresses.
fn site() -> axum::Router {
    let describe = |version: Version, Extension(security): Extension<TransportSecurity>| async move {
        format!("{version:?} {security:?}")
    };
    let routes = Router::new()
        .route(exact_path("/"), (|| async { Html(PAGE) }).into_service())
        .route(
            exact_path("/data"),
            (|| async { Json([PAGE; 4]) }).into_service(),
        )
        .route(exact_path("/describe"), describe.into_service())
        .route(
            exact_path("/secured").and(secured()),
            (|| async { "secured" }).into_service(),
        )
        .route(
            exact_path("/secured").and(unsecure()),
            (|| async { "unsecure" }).into_service(),
        );
    app(routes).layer(DefaultMiddleware::new())
}

fn development(authority: &Authority) -> warded_server::Server {
    common::server(authority).development(ANY_PORT)
}

/// A request as a browser sends it when it opens a page.
fn page(path: &str) -> Request<axum::body::Body> {
    let mut request = get(path);
    let headers = request.headers_mut();
    headers.insert(
        header::ACCEPT_ENCODING,
        HeaderValue::from_static("gzip, br"),
    );
    headers.insert("sec-fetch-dest", HeaderValue::from_static("document"));
    request
}

#[tokio::test]
async fn the_development_address_serves_plain_http1() {
    let authority = Authority::new();
    let running = Running::start(development(&authority), site());
    let address = running.development.unwrap();
    assert!(address.ip().is_loopback());
    let mut client = Client::plain(address).await.unwrap();
    for (path, expected) in [("/describe", "HTTP/1.1 Unsecure"), ("/secured", "unsecure")] {
        let response = client.get(path).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK, "{path}");
        assert!(!response.headers().contains_key(header::ALT_SVC), "{path}");
        assert_eq!(text(response).await.unwrap(), expected);
    }

    // HTTPS goes on as before.
    for version in VERSIONS {
        let mut client = Client::connect(version, running.https, &authority)
            .await
            .unwrap();
        let response = client.get("/secured").await.unwrap();
        assert_eq!(text(response).await.unwrap(), "secured", "{version:?}");
    }
}

#[tokio::test]
async fn the_development_address_is_apart_from_the_cleartext_listener() {
    let authority = Authority::new();
    for server in [
        development(&authority).serve_http(ANY_PORT),
        development(&authority).redirect_http(ANY_PORT),
    ] {
        let running = Running::start(server, site());
        let plain = running.http.unwrap();
        let address = running.development.unwrap();
        assert_ne!(plain, address);
        let response = Client::plain(address)
            .await
            .unwrap()
            .get("/secured")
            .await
            .unwrap();
        assert_eq!(text(response).await.unwrap(), "unsecure");
    }
}

#[tokio::test]
async fn the_development_address_must_be_loopback() {
    let authority = Authority::new();
    for address in ["0.0.0.0:0", "[::]:0", "192.0.2.1:3000"] {
        let address = address.parse().unwrap();
        let result = development(&authority).development(address).bind();
        let Err(error @ Error::Development { address: refused }) = result else {
            panic!("{address} was not refused");
        };
        assert_eq!(refused, address);
        assert_eq!(
            error.to_string(),
            format!(
                "the development address {address} must be a loopback address, such as \
                 127.0.0.1:3000, because it has no TLS"
            )
        );
    }
    let mapped = "[::ffff:127.0.0.1]:0".parse().unwrap();
    assert!(development(&authority).development(mapped).bind().is_ok());
}

#[tokio::test]
async fn a_taken_development_address_is_reported() {
    let authority = Authority::new();
    let taken = TcpListener::bind(ANY_PORT).unwrap();
    let result = development(&authority)
        .development(taken.local_addr().unwrap())
        .bind();
    assert!(
        matches!(
            result,
            Err(Error::Bind {
                listener: "development over HTTP",
                ..
            })
        ),
        "{result:?}"
    );
}

#[tokio::test]
async fn pages_get_the_reload_script_on_the_development_address_only() {
    let authority = Authority::new();
    let running = Running::start(development(&authority), site());
    let mut client = Client::plain(running.development.unwrap()).await.unwrap();

    let response = client.send(page("/")).await.unwrap();
    // Uncompressed, although the browser accepts compression, so that the script can be added.
    assert!(!response.headers().contains_key(header::CONTENT_ENCODING));
    let html = text(response).await.unwrap();
    assert!(html.starts_with(PAGE), "{html}");
    assert!(html.contains(EVENTS), "{html}");

    // Not for what is not a page.
    let mut fragment = page("/");
    let fetch = HeaderValue::from_static("empty");
    fragment.headers_mut().insert("sec-fetch-dest", fetch);
    for request in [fragment, page("/data")] {
        let path = request.uri().path().to_owned();
        let body = text(client.send(request).await.unwrap()).await.unwrap();
        assert!(!body.contains(EVENTS), "{path}: {body}");
    }

    // HTTPS compresses as before, and has no script.
    for version in VERSIONS {
        let mut client = Client::connect(version, running.https, &authority)
            .await
            .unwrap();
        let response = client.send(page("/")).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()[header::CONTENT_ENCODING], "br");
        soon(response.into_body().collect()).await.unwrap();
        let mut request = page("/");
        request.headers_mut().remove(header::ACCEPT_ENCODING);
        let html = text(client.send(request).await.unwrap()).await.unwrap();
        assert_eq!(html, PAGE, "{version:?}");
        let events = client.get(EVENTS).await.unwrap();
        assert_eq!(events.status(), StatusCode::NOT_FOUND, "{version:?}");
    }
}

#[tokio::test]
async fn the_reload_stream_ends_when_the_server_stops() {
    let authority = Authority::new();
    // A stream that went on would hold up the shutdown for this long.
    let server = development(&authority).grace_period(Duration::from_secs(60));
    let mut running = Running::start(server, site());
    let mut client = Client::plain(running.development.unwrap()).await.unwrap();
    let response = client.get(EVENTS).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers()[header::CONTENT_TYPE],
        "text/event-stream"
    );
    let mut body = response.into_body();
    let first = chunk(&mut body).await.unwrap();
    assert!(first.starts_with(b"event: init"), "{first:?}");

    let stopping = Instant::now();
    running.stop();
    // The rest of the stream is its end.
    let rest = soon(body.collect()).await.unwrap();
    assert!(rest.to_bytes().is_empty());
    running.ended().await.unwrap();
    assert!(stopping.elapsed() < Duration::from_secs(10));
}

#[tokio::test]
async fn live_reload_can_be_turned_off() {
    let authority = Authority::new();
    let server = development(&authority).live_reload(false);
    let running = Running::start(server, site());
    let mut client = Client::plain(running.development.unwrap()).await.unwrap();
    let response = client.send(page("/")).await.unwrap();
    assert_eq!(response.headers()[header::CONTENT_ENCODING], "br");
    soon(response.into_body().collect()).await.unwrap();
    let events = client.get(EVENTS).await.unwrap();
    assert_eq!(events.status(), StatusCode::NOT_FOUND);
}

/// A request for this URI, which names the host, with these fields.
fn to(method: &str, uri: &str, fields: &[(&'static str, &'static str)]) -> Request<Body> {
    let mut request = Request::builder()
        .method(method)
        .uri(uri)
        .body(Body::empty())
        .unwrap();
    for (name, value) in fields {
        let value = HeaderValue::from_static(value);
        request.headers_mut().append(*name, value);
    }
    request
}

const REFUSED: &str = "The development address answers ";

#[tokio::test]
async fn other_sites_are_refused_on_the_development_address() {
    let authority = Authority::new();
    let cross = ("sec-fetch-site", "cross-site");
    let navigate = ("sec-fetch-mode", "navigate");
    for live_reload in [true, false] {
        let server = development(&authority).live_reload(live_reload);
        let running = Running::start(server, site());
        let address = running.development.unwrap();
        let port = address.port();
        let here = format!("http://localhost:{port}/");
        for http2 in [false, true] {
            let case = format!("live reload {live_reload}, HTTP/2 {http2}");
            let mut client = if http2 {
                Client::plain_http2(address).await
            } else {
                Client::plain(address).await
            }
            .unwrap();
            let mut refused = vec![
                to("GET", "http://attacker.example/", &[]),
                to("GET", &format!("http://attacker.example.:{port}/"), &[]),
                to(
                    "GET",
                    &format!("http://localhost.attacker.example:{port}/"),
                    &[],
                ),
                to("GET", &format!("http://127.0.0.1.nip.io:{port}/"), &[]),
                to(
                    "GET",
                    &format!("http://attacker.example:{port}{EVENTS}"),
                    &[],
                ),
                to("GET", &here, &[cross, ("sec-fetch-mode", "cors")]),
                to(
                    "GET",
                    &here,
                    &[cross, navigate, ("sec-fetch-dest", "embed")],
                ),
                to("POST", &here, &[cross, navigate]),
                to("POST", &here, &[("origin", "https://attacker.example")]),
                to("DELETE", &here, &[("origin", "null")]),
                // A page can come from any server's IP address.
                to("POST", &here, &[("origin", "http://192.0.2.42")]),
                to("POST", &here, &[("origin", "http://[2001:db8::1]:8080")]),
            ];
            if !http2 {
                for origin in ["https://attacker.example", "http://192.0.2.42"] {
                    refused.push(to(
                        "GET",
                        &here,
                        &[
                            ("connection", "upgrade"),
                            ("upgrade", "websocket"),
                            ("origin", origin),
                        ],
                    ));
                }
            }
            for request in refused {
                let what = format!("{case}: {} {:?}", request.uri(), request.headers());
                let response = client.send(request).await.unwrap();
                assert_eq!(response.status(), StatusCode::FORBIDDEN, "{what}");
                assert!(text(response).await.unwrap().starts_with(REFUSED), "{what}");
            }
            for request in [
                to("GET", &format!("http://127.0.0.1:{port}/"), &[]),
                to("GET", &format!("http://[::1]:{port}/"), &[]),
                to("GET", &here, &[]),
                to("GET", &format!("http://App.LocalHost:{port}/"), &[]),
                to(
                    "GET",
                    &here,
                    &[cross, navigate, ("sec-fetch-dest", "document")],
                ),
                to("GET", &here, &[("origin", "https://attacker.example")]),
                to("POST", &here, &[("origin", "http://127.0.0.1:5173")]),
                to("POST", &here, &[("sec-fetch-site", "same-origin")]),
            ] {
                let what = format!("{case}: {} {:?}", request.uri(), request.headers());
                let response = client.send(request).await.unwrap();
                assert_eq!(response.status(), StatusCode::OK, "{what}");
                assert!(text(response).await.unwrap().starts_with(PAGE), "{what}");
            }
        }
        // A request that names no host at all.
        let response = common::cleartext(address, "GET / HTTP/1.0\r\n\r\n").await;
        assert!(response.contains(" 403 Forbidden\r\n"), "{response}");
        assert!(
            response.contains(&format!("Open http://{address}/")),
            "{response}"
        );
    }
}

#[tokio::test]
async fn ranges_of_pages_are_left_as_they_are() {
    let authority = Authority::new();
    let directory = tempfile::tempdir().unwrap();
    std::fs::write(directory.path().join("page.html"), "<p>Hi").unwrap();
    let files = StaticFiles::new("/files", directory.path());
    let routes = Router::new().route(path("/files/{*file}").unwrap(), files);
    let running = Running::start(development(&authority), app(routes));
    let mut client = Client::plain(running.development.unwrap()).await.unwrap();

    let mut request = page("/files/page.html");
    let range = HeaderValue::from_static("bytes=0-2");
    request.headers_mut().insert(header::RANGE, range);
    let response = client.send(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::PARTIAL_CONTENT);
    assert_eq!(response.headers()[header::CONTENT_RANGE], "bytes 0-2/5");
    assert_eq!(response.headers()[header::CONTENT_LENGTH], "3");
    assert_eq!(text(response).await.unwrap(), "<p>");

    // The whole page has the script, and the length that it has with it.
    let response = client.send(page("/files/page.html")).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let length: usize = response.headers()[header::CONTENT_LENGTH]
        .to_str()
        .unwrap()
        .parse()
        .unwrap();
    let html = text(response).await.unwrap();
    assert!(html.starts_with("<p>Hi") && html.contains(EVENTS), "{html}");
    assert_eq!(html.len(), length);
}

#[tokio::test]
async fn malformed_hosts_are_refused_before_the_development_address_checks_them() {
    let authority = Authority::new();
    let running = Running::start(development(&authority), site());
    let address = running.development.unwrap();
    for (fields, refusal) in [
        ("", "the HTTP/1.1 request has no Host field"),
        (
            "Host: localhost\r\nHost: localhost\r\n",
            "the request has several Host fields",
        ),
    ] {
        let request = format!("GET / HTTP/1.1\r\n{fields}Connection: close\r\n\r\n");
        let response = common::cleartext(address, &request).await;
        assert!(response.contains(" 400 Bad Request\r\n"), "{response}");
        let refusal = format!("\r\n\r\nMalformed request: {refusal}.\n");
        assert!(response.ends_with(&refusal), "{response}");
    }
    // HTTP/2 without a host is well-formed, but not for this computer. `raw_http2` decodes only
    // the indexed 200 and 400, and the 403 comes as a literal.
    let (status, content) = common::raw_http2(address, &common::HOSTLESS_GET).await;
    assert_eq!(status, None);
    let content = String::from_utf8(content).unwrap();
    assert!(content.starts_with(REFUSED), "{content}");
}
