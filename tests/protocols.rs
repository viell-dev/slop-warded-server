//! The same application over real HTTP/1.1, HTTP/2 and HTTP/3 connections.

mod common;

use axum::{
    Extension,
    body::{Body, Bytes},
    extract::{
        ConnectInfo, Request,
        ws::{Message, WebSocketUpgrade},
    },
    handler::HandlerWithoutStateExt,
    http::{HeaderMap, HeaderValue, StatusCode, Version, header},
    response::{IntoResponse, Response},
    routing::{any, get as axum_get, post},
};
use common::{Authority, BoxError, Client, Running, VERSIONS, channel, chunk, get, request, text};
use futures_util::{SinkExt, StreamExt};
use http_body_util::BodyExt;
use serde::Deserialize;
use std::{net::SocketAddr, sync::Arc};
use tokio::sync::{Mutex, mpsc};
use warded::{
    Matcher, Router, TransportSecurity,
    matchers::{exact_path, get as get_matcher, path, secured, unsecure, version},
};
use warded_server::{Params, app};

#[derive(Deserialize)]
struct Order {
    id: u64,
    #[serde(default)]
    note: String,
}

fn warded_app() -> axum::Router {
    let describe = |version: Version,
                    Extension(security): Extension<TransportSecurity>,
                    ConnectInfo(peer): ConnectInfo<SocketAddr>| async move {
        format!("{version:?} {security:?} {}", peer.ip())
    };
    let mut routes =
        Router::new()
            .route(get_matcher().and(exact_path("/describe")), any(describe))
            .route(
                exact_path("/secured").and(secured()),
                any(|| async { "secured" }),
            )
            .route(
                exact_path("/secured").and(unsecure()),
                any(|| async { "unsecure" }),
            )
            .route(
                path("/orders/{id}").unwrap(),
                any(|Params(order): Params<Order>| async move { format!("order {}", order.id) }),
            )
            .route(
                path("/notes/{id}/{note}").unwrap(),
                any(|Params(order): Params<Order>| async move {
                    format!("{}: {}", order.id, order.note)
                }),
            )
            .route(
                exact_path("/strict").and(
                    warded::negotiation::accepts_media_type("text/plain")
                        .unwrap()
                        .invalid_is_error(),
                ),
                any(|| async { "strict" }),
            );
    for (declared, name) in [
        (Version::HTTP_11, "one"),
        (Version::HTTP_2, "two"),
        (Version::HTTP_3, "three"),
    ] {
        let condition = exact_path("/version").and(version(declared));
        routes = routes.route(condition, any(move || async move { name }));
    }
    app(routes)
}

#[tokio::test]
async fn warded_conditions_see_the_real_connection() {
    let authority = Authority::new();
    let running = Running::start(common::server(&authority), warded_app());
    for (version, name) in VERSIONS.into_iter().zip(["one", "two", "three"]) {
        let mut client = Client::connect(version, running.https, &authority)
            .await
            .unwrap();
        for (path, expected) in [
            ("/describe", format!("{version:?} Https 127.0.0.1")),
            ("/version", name.to_owned()),
            ("/secured", "secured".to_owned()),
            ("/orders/42", "order 42".to_owned()),
            ("/notes/1/a%20b%2Bc+d%26e=f", "1: a b+c+d&e=f".to_owned()),
        ] {
            let response = client.get(path).await.unwrap();
            assert_eq!(response.status(), StatusCode::OK, "{version:?} {path}");
            assert_eq!(response.version(), version);
            assert_eq!(text(response).await.unwrap(), expected, "{version:?}");
        }
    }
}

#[tokio::test]
async fn routing_failures_become_responses() {
    let authority = Authority::new();
    let running = Running::start(common::server(&authority), warded_app());
    for version in VERSIONS {
        let mut client = Client::connect(version, running.https, &authority)
            .await
            .unwrap();
        let malformed = {
            let mut request = get("/strict");
            let value = HeaderValue::from_static("text/html;q=2");
            request.headers_mut().insert(header::ACCEPT, value);
            request
        };
        for (request, status) in [
            (get("/nowhere"), StatusCode::NOT_FOUND),
            (
                request("POST", "/describe", Body::empty()),
                StatusCode::NOT_FOUND,
            ),
            (get("/orders/forty-two"), StatusCode::BAD_REQUEST),
            (get("/notes/1/%FF"), StatusCode::BAD_REQUEST),
            (malformed, StatusCode::BAD_REQUEST),
        ] {
            let path = request.uri().path().to_owned();
            let response = client.send(request).await.unwrap();
            assert_eq!(response.status(), status, "{version:?} {path}");
        }
    }
}

#[tokio::test]
async fn a_refused_capture_is_named() {
    let authority = Authority::new();
    let running = Running::start(common::server(&authority), warded_app());
    let mut client = Client::connect(Version::HTTP_2, running.https, &authority)
        .await
        .unwrap();
    let response = client.get("/orders/forty-two").await.unwrap();
    let body = text(response).await.unwrap();
    assert_eq!(body, "invalid path: id: invalid digit found in string\n");
}

#[tokio::test]
async fn params_outside_a_warded_route_is_a_server_error() {
    let authority = Authority::new();
    let app = axum::Router::new().route("/", axum_get(|_: Params<Order>| async {}));
    let running = Running::start(common::server(&authority), app);
    let mut client = Client::connect(Version::HTTP_2, running.https, &authority)
        .await
        .unwrap();
    let response = client.get("/").await.unwrap();
    assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
}

