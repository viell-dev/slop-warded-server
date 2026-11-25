//! The same application over real HTTP/1.1, HTTP/2 and HTTP/3 connections.

mod common;

use axum::{
    Extension,
    body::{Body, Bytes},
    extract::{
        ConnectInfo, Request,
        ws::{Message, WebSocketUpgrade},
    },
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
async fn statuses_without_content_have_no_body() {
    let authority = Authority::new();
    let app = axum::Router::new()
        .route(
            "/204",
            axum_get(|| async { (StatusCode::NO_CONTENT, "unexpected") }),
        )
        .route(
            "/304",
            axum_get(|| async { (StatusCode::NOT_MODIFIED, "unexpected") }),
        );
    let running = Running::start(common::server(&authority), app);
    for version in VERSIONS {
        for path in ["/204", "/304"] {
            let mut client = Client::connect(version, running.https, &authority)
                .await
                .unwrap();
            let response = client.get(path).await.unwrap();
            let length = response.headers().get(header::CONTENT_LENGTH);
            assert_eq!(length, None, "{version:?} {path}");
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
