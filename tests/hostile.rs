//! Clients and applications that misbehave.

mod common;

use axum::{
    body::{Body, Bytes},
    http::{HeaderValue, StatusCode, Version, header},
    response::Response,
    routing::{get as axum_get, post},
};
use common::{
    Authority, BoxError, Client, DATA, Running, VERSIONS, channel, chunk, frame, get,
    headers_frame, request, text,
};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use warded_server::Limits;

/// A `content-length` field: its name from the static table and a literal value.
fn content_length(value: &str) -> Vec<u8> {
    let mut field = vec![0x54, value.len().try_into().unwrap()];
    field.extend_from_slice(value.as_bytes());
    field
}

/// Send bytes on a request stream, end it, and return everything the server sends back.
async fn raw(quic: &quinn::Connection, bytes: &[u8]) -> Result<Vec<u8>, BoxError> {
    common::soon(async {
        let (mut send, mut receive) = quic.open_bi().await?;
        send.write_all(bytes).await?;
        send.finish()?;
        Ok(receive.read_to_end(1 << 20).await?)
    })
    .await
}

/// Whether a raw response begins with a HEADERS frame whose status field is this one.
///
/// h3 encodes the statuses used here as an index into QPACK's static table, or with their name
/// from it and a literal value.
fn has_status(response: &[u8], status: StatusCode) -> bool {
    let indexed = match status {
        StatusCode::OK => Some(0xd9),
        StatusCode::BAD_REQUEST => Some(0xff),
        _ => None,
    };
    let literal = status.as_str().as_bytes();
    let head = &response[..response.len().min(24)];
    head.first() == Some(&1)
        && (indexed.is_some_and(|byte| head.contains(&byte))
            || head.windows(literal.len()).any(|window| window == literal)
            || huffman(status)
                .is_some_and(|coded| head.windows(coded.len()).any(|window| window == coded)))
}

/// The Huffman coding of a status, where h3 uses it.
fn huffman(status: StatusCode) -> Option<&'static [u8]> {
    match status {
        StatusCode::REQUEST_HEADER_FIELDS_TOO_LARGE => Some(&[0x69, 0x90, 0xff]),
        _ => None,
    }
}

fn echo() -> axum::Router {
    axum::Router::new()
        .route("/", axum_get(|| async { "fine" }))
        .route("/echo", post(|body: Bytes| async move { body }))
}

#[tokio::test]
async fn an_oversized_head_is_refused() {
    let authority = Authority::new();
    let running = Running::start(common::server(&authority), echo());
    for version in [Version::HTTP_11, Version::HTTP_2] {
        let mut client = Client::connect(version, running.https, &authority)
            .await
            .unwrap();
        let mut request = get("/");
        let value = HeaderValue::try_from("x".repeat(40_000)).unwrap();
        request.headers_mut().insert("x-large", value);
        let response = client.send(request).await.unwrap();
        let expected = StatusCode::REQUEST_HEADER_FIELDS_TOO_LARGE;
        assert_eq!(response.status(), expected, "{version:?}");
    }
}

#[tokio::test]
async fn a_small_http3_head_that_decodes_to_a_large_one_is_refused() {
    let authority = Authority::new();
    let running = Running::start(common::server(&authority), echo());
    let (_endpoint, _) = common::quic(running.https, &authority).await.unwrap();
    let mut client = common::http3(running.https, &authority).await.unwrap();
    // Each byte is a whole strict-transport-security field.
    let accordion = headers_frame("GET", "/", &[0xfa; 2_000]);
    let response = raw(&client.quic, &accordion).await.unwrap();
    assert!(
        has_status(&response, StatusCode::REQUEST_HEADER_FIELDS_TOO_LARGE),
        "{response:02x?}"
    );
    // The control: a head within the limit, and other requests on the same connection.
    let response = raw(&client.quic, &headers_frame("GET", "/", &[0xfa; 20])).await;
    assert!(has_status(&response.unwrap(), StatusCode::OK));
    let response = common::send_http3(&mut client.send, get("/"))
        .await
        .unwrap();
    assert_eq!(text(response).await.unwrap(), "fine");
}