#[tokio::test]
async fn an_ordinary_axum_application_works() {
    let authority = Authority::new();
    let app = axum::Router::new()
        .route(
            "/{name}",
            axum_get(|axum::extract::Path(name): axum::extract::Path<String>| async move { name }),
        )
        .route("/echo", post(|body: Bytes| async move { body }));
    let running = Running::start(common::server(&authority), app);
    for version in VERSIONS {
        let mut client = Client::connect(version, running.https, &authority)
            .await
            .unwrap();
        let response = client.get("/someone").await.unwrap();
        assert_eq!(text(response).await.unwrap(), "someone");
        let body = "x".repeat(1_000_000);
        let response = client
            .send(request("POST", "/echo", body.clone().into()))
            .await
            .unwrap();
        assert!(text(response).await.unwrap() == body, "{version:?}");
    }
}

#[tokio::test]
async fn responses_have_the_same_head_on_every_protocol() {
    let authority = Authority::new();
    let hop_by_hop = |_: Request| async {
        let mut response = Response::new(Body::from("twelve bytes"));
        for (name, value) in [
            ("connection", "keep-alive"),
            ("keep-alive", "timeout=5"),
            ("proxy-connection", "keep-alive"),
        ] {
            let value = HeaderValue::from_static(value);
            response.headers_mut().insert(name, value);
        }
        response
    };
    let app = axum::Router::new().fallback(hop_by_hop);
    let running = Running::start(common::server(&authority), app);
    for version in VERSIONS {
        for method in ["GET", "HEAD"] {
            let mut client = Client::connect(version, running.https, &authority)
                .await
                .unwrap();
            let response = client
                .send(request(method, "/", Body::empty()))
                .await
                .unwrap();
            let headers = response.headers().clone();
            assert_eq!(
                headers[header::CONTENT_LENGTH],
                "12",
                "{version:?} {method}"
            );
            assert!(headers.contains_key(header::DATE), "{version:?} {method}");
            if version != Version::HTTP_11 {
                for name in ["connection", "keep-alive", "proxy-connection"] {
                    assert!(!headers.contains_key(name), "{version:?} {name}");
                }
            }
            let expected = if method == "GET" { "twelve bytes" } else { "" };
            assert_eq!(text(response).await.unwrap(), expected, "{version:?}");
        }
    }
}

#[tokio::test]
async fn a_service_without_a_router_gets_the_same_heads() {
    let authority = Authority::new();
    // Not a handler: Axum gives those HEAD and Content-Length handling of its own.
    let app = tower::service_fn(|_: Request| async {
        Ok::<_, std::convert::Infallible>(Response::new(Body::from("abc")))
    });
    let bound = common::server(&authority).bind().unwrap();
    let address = bound.https_addr();
    let serving = tokio::spawn(bound.serve_until(app, std::future::pending()));
    for version in VERSIONS {
        for (method, body) in [("GET", "abc"), ("HEAD", "")] {
            let mut client = Client::connect(version, address, &authority).await.unwrap();
            let request = request(method, "/", Body::empty());
            let response = client.send(request).await.unwrap();
            let length = &response.headers()[header::CONTENT_LENGTH];
            assert_eq!(length, "3", "{version:?} {method}");
            assert_eq!(text(response).await.unwrap(), body, "{version:?} {method}");
        }
    }
    serving.abort();
}

#[tokio::test]
async fn a_layer_sees_what_a_handler_would_answer_to_get() {
    async fn page() -> &'static str {
        "abc"
    }

    let authority = Authority::new();
    let routes = Router::new().route(exact_path("/"), page.into_service());
    // Stands for compression: the content has another length afterwards.
    let twice = axum::middleware::map_response(|response: Response| async {
        let body = response.into_body().collect().await.unwrap().to_bytes();
        body.repeat(2)
    });
    let running = Running::start(common::server(&authority), app(routes).layer(twice));
    for version in VERSIONS {
        for (method, body) in [("GET", "abcabc"), ("HEAD", "")] {
            let mut client = Client::connect(version, running.https, &authority)
                .await
                .unwrap();
            let request = request(method, "/", Body::empty());
            let response = client.send(request).await.unwrap();
            let length = &response.headers()[header::CONTENT_LENGTH];
            assert_eq!(length, "6", "{version:?} {method}");
            assert_eq!(text(response).await.unwrap(), body, "{version:?} {method}");
        }
    }
}

#[tokio::test]
async fn head_does_not_invent_a_length_for_a_stream() {
    let authority = Authority::new();
    let app = axum::Router::new().route(
        "/",
        axum_get(|| async {
            let (mut sender, body) = channel();
            tokio::spawn(async move { sender.send_data("abc".into()).await });
            body
        }),
    );
    let running = Running::start(common::server(&authority), app);
    for version in VERSIONS {
        let mut client = Client::connect(version, running.https, &authority)
            .await
            .unwrap();
        let response = client
            .send(request("HEAD", "/", Body::empty()))
            .await
            .unwrap();
        let length = response.headers().get(header::CONTENT_LENGTH);
        assert_eq!(length, None, "{version:?}");
    }
}

#[tokio::test]
async fn connect_works_over_http1_and_is_refused_over_http3() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let authority = Authority::new();
    let app = tower::service_fn(|mut request: Request| async move {
        tokio::spawn(async move {
            let upgraded = hyper::upgrade::on(&mut request).await.unwrap();
            let mut tunnel = hyper_util::rt::TokioIo::new(upgraded);
            tunnel.write_all(b"tunnel").await.unwrap();
        });
        Ok::<_, std::convert::Infallible>(Response::new(Body::empty()))
    });
    let bound = common::server(&authority).bind().unwrap();
    let address = bound.https_addr();
    let serving = tokio::spawn(bound.serve_until(app, std::future::pending()));

    let mut stream = common::tls(address, &authority, b"http/1.1").await.unwrap();
    stream
        .write_all(b"CONNECT example.com:443 HTTP/1.1\r\nHost: example.com:443\r\n\r\n")
        .await
        .unwrap();
    let mut answer = Vec::new();
    common::soon(async {
        while !answer.ends_with(b"tunnel") {
            let mut byte = [0];
            stream.read_exact(&mut byte).await.unwrap();
            answer.extend_from_slice(&byte);
        }
    })
    .await;
    let answer = String::from_utf8(answer).unwrap();
    assert!(answer.starts_with("HTTP/1.1 200 "), "{answer}");
    assert!(!answer.contains("content-length"), "{answer}");

    // :method CONNECT and an :authority.
    let (_endpoint, quic) = common::quic(address, &authority).await.unwrap();
    let (mut send, mut receive) = quic.open_bi().await.unwrap();
    let block = [0, 0, 0xcf, 0x50, 1, b'a'];
    send.write_all(&common::frame(1, block.len() as u64, &block))
        .await
        .unwrap();
    send.finish().unwrap();
    if let Ok(answer) = common::soon(receive.read_to_end(1024)).await {
        // Not :status 200 from the static table.
        assert!(
            !answer[..answer.len().min(8)].contains(&0xd9),
            "{answer:02x?}"
        );
    }
    serving.abort();
}

#[tokio::test]
async fn statuses_without_content_have_no_body() {
    let authority = Authority::new();
    let app = axum::Router::new()
        .route(
            "/204",
            axum_get(|| async { (StatusCode::NO_CONTENT, "unexpected") }),
        )
        .route(
            "/205",
            // With a Transfer-Encoding, which cannot go beside the length 0.
            axum_get(|| async {
                let chunked = [(header::TRANSFER_ENCODING, "chunked")];
                (StatusCode::RESET_CONTENT, chunked, "unexpected")
            }),
        )
        .route(
            "/304",
            axum_get(|| async { (StatusCode::NOT_MODIFIED, "unexpected") }),
        );
    let running = Running::start(common::server(&authority), app);
    for version in VERSIONS {
        for path in ["/204", "/205", "/304"] {
            let mut client = Client::connect(version, running.https, &authority)
                .await
                .unwrap();
            let response = client.get(path).await.unwrap();
            let length = response.headers().get(header::CONTENT_LENGTH);
            if path == "/205" {
                assert!(length.is_none_or(|length| length == "0"), "{version:?}");
            } else {
                assert_eq!(length, None, "{version:?} {path}");
            }
            assert_eq!(text(response).await.unwrap(), "", "{version:?} {path}");
        }
    }
}

#[tokio::test]
async fn an_informational_status_is_not_a_final_response() {
    let authority = Authority::new();
    let app = axum::Router::new().fallback(|| async { StatusCode::EARLY_HINTS });
    let running = Running::start(common::server(&authority), app);
    let mut client = Client::connect(Version::HTTP_3, running.https, &authority)
        .await
        .unwrap();
    let response = client.get("/").await.unwrap();
    assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
}

#[tokio::test]
async fn http3_is_advertised_where_it_is_available() {
    let authority = Authority::new();
    let alt_svc = async |server, version| {
        let app = axum::Router::new().fallback(|| async {});
        let running = Running::start(server, app);
        let mut client = Client::connect(version, running.https, &authority)
            .await
            .unwrap();
        let response = client.get("/").await.unwrap();
        let value = response.headers().get(header::ALT_SVC).cloned();
        (running.https.port(), value)
    };
    for version in VERSIONS {
        let (port, value) = alt_svc(common::server(&authority), version).await;
        assert_eq!(value.unwrap(), format!("h3=\":{port}\"; ma=86400"));
        let (_, value) = alt_svc(common::server(&authority).public_port(443), version).await;
        assert_eq!(value.unwrap(), "h3=\":443\"; ma=86400");
        if version != Version::HTTP_3 {
            let (_, value) = alt_svc(common::server(&authority).http3(false), version).await;
            assert_eq!(value, None);
        }
    }
}

#[tokio::test]
async fn without_http3_nothing_listens_on_udp() {
    let authority = Authority::new();
    let server = common::server(&authority).http3(false);
    let running = Running::start(server, axum::Router::new());
    std::net::UdpSocket::bind(running.https).expect("the port is free for UDP");
}

#[tokio::test]
async fn split_cookies_arrive_as_one_field() {
    let authority = Authority::new();
    let app = axum::Router::new().fallback(|headers: HeaderMap| async move {
        let cookies = headers.get_all(header::COOKIE).iter();
        let cookies: Vec<_> = cookies.map(|value| value.to_str().unwrap()).collect();
        cookies.join(" | ")
    });
    let running = Running::start(common::server(&authority), app);
    for version in [Version::HTTP_2, Version::HTTP_3] {
        let mut client = Client::connect(version, running.https, &authority)
            .await
            .unwrap();
        let mut request = get("/");
        for cookie in ["a=1", "b=2"] {
            let value = HeaderValue::from_static(cookie);
            request.headers_mut().append(header::COOKIE, value);
        }
        let response = client.send(request).await.unwrap();
        assert_eq!(text(response).await.unwrap(), "a=1; b=2", "{version:?}");
    }
}