#[tokio::test]
async fn a_malformed_http3_body_is_not_a_complete_one() {
    let authority = Authority::new();
    let running = Running::start(common::server(&authority), echo());
    let accepted = [
        // Complete bodies, with and without a declared length.
        [
            headers_frame("POST", "/echo", &[]),
            frame(DATA, 5, b"hello"),
        ]
        .concat(),
        [
            headers_frame("POST", "/echo", &content_length("5")),
            frame(DATA, 5, b"hello"),
        ]
        .concat(),
        [
            headers_frame("POST", "/echo", &content_length("5")),
            frame(DATA, 2, b"he"),
            frame(DATA, 3, b"llo"),
        ]
        .concat(),
    ];
    for bytes in accepted {
        let client = common::http3(running.https, &authority).await.unwrap();
        let response = raw(&client.quic, &bytes).await.unwrap();
        assert!(has_status(&response, StatusCode::OK), "{bytes:02x?}");
        // Grease frames may surround the DATA frame.
        assert!(response.windows(5).any(|window| window == b"hello"));
    }
    let malformed = [
        // The stream ends inside a DATA frame: without any of it, and with a part.
        [headers_frame("POST", "/echo", &[]), frame(DATA, 22, b"")].concat(),
        [
            headers_frame("POST", "/echo", &[]),
            frame(DATA, 22, b"hello"),
        ]
        .concat(),
        // The body is shorter and longer than declared.
        [
            headers_frame("POST", "/echo", &content_length("6")),
            frame(DATA, 5, b"hello"),
        ]
        .concat(),
        [
            headers_frame("POST", "/echo", &content_length("4")),
            frame(DATA, 5, b"hello"),
        ]
        .concat(),
        // An empty DATA frame, which h3 0.0.8 takes for the end.
        [
            headers_frame("POST", "/echo", &[]),
            frame(DATA, 0, b""),
            frame(DATA, 5, b"hello"),
        ]
        .concat(),
    ];
    for bytes in malformed {
        let client = common::http3(running.https, &authority).await.unwrap();
        // Either an error response or an HTTP/3 error, but never the echo of a partial body.
        if let Ok(response) = raw(&client.quic, &bytes).await {
            assert!(!has_status(&response, StatusCode::OK), "{response:02x?}");
        }
    }
    // Nothing above harmed the server.
    let mut client = common::http3(running.https, &authority).await.unwrap();
    let response = common::send_http3(&mut client.send, get("/"))
        .await
        .unwrap();
    assert_eq!(text(response).await.unwrap(), "fine");
}

#[tokio::test]
async fn a_malformed_http3_head_does_not_reach_the_application() {
    let authority = Authority::new();
    let running = Running::start(common::server(&authority), echo());
    let mut client = common::http3(running.https, &authority).await.unwrap();
    // transfer-encoding: chunked, with a literal name and value.
    let mut field = vec![0x27, 17 - 7];
    field.extend_from_slice(b"transfer-encoding");
    field.push(7);
    field.extend_from_slice(b"chunked");
    let two_lengths = [content_length("5"), content_length("6")].concat();
    for more in [field, two_lengths, content_length("+5")] {
        let bytes = [
            headers_frame("POST", "/echo", &more),
            frame(DATA, 5, b"hello"),
        ]
        .concat();
        assert!(raw(&client.quic, &bytes).await.is_err(), "{more:02x?}");
    }
    let response = common::send_http3(&mut client.send, get("/"))
        .await
        .unwrap();
    assert_eq!(text(response).await.unwrap(), "fine");
}