#[tokio::test]
async fn bodies_stream_in_both_directions() {
    let authority = Authority::new();
    // Echo each piece as it arrives, then report the trailers as trailers.
    let app = axum::Router::new().fallback(|request: Request| async {
        let (mut sender, body) = channel();
        tokio::spawn(async move {
            let mut body = request.into_body();
            while let Some(frame) = body.frame().await {
                match frame?.into_data() {
                    Ok(data) => sender.send_data(data).await?,
                    Err(frame) => sender.send_trailers(frame.into_trailers().unwrap()).await?,
                }
            }
            Ok::<_, BoxError>(())
        });
        Response::new(body)
    });
    let running = Running::start(common::server(&authority), app);
    for version in VERSIONS {
        let mut client = Client::connect(version, running.https, &authority)
            .await
            .unwrap();
        let (mut sender, body) = channel();
        let mut outgoing = request("POST", "/", body);
        let headers = outgoing.headers_mut();
        headers.insert(header::TRAILER, HeaderValue::from_static("x-sum"));
        headers.insert(header::TE, HeaderValue::from_static("trailers"));
        let response = client.send(outgoing).await.unwrap();
        let mut body = response.into_body();
        // Each piece returns before the next is sent, so neither side collects the body.
        for piece in ["first", "second", "third"] {
            sender.send_data(piece.into()).await.unwrap();
            assert_eq!(chunk(&mut body).await.unwrap(), piece, "{version:?}");
        }
        let mut trailers = HeaderMap::new();
        trailers.insert("x-sum", HeaderValue::from_static("3"));
        sender.send_trailers(trailers).await.unwrap();
        drop(sender);
        let rest = common::soon(body.collect()).await.unwrap();
        // Hyper sends HTTP/1.1 response trailers only for fields the response announced.
        if version != Version::HTTP_11 {
            let trailers = rest.trailers().expect("trailers");
            assert_eq!(trailers["x-sum"], "3", "{version:?}");
        }
        assert!(rest.to_bytes().is_empty());
    }
}

#[tokio::test]
async fn an_event_stream_is_cancelled_when_its_client_leaves() {
    let authority = Authority::new();
    let (ended, mut has_ended) = mpsc::channel(1);
    let app = axum::Router::new().fallback(move || {
        let ended = ended.clone();
        async move {
            let (mut sender, body) = channel();
            tokio::spawn(async move {
                while sender.send_data("data: tick\n\n".into()).await.is_ok() {
                    tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                }
                ended.send(()).await.unwrap();
            });
            ([(header::CONTENT_TYPE, "text/event-stream")], body).into_response()
        }
    });
    let running = Running::start(common::server(&authority), app);
    for version in VERSIONS {
        let mut client = Client::connect(version, running.https, &authority)
            .await
            .unwrap();
        let mut body = client.get("/").await.unwrap().into_body();
        chunk(&mut body).await.unwrap();
        drop(body);
        if let Client::Http3(client) = client {
            client.close().await;
        }
        common::soon(has_ended.recv()).await.expect("{version:?}");
    }
}

#[tokio::test]
async fn an_unread_request_body_does_not_spoil_the_response() {
    let authority = Authority::new();
    let app = axum::Router::new().fallback(|| async { "refused" });
    let running = Running::start(common::server(&authority), app);
    // An HTTP/1.1 client that is still writing may miss the response, whatever the server does.
    for version in [Version::HTTP_2, Version::HTTP_3] {
        let mut client = Client::connect(version, running.https, &authority)
            .await
            .unwrap();
        // Keep the body open, so the response precedes its end.
        let (mut sender, body) = channel();
        let sending =
            tokio::spawn(
                async move { while sender.send_data(vec![0; 10_000].into()).await.is_ok() {} },
            );
        let response = client.send(request("POST", "/", body)).await.unwrap();
        assert_eq!(text(response).await.unwrap(), "refused", "{version:?}");
        sending.abort();
    }
}

#[tokio::test]
async fn a_response_does_not_wait_for_a_request_body() {
    let authority = Authority::new();
    let app = axum::Router::new()
        .route("/unread", post(|| async { "early" }))
        .route(
            "/given-up",
            post(|request: Request| async {
                // Drops the body while a read is in progress.
                let mut body = request.into_body();
                let wait = std::time::Duration::from_millis(20);
                assert!(tokio::time::timeout(wait, body.frame()).await.is_err());
                "early"
            }),
        );
    let running = Running::start(common::server(&authority), app);
    for version in VERSIONS {
        let mut client = Client::connect(version, running.https, &authority)
            .await
            .unwrap();
        for path in ["/unread", "/given-up"] {
            let (_sender, body) = channel();
            let response = client.send(request("POST", path, body)).await.unwrap();
            assert_eq!(text(response).await.unwrap(), "early", "{version:?} {path}");
            // HTTP/1.1 cannot reuse a connection whose request never ended.
            if version == Version::HTTP_11 {
                client = Client::connect(version, running.https, &authority)
                    .await
                    .unwrap();
            }
        }
    }
}