#[tokio::test]
async fn a_truncated_http3_body_leaves_its_connection_usable() {
    let authority = Authority::new();
    let running = Running::start(common::server(&authority), echo());
    let mut client = common::http3(running.https, &authority).await.unwrap();
    let truncated = [headers_frame("POST", "/echo", &[]), frame(DATA, 22, b"")].concat();
    let response = raw(&client.quic, &truncated).await.unwrap();
    assert!(
        has_status(&response, StatusCode::BAD_REQUEST),
        "{response:02x?}"
    );
    let response = common::send_http3(&mut client.send, get("/"))
        .await
        .unwrap();
    assert_eq!(text(response).await.unwrap(), "fine");
}

#[tokio::test]
async fn dropping_a_partly_read_http3_body_does_not_fail_the_request() {
    let authority = Authority::new();
    let app = axum::Router::new().fallback(|request: axum::extract::Request| async {
        // Leaves the second piece in h3's buffer, where it hides that a read is in progress.
        let mut body = request.into_body();
        let first = chunk(&mut body).await.unwrap();
        assert_eq!(first, "first");
        "enough"
    });
    let running = Running::start(common::server(&authority), app);
    let client = common::http3(running.https, &authority).await.unwrap();
    let (mut send, mut receive) = client.quic.open_bi().await.unwrap();
    let request = [
        headers_frame("POST", "/", &[]),
        frame(DATA, 5, b"first"),
        frame(DATA, 6, b"second"),
    ];
    send.write_all(&request.concat()).await.unwrap();
    let response = common::soon(receive.read_to_end(1024)).await.unwrap();
    assert!(has_status(&response, StatusCode::OK), "{response:02x?}");
}

#[tokio::test]
async fn a_failed_response_body_is_not_a_complete_one() {
    let authority = Authority::new();
    let app = axum::Router::new().fallback(|| async {
        let (mut sender, body) = channel();
        tokio::spawn(async move {
            sender.send_data("the beginning".into()).await.unwrap();
            // Let the beginning go out on its own.
            tokio::time::sleep(Duration::from_millis(50)).await;
            sender.abort("the source failed".into());
        });
        Response::new(body)
    });
    let running = Running::start(common::server(&authority), app);
    for version in VERSIONS {
        let mut client = Client::connect(version, running.https, &authority)
            .await
            .unwrap();
        let mut body = client.get("/").await.unwrap().into_body();
        assert_eq!(chunk(&mut body).await.unwrap(), "the beginning");
        assert!(text(Response::new(body)).await.is_err(), "{version:?}");
    }
}

#[tokio::test]
async fn a_panicking_handler_fails_only_its_request() {
    async fn panicking() {
        panic!("expected by the test")
    }

    let authority = Authority::new();
    let app = echo().route("/panic", axum_get(panicking));
    let running = Running::start(common::server(&authority), app);
    for version in VERSIONS {
        let mut client = Client::connect(version, running.https, &authority)
            .await
            .unwrap();
        assert!(client.get("/panic").await.is_err(), "{version:?}");
        // HTTP/1.1 has only its connection to report the failure with.
        if version == Version::HTTP_11 {
            client = Client::connect(version, running.https, &authority)
                .await
                .unwrap();
        }
        let response = client.get("/").await.unwrap();
        assert_eq!(text(response).await.unwrap(), "fine", "{version:?}");
    }
}