#[tokio::test]
async fn cleartext_redirects_or_serves_the_application() {
    let authority = Authority::new();
    let any_port = ([127, 0, 0, 1], 0).into();
    let request = "GET /secured?x HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n";

    let server = common::server(&authority).redirect_http(any_port);
    let running = Running::start(server, warded_app());
    let response = common::cleartext(running.http.unwrap(), request).await;
    assert!(response.starts_with("HTTP/1.1 308 "), "{response}");
    let location = format!(
        "location: https://localhost:{}/secured?x\r\n",
        running.https.port()
    );
    assert!(response.contains(&location), "{response}");

    let server = common::server(&authority).serve_http(any_port);
    let running = Running::start(server, warded_app());
    let response = common::cleartext(running.http.unwrap(), request).await;
    assert!(response.ends_with("\r\n\r\nunsecure"), "{response}");
    assert!(!response.contains("alt-svc"), "{response}");
}

#[tokio::test]
async fn websockets_work_over_http1() {
    let authority = Authority::new();
    let app = axum::Router::new().route(
        "/ws",
        any(|upgrade: WebSocketUpgrade| async {
            upgrade.on_upgrade(|mut socket| async move {
                while let Some(Ok(Message::Text(message))) = socket.recv().await {
                    socket.send(Message::Text(message)).await.unwrap();
                }
            })
        }),
    );
    let running = Running::start(common::server(&authority), app);
    let stream = common::tls(running.https, &authority, b"http/1.1")
        .await
        .unwrap();
    let (mut socket, _) = tokio_tungstenite::client_async("wss://localhost/ws", stream)
        .await
        .unwrap();
    socket.send("hello".into()).await.unwrap();
    let reply = common::soon(socket.next()).await.unwrap().unwrap();
    assert_eq!(reply.into_text().unwrap(), "hello");
}

#[tokio::test]
async fn one_connection_carries_concurrent_requests() {
    let authority = Authority::new();
    let (release, held) = tokio::sync::oneshot::channel::<()>();
    let held = Arc::new(Mutex::new(Some(held)));
    let app = axum::Router::new()
        .route(
            "/held",
            axum_get(move || {
                let held = held.clone();
                async move {
                    let held = held.lock().await.take().unwrap();
                    held.await.unwrap();
                    "held"
                }
            }),
        )
        .route("/free", axum_get(|| async { "free" }));
    let running = Running::start(common::server(&authority), app);
    let Client::Http3(mut client) = Client::connect(Version::HTTP_3, running.https, &authority)
        .await
        .unwrap()
    else {
        unreachable!()
    };
    let mut held_client = client.send.clone();
    let waiting = tokio::spawn(async move {
        common::send_http3(&mut held_client, get("/held"))
            .await
            .unwrap()
    });
    let response = common::send_http3(&mut client.send, get("/free"))
        .await
        .unwrap();
    assert_eq!(text(response).await.unwrap(), "free");
    release.send(()).unwrap();
    assert_eq!(text(waiting.await.unwrap()).await.unwrap(), "held");
}

/// The code of the HTTP/3 stream error that a request failed with.
fn http3_error(error: &BoxError) -> Option<h3::error::Code> {
    match error.downcast_ref::<h3::error::StreamError>()? {
        h3::error::StreamError::RemoteTerminate { code, .. } => Some(*code),
        _ => None,
    }
}

/// The status of a response, its content if it is a 200, or the HTTP/3 error instead.
async fn outcome(response: Result<Response, BoxError>) -> String {
    match response {
        Ok(response) if response.status() == StatusCode::OK => text(response).await.unwrap(),
        Ok(response) => response.status().to_string(),
        Err(error) => {
            http3_error(&error).map_or_else(|| format!("{error:?}"), |code| code.to_string())
        }
    }
}

#[tokio::test]
async fn whitespace_around_a_field_value_is_malformed() {
    let authority = Authority::new();
    let tenant = warded::matchers::header_eq("x-tenant", "acme").unwrap();
    let routes = Router::new()
        .route(exact_path("/").and(tenant), any(|| async { "tenant" }))
        .route(
            exact_path("/"),
            any(|headers: HeaderMap| async move { format!("other {:?}", headers["x-tenant"]) }),
        );
    let running = Running::start(common::server(&authority), app(routes));
    for version in VERSIONS {
        // One connection for all, which outlives the refusals.
        let mut client = Client::connect(version, running.https, &authority)
            .await
            .unwrap();
        for (value, http1) in [
            (" acme", "tenant"),
            ("acme ", "tenant"),
            ("\tacme", "tenant"),
            ("acme\t", "tenant"),
            (" \tacme\t ", "tenant"),
            (" ", "other \"\""),
            // Well-formed values, which arrive as they are on every protocol.
            ("acme", "tenant"),
            ("a \tcme", "other \"a \tcme\""),
            ("", "other \"\""),
        ] {
            let mut request = get("/");
            let field = HeaderValue::from_static(value);
            request.headers_mut().insert("x-tenant", field);
            let malformed = value.trim_matches([' ', '\t']) != value;
            let expected = match version {
                Version::HTTP_2 if malformed => "400 Bad Request",
                Version::HTTP_3 if malformed => "H3_MESSAGE_ERROR",
                // Hyper's parser removes the whitespace, as RFC 9112 says.
                _ => http1,
            };
            let actual = outcome(client.send(request).await).await;
            assert_eq!(actual, expected, "{version:?} {value:?}");
        }
    }
    let mut client = Client::connect(Version::HTTP_2, running.https, &authority)
        .await
        .unwrap();
    let mut request = get("/");
    request
        .headers_mut()
        .insert("x-tenant", HeaderValue::from_static("acme "));
    let response = client.send(request).await.unwrap();
    let reason = "Malformed request: a field value begins or ends with whitespace.\n";
    assert_eq!(text(response).await.unwrap(), reason);
}

/// An application that answers with the target and the `Host` fields that it sees.
fn hosts() -> axum::Router {
    axum::Router::new().fallback(|request: Request| async move {
        let hosts = request.headers().get_all(header::HOST).iter();
        let hosts: Vec<_> = hosts.map(|host| host.to_str().unwrap()).collect();
        format!("{} {hosts:?}", request.uri())
    })
}

#[tokio::test]
async fn http1_requests_name_one_valid_host() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let authority = Authority::new();
    let server = common::server(&authority).serve_http(([127, 0, 0, 1], 0).into());
    let running = Running::start(server, hosts());
    for (target, version, fields, expected) in [
        ("/", "1.1", "Host: localhost\r\n", r#"/ ["localhost"]"#),
        (
            "/",
            "1.1",
            "Host: localhost:8080\r\n",
            r#"/ ["localhost:8080"]"#,
        ),
        (
            "/",
            "1.1",
            "Host: LOCALHOST.:8080\r\n",
            r#"/ ["LOCALHOST.:8080"]"#,
        ),
        ("/", "1.1", "Host: [::1]:8080\r\n", r#"/ ["[::1]:8080"]"#),
        // The target has no host.
        ("/", "1.1", "Host:\r\n", r#"/ [""]"#),
        ("/", "1.0", "", "/ []"),
        // The target's host replaces the field's.
        (
            "http://localhost:8080/",
            "1.1",
            "Host: other.example\r\n",
            r#"http://localhost:8080/ ["localhost:8080"]"#,
        ),
        (
            "http://localhost:8080/",
            "1.1",
            "Host:\r\n",
            r#"http://localhost:8080/ ["localhost:8080"]"#,
        ),
        ("http://localhost/", "1.0", "", "http://localhost/ []"),
        // Refused by the server, before the application.
        ("/", "1.1", "", "refused"),
        ("/", "1.1", "Host: one\r\nHost: two\r\n", "refused"),
        ("/", "1.1", "Host: one\r\nHost: one\r\n", "refused"),
        ("/", "1.0", "Host: one\r\nHost: two\r\n", "refused"),
        ("/", "1.1", "Host: not a host/x\r\n", "refused"),
        ("/", "1.1", "Host: user@localhost\r\n", "refused"),
        ("/", "1.1", "Host: localhost:abc\r\n", "refused"),
        ("/", "1.1", "Host: [bogus]\r\n", "refused"),
        ("http://localhost/", "1.1", "", "refused"),
        (
            "http://localhost/",
            "1.1",
            "Host: a\r\nHost: b\r\n",
            "refused",
        ),
        (
            "http://user@localhost/",
            "1.1",
            "Host: localhost\r\n",
            "refused",
        ),
        // Refused by Hyper already: a value folded onto a second line.
        ("/", "1.1", "Host: local\r\n host\r\n", "400"),
    ] {
        let request = format!("GET {target} HTTP/{version}\r\n{fields}Connection: close\r\n\r\n");
        for tls in [false, true] {
            let response = if tls {
                let mut stream = common::tls(running.https, &authority, b"http/1.1")
                    .await
                    .unwrap();
                stream.write_all(request.as_bytes()).await.unwrap();
                let mut response = String::new();
                common::soon(stream.read_to_string(&mut response))
                    .await
                    .unwrap();
                response
            } else {
                common::cleartext(running.http.unwrap(), &request).await
            };
            let (head, body) = response.split_once("\r\n\r\n").unwrap();
            let status = head.split(' ').nth(1).unwrap();
            let actual = match status {
                "200" => body,
                "400" if body.starts_with("Malformed request: ") => "refused",
                other => other,
            };
            assert_eq!(actual, expected, "TLS {tls}: {request:?}");
        }
    }
}

#[tokio::test]
async fn an_http2_host_field_names_the_host_of_the_authority() {
    let authority = Authority::new();
    let running = Running::start(common::server(&authority), hosts());
    let mut client = Client::connect(Version::HTTP_2, running.https, &authority)
        .await
        .unwrap();
    for (fields, expected) in [
        (&[][..], "https://localhost/ []"),
        (&["localhost"], r#"https://localhost/ ["localhost"]"#),
        (
            &["LocalHost:443"],
            r#"https://localhost/ ["LocalHost:443"]"#,
        ),
        (&["other.example"], "400 Bad Request"),
        (&["localhost:8443"], "400 Bad Request"),
        (&["user@localhost"], "400 Bad Request"),
        (&[""], "400 Bad Request"),
        (&["localhost", "localhost"], "400 Bad Request"),
    ] {
        let mut request = get("/");
        for host in fields {
            let host = HeaderValue::from_static(host);
            request.headers_mut().append(header::HOST, host);
        }
        let actual = outcome(client.send(request).await).await;
        assert_eq!(actual, expected, "{fields:?}");
    }
}

/// Give a response the framing fields that a test case names.
fn frame_as(headers: &mut HeaderMap, case: &str) {
    let fields: &[(_, &'static str)] = match case {
        "wrong" => &[(header::CONTENT_LENGTH, "6")],
        "short" => &[(header::CONTENT_LENGTH, "100")],
        "two" => &[(header::CONTENT_LENGTH, "5"), (header::CONTENT_LENGTH, "6")],
        "same" => &[(header::CONTENT_LENGTH, "5"), (header::CONTENT_LENGTH, "5")],
        "invalid" => &[(header::CONTENT_LENGTH, "five")],
        "chunked" => &[(header::TRANSFER_ENCODING, "chunked")],
        "both" => &[
            (header::TRANSFER_ENCODING, "chunked"),
            (header::CONTENT_LENGTH, "5"),
        ],
        _ => &[],
    };
    for (name, value) in fields {
        headers.append(name, HeaderValue::from_static(value));
    }
}

#[tokio::test]
async fn responses_are_framed_by_their_true_length_or_their_encoding() {
    use std::convert::Infallible;

    let authority = Authority::new();
    // Axum gives the responses of handlers a length, but not those of other services.
    let service = tower::service_fn(|request: Request| async move {
        let (case, body) = match request.uri().path().split('/').collect::<Vec<_>>()[..] {
            ["", "service", case] => (case.to_owned(), Body::from("hello")),
            ["", "stream", case] => {
                let (mut sender, body) = channel();
                tokio::spawn(async move { sender.send_data("hello".into()).await });
                (case.to_owned(), body)
            }
            _ => unreachable!(),
        };
        let mut response = Response::new(body);
        frame_as(response.headers_mut(), &case);
        Ok::<_, Infallible>(response)
    });
    let handler = |axum::extract::Path(case): axum::extract::Path<String>| async move {
        let mut response = "hello".into_response();
        frame_as(response.headers_mut(), &case);
        response
    };
    let app = axum::Router::new()
        .route("/handler/{case}", any(handler))
        .fallback_service(service);
    let server = common::server(&authority).serve_http(([127, 0, 0, 1], 0).into());
    let running = Running::start(server, app);
    let cases = [
        "none", "wrong", "short", "two", "same", "invalid", "chunked", "both",
    ];
    let mut paths: Vec<_> = cases
        .iter()
        .map(|case| format!("/service/{case}"))
        .collect();
    paths.extend(cases.iter().map(|case| format!("/handler/{case}")));
    // Content of unknown length keeps a valid length, which the protocols check it against.
    paths.extend(
        ["two", "same", "invalid", "chunked", "both"].map(|case| format!("/stream/{case}")),
    );
    for path in &paths {
        // Over HTTP/1.1 the application's Transfer-Encoding frames the content, and Hyper sends none
        // for HEAD.
        let encoded = path.ends_with("/chunked") || path.ends_with("/both");
        for method in ["GET", "HEAD"] {
            let length = match (path.as_str(), method) {
                ("/stream/two" | "/stream/invalid" | "/stream/chunked", _) => None,
                // Axum has taken the content off, and the server cannot tell the true length.
                ("/handler/wrong", "HEAD") => Some("6"),
                ("/handler/short", "HEAD") => Some("100"),
                ("/handler/two" | "/handler/invalid", "HEAD") => None,
                _ => Some("5"),
            };
            let content = if method == "GET" { "hello" } else { "" };
            for version in VERSIONS {
                let (length, encoding) = match version {
                    Version::HTTP_11 if encoded && method == "GET" => (None, Some("chunked")),
                    Version::HTTP_11 if encoded => (None, None),
                    _ => (length, None),
                };
                let mut client = Client::connect(version, running.https, &authority)
                    .await
                    .unwrap();
                let case = format!("{version:?} {method} {path}");
                let response = client.send(request(method, path, Body::empty())).await;
                let response = response.unwrap_or_else(|error| panic!("{case}: {error}"));
                assert_eq!(response.status(), StatusCode::OK, "{case}");
                let headers = response.headers().clone();
                let lengths: Vec<_> = headers.get_all(header::CONTENT_LENGTH).iter().collect();
                assert_eq!(lengths, Vec::from_iter(length), "{case}");
                if version != Version::HTTP_11 || encoding.is_some() {
                    let encodings: Vec<_> =
                        headers.get_all(header::TRANSFER_ENCODING).iter().collect();
                    assert_eq!(encodings, Vec::from_iter(encoding), "{case}");
                }
                assert_eq!(text(response).await.unwrap(), content, "{case}");
            }
            // What goes over the wire, which a client could otherwise tidy up.
            let request =
                format!("{method} {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n");
            let response = common::cleartext(running.http.unwrap(), &request).await;
            let (head, body) = response.split_once("\r\n\r\n").unwrap();
            let head = head.to_ascii_lowercase();
            let fields = |name: &str| head.lines().filter(|line| line.starts_with(name)).count();
            let framing = (fields("content-length:"), fields("transfer-encoding:"));
            let length = length.filter(|_| !encoded);
            let expected = match method {
                "GET" => (usize::from(length.is_some()), usize::from(length.is_none())),
                _ => (usize::from(length.is_some()), 0),
            };
            assert_eq!(framing, expected, "{request:?}: {response}");
            if method == "GET" && length.is_some() {
                assert_eq!(body, content, "{request:?}");
            } else if method == "GET" {
                assert_eq!(body, "5\r\nhello\r\n0\r\n\r\n", "{request:?}");
            } else {
                assert_eq!(body, "", "{request:?}");
            }
        }
    }
}

#[tokio::test]
async fn an_applications_transfer_encoding_frames_http1_content() {
    use http_body_util::Full;
    use std::convert::Infallible;

    let authority = Authority::new();
    let app = tower::service_fn(|request: Request| async move {
        let response = match request.uri().path() {
            // Content of known length, with trailers, which HTTP/1.1 sends only in chunks.
            "/trailers" => {
                let mut trailers = HeaderMap::new();
                trailers.insert("x-sum", HeaderValue::from_static("1"));
                let body = Full::new(Bytes::from_static(b"hello"))
                    .with_trailers(async { Some(Ok::<_, Infallible>(trailers)) });
                let mut response = Response::new(Body::new(body));
                let headers = response.headers_mut();
                headers.insert(header::TRAILER, HeaderValue::from_static("x-sum"));
                headers.insert(
                    header::TRANSFER_ENCODING,
                    HeaderValue::from_static("chunked"),
                );
                response
            }
            // Content in another transfer coding; the test does not decode it.
            "/coded" => {
                let mut response = Response::new(Body::from("hello"));
                let coding = HeaderValue::from_static("gzip, chunked");
                response
                    .headers_mut()
                    .insert(header::TRANSFER_ENCODING, coding);
                response
            }
            _ => unreachable!(),
        };
        Ok::<_, Infallible>(response)
    });
    let app = axum::Router::new().fallback_service(app);
    let server = common::server(&authority).serve_http(([127, 0, 0, 1], 0).into());
    let running = Running::start(server, app);
    for version in VERSIONS {
        let mut client = Client::connect(version, running.https, &authority)
            .await
            .unwrap();
        let mut request = get("/trailers");
        let te = HeaderValue::from_static("trailers");
        request.headers_mut().insert(header::TE, te);
        let response = client.send(request).await.unwrap();
        let headers = response.headers().clone();
        let (length, encoding) = match version {
            Version::HTTP_11 => (None, Some("chunked")),
            _ => (Some("5"), None),
        };
        let lengths: Vec<_> = headers.get_all(header::CONTENT_LENGTH).iter().collect();
        assert_eq!(lengths, Vec::from_iter(length), "{version:?}");
        let encodings: Vec<_> = headers.get_all(header::TRANSFER_ENCODING).iter().collect();
        assert_eq!(encodings, Vec::from_iter(encoding), "{version:?}");
        let collected = common::soon(response.into_body().collect()).await.unwrap();
        let trailers = collected.trailers().cloned().expect("trailers");
        assert_eq!(trailers["x-sum"], "1", "{version:?}");
        assert_eq!(collected.to_bytes(), "hello", "{version:?}");
    }
    let request = "GET /coded HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n";
    let response = common::cleartext(running.http.unwrap(), request).await;
    let (head, body) = response.split_once("\r\n\r\n").unwrap();
    assert!(
        head.contains("\r\ntransfer-encoding: gzip, chunked\r\n"),
        "{head}"
    );
    assert!(!head.contains("content-length"), "{head}");
    assert_eq!(body, "5\r\nhello\r\n0\r\n\r\n");
}

#[tokio::test]
async fn responses_without_content_have_no_framing() {
    use std::convert::Infallible;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let app = tower::service_fn(|request: Request| async move {
        let field = |name| request.headers().get(name).cloned();
        let (status, framing) = match field("x-case").unwrap().as_bytes() {
            b"length" => (StatusCode::OK, (header::CONTENT_LENGTH, "0")),
            b"chunked" => (StatusCode::OK, (header::TRANSFER_ENCODING, "chunked")),
            b"switch" => (
                StatusCode::SWITCHING_PROTOCOLS,
                (header::CONTENT_LENGTH, "6"),
            ),
            b"switch-right" => (
                StatusCode::SWITCHING_PROTOCOLS,
                (header::CONTENT_LENGTH, "5"),
            ),
            _ => unreachable!(),
        };
        let mut response = Response::new(Body::from("hello"));
        *response.status_mut() = status;
        let headers = response.headers_mut();
        headers.insert(framing.0, HeaderValue::from_static(framing.1));
        if status == StatusCode::SWITCHING_PROTOCOLS {
            headers.insert(header::CONNECTION, HeaderValue::from_static("upgrade"));
            headers.insert(header::UPGRADE, HeaderValue::from_static("example"));
        }
        Ok::<_, Infallible>(response)
    });
    let authority = Authority::new();
    let server = common::server(&authority).serve_http(([127, 0, 0, 1], 0).into());
    let running = Running::start(server, axum::Router::new().fallback_service(app));
    for (request, status) in [
        // A tunnel begins after the head of a successful response to CONNECT.
        (
            "CONNECT localhost:1 HTTP/1.1\r\nHost: localhost:1\r\nX-Case: length\r\n",
            "200",
        ),
        (
            "CONNECT localhost:1 HTTP/1.1\r\nHost: localhost:1\r\nX-Case: chunked\r\n",
            "200",
        ),
        // An informational response ends with its head.
        (
            "GET / HTTP/1.1\r\nHost: localhost\r\nConnection: upgrade\r\nUpgrade: example\r\n\
             X-Case: switch\r\n",
            "101",
        ),
        (
            "GET / HTTP/1.1\r\nHost: localhost\r\nConnection: upgrade\r\nUpgrade: example\r\n\
             X-Case: switch-right\r\n",
            "101",
        ),
    ] {
        let mut stream = tokio::net::TcpStream::connect(running.http.unwrap())
            .await
            .unwrap();
        stream
            .write_all(format!("{request}\r\n").as_bytes())
            .await
            .unwrap();
        let mut head = Vec::new();
        common::soon(async {
            while !head.ends_with(b"\r\n\r\n") {
                let mut byte = [0];
                stream.read_exact(&mut byte).await.unwrap();
                head.push(byte[0]);
            }
        })
        .await;
        let head = String::from_utf8(head).unwrap();
        assert!(
            head.starts_with(&format!("HTTP/1.1 {status} ")),
            "{request:?}: {head}"
        );
        for name in ["content-length", "transfer-encoding"] {
            assert!(!head.contains(name), "{request:?}: {head}");
        }
    }
}