#[tokio::test]
async fn the_connection_limit_covers_every_protocol() {
    let authority = Authority::new();
    for held in VERSIONS {
        let mut limits = Limits::default();
        limits.connections = 1;
        let server = common::server(&authority).limits(limits);
        let running = Running::start(server, echo());
        let mut client = Client::connect(held, running.https, &authority)
            .await
            .unwrap();
        client.get("/").await.unwrap();
        for refused in VERSIONS {
            let result = Client::connect(refused, running.https, &authority).await;
            assert!(result.is_err(), "{refused:?} beside {held:?}");
        }
        match client {
            Client::Http3(client) => client.close().await,
            tcp => drop(tcp),
        }
        // The slot comes back once the server sees the connection close.
        common::soon(async {
            while Client::connect(held, running.https, &authority)
                .await
                .is_err()
            {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await;
    }
}

#[tokio::test]
async fn stalled_clients_are_disconnected() {
    let authority = Authority::new();
    let mut limits = Limits::default();
    limits.handshake_timeout = Duration::from_millis(200);
    limits.head_timeout = Duration::from_millis(300);
    let running = Running::start(common::server(&authority).limits(limits), echo());

    // No TLS handshake.
    let mut stream = tokio::net::TcpStream::connect(running.https).await.unwrap();
    assert_eq!(common::soon(stream.read(&mut [0; 16])).await.unwrap(), 0);

    // A QUIC handshake without room for the server's control stream.
    let config = authority.client_config(b"h3");
    let config = quinn::crypto::rustls::QuicClientConfig::try_from(config).unwrap();
    let mut config = quinn::ClientConfig::new(std::sync::Arc::new(config));
    let mut transport = quinn::TransportConfig::default();
    transport
        .max_concurrent_uni_streams(0_u8.into())
        .keep_alive_interval(Some(Duration::from_millis(50)));
    config.transport_config(std::sync::Arc::new(transport));
    let endpoint = quinn::Endpoint::client(([127, 0, 0, 1], 0).into()).unwrap();
    let quic = endpoint
        .connect_with(config, running.https, "localhost")
        .unwrap()
        .await
        .unwrap();
    common::soon(quic.closed()).await;

    // A partial HTTP/1.1 head.
    let mut stream = common::tls(running.https, &authority, b"http/1.1")
        .await
        .unwrap();
    stream.write_all(b"GET / HTTP/1.1\r\nHost:").await.unwrap();
    let mut answer = Vec::new();
    let _ = common::soon(stream.read_to_end(&mut answer)).await;
    assert!(!answer.starts_with(b"HTTP/1.1 200"));

    // A partial HTTP/3 head, on a connection that stays usable.
    let mut client = common::http3(running.https, &authority).await.unwrap();
    let (mut send, mut receive) = client.quic.open_bi().await.unwrap();
    let head = headers_frame("GET", "/", &[]);
    send.write_all(&head[..head.len() - 1]).await.unwrap();
    let answer = common::soon(receive.read_to_end(1024)).await;
    assert!(answer.is_err() || answer.unwrap().is_empty());
    let response = common::send_http3(&mut client.send, get("/"))
        .await
        .unwrap();
    assert_eq!(text(response).await.unwrap(), "fine");
}

#[tokio::test]
async fn quic_clients_prove_their_address_when_slots_run_low() {
    let authority = Authority::new();
    let mut limits = Limits::default();
    limits.connections = 4;
    let running = Running::start(common::server(&authority).limits(limits), echo());
    // A long header packet of QUIC version 1 and the type Retry (RFC 9000 section 17.2.5).
    let is_retry = |first: u8| first & 0xf0 == 0xf0;
    let mut held = Vec::new();
    for retried in [false, false, true, true] {
        let (address, reply) = common::first_reply(running.https).await;
        let mut client = common::http3(address, &authority).await.unwrap();
        let response = common::send_http3(&mut client.send, get("/"))
            .await
            .unwrap();
        assert_eq!(text(response).await.unwrap(), "fine");
        assert_eq!(is_retry(reply.await.unwrap()), retried, "{}", held.len());
        held.push(client);
    }
    assert!(common::http3(running.https, &authority).await.is_err());
}

#[tokio::test]
async fn silent_clients_are_disconnected() {
    let authority = Authority::new();
    let mut limits = Limits::default();
    limits.idle_timeout = Duration::from_millis(300);
    let running = Running::start(common::server(&authority).limits(limits), echo());

    let client = common::http3(running.https, &authority).await.unwrap();
    common::soon(client.quic.closed()).await;

    // An HTTP/2 client that sends a request and then answers no pings.
    let mut stream = common::tls(running.https, &authority, b"h2").await.unwrap();
    stream
        .write_all(b"PRI * HTTP/2.0\r\n\r\nSM\r\n\r\n")
        .await
        .unwrap();
    stream
        .write_all(&[0, 0, 0, 4, 0, 0, 0, 0, 0])
        .await
        .unwrap();
    // HEADERS that end the stream: GET, https, / and a literal :authority.
    let head = [0x82, 0x87, 0x84, 0x41, 1, b'a'];
    stream
        .write_all(&[0, 0, head.len() as u8, 1, 5, 0, 0, 0, 1])
        .await
        .unwrap();
    stream.write_all(&head).await.unwrap();
    let mut received = Vec::new();
    let _ = common::soon(stream.read_to_end(&mut received)).await;
    assert!(!received.is_empty());
}

#[tokio::test]
async fn early_data_is_not_accepted() {
    let authority = Authority::new();
    let running = Running::start(common::server(&authority), echo());
    let mut config = authority.client_config(b"h3");
    config.enable_early_data = true;
    let config = quinn::crypto::rustls::QuicClientConfig::try_from(config).unwrap();
    let config = quinn::ClientConfig::new(std::sync::Arc::new(config));
    let endpoint = quinn::Endpoint::client(([127, 0, 0, 1], 0).into()).unwrap();
    let connect = || {
        endpoint
            .connect_with(config.clone(), running.https, "localhost")
            .unwrap()
    };
    // The first connection gets a ticket for the second to resume with.
    let first = common::soon(connect()).await.unwrap();
    let mut settings = common::soon(first.accept_uni()).await.unwrap();
    settings.read_exact(&mut [0; 1]).await.unwrap();
    assert!(connect().into_0rtt().is_err());
}

#[tokio::test]
async fn other_protocols_are_refused() {
    let authority = Authority::new();
    let running = Running::start(common::server(&authority), echo());
    let connector =
        tokio_rustls::TlsConnector::from(std::sync::Arc::new(authority.client_config(b"spdy/3")));
    let stream = tokio::net::TcpStream::connect(running.https).await.unwrap();
    let name = "localhost".try_into().unwrap();
    assert!(connector.connect(name, stream).await.is_err());

    let config = authority.client_config(b"not-http");
    let config = quinn::crypto::rustls::QuicClientConfig::try_from(config).unwrap();
    let endpoint = quinn::Endpoint::client(([127, 0, 0, 1], 0).into()).unwrap();
    let config = quinn::ClientConfig::new(std::sync::Arc::new(config));
    let connecting = endpoint
        .connect_with(config, running.https, "localhost")
        .unwrap();
    assert!(common::soon(connecting).await.is_err());
}

#[tokio::test]
async fn forwarding_fields_do_not_change_the_security_context() {
    let authority = Authority::new();
    let app = axum::Router::new().fallback(
        |axum::Extension(security): axum::Extension<warded::TransportSecurity>| async move {
            format!("{security:?}")
        },
    );
    let server = common::server(&authority).serve_http(([127, 0, 0, 1], 0).into());
    let running = Running::start(server, app);
    let response = common::cleartext(
        running.http.unwrap(),
        "GET / HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\
         X-Forwarded-Proto: https\r\nForwarded: proto=https\r\n\r\n",
    )
    .await;
    assert!(response.ends_with("Unsecure"), "{response}");
    for version in VERSIONS {
        let mut client = Client::connect(version, running.https, &authority)
            .await
            .unwrap();
        let mut request = request("GET", "/", Body::empty());
        let value = HeaderValue::from_static("proto=http");
        request.headers_mut().insert(header::FORWARDED, value);
        let response = client.send(request).await.unwrap();
        assert_eq!(text(response).await.unwrap(), "Https");
    }
}
