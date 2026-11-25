//! Clients and applications that misbehave.

mod common;

use axum::{
    body::{Body, Bytes},
    http::{HeaderValue, StatusCode, Version, header},
    response::Response,
    routing::{get as axum_get, post},
};
use common::{
    Authority, BoxError, Client, DATA, Running, UNKNOWN, VERSIONS, channel, chunk, frame, get,
    headers_frame, request, text,
};
use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    sync::{mpsc, watch},
    task::JoinSet,
};
use warded_server::Limits;

/// `H3_NO_ERROR` and `H3_EXCESSIVE_LOAD` (RFC 9114 section 8.1).
const NO_ERROR: quinn::VarInt = quinn::VarInt::from_u32(0x100);
const EXCESSIVE_LOAD: quinn::VarInt = quinn::VarInt::from_u32(0x107);

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
        // Empty DATA frames, which h3 0.0.8 takes for the end.
        [
            headers_frame("POST", "/echo", &content_length("5")),
            frame(DATA, 0, b""),
            frame(DATA, 5, b"hello"),
            frame(DATA, 0, b""),
        ]
        .concat(),
        // Frames that mean nothing, which are larger than any limit.
        [
            frame(UNKNOWN, 3_000_000, &[0; 3_000_000]),
            headers_frame("POST", "/echo", &content_length("5")),
            frame(UNKNOWN, 3_000_000, &[0; 3_000_000]),
            frame(DATA, 5, b"hello"),
            frame(UNKNOWN, 3_000_000, &[0; 3_000_000]),
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
        // The stream ends inside a frame that means nothing.
        [
            headers_frame("POST", "/echo", &[]),
            frame(DATA, 5, b"hello"),
            frame(UNKNOWN, 5, b"rest"),
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
        // Leaves a read in progress.
        let mut body = request.into_body();
        let first = chunk(&mut body).await.unwrap();
        assert!(first.starts_with(b"first"));
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
    // The client has not finished, and is told that it need not.
    let code = common::soon(send.stopped()).await.unwrap();
    assert_eq!(code, Some(NO_ERROR));
}

/// Send the beginning of a frame that declares a length beyond any limit, and return the code with
/// which the server stops it.
async fn stopped(quic: &quinn::Connection, before: &[u8], kind: u8) -> Option<quinn::VarInt> {
    let (mut send, _receive) = quic.open_bi().await.unwrap();
    send.write_all(before).await.unwrap();
    send.write_all(&frame(kind, 1 << 29, &[0; 1_000]))
        .await
        .unwrap();
    common::soon(send.stopped()).await.unwrap()
}

#[tokio::test]
async fn an_http3_frame_that_declares_too_much_is_refused_before_it_arrives() {
    const HEADERS: u8 = 1;
    const SETTINGS: u8 = 4;

    let authority = Authority::new();
    let running = Running::start(common::server(&authority), echo());
    let mut client = common::http3(running.https, &authority).await.unwrap();
    // A head, and trailers after a body.
    let head = headers_frame("POST", "/echo", &[]);
    let body = [head, frame(DATA, 5, b"hello")].concat();
    for before in [&[][..], &body] {
        let code = stopped(&client.quic, before, HEADERS).await;
        assert_eq!(code, Some(EXCESSIVE_LOAD));
    }
    let response = common::send_http3(&mut client.send, get("/"))
        .await
        .unwrap();
    assert_eq!(text(response).await.unwrap(), "fine");

    // A control stream, whose connection fails with it.
    let (_endpoint, quic) = common::quic(running.https, &authority).await.unwrap();
    let mut control = quic.open_uni().await.unwrap();
    let settings = frame(SETTINGS, 1 << 29, &[0; 1_000]);
    let beginning = [&[0][..], &settings].concat();
    control.write_all(&beginning).await.unwrap();
    common::soon(quic.closed()).await;
}

#[tokio::test]
async fn a_head_within_twice_the_limit_gets_a_response() {
    let authority = Authority::new();
    let mut limits = Limits::default();
    limits.head_bytes = 1_000;
    let running = Running::start(common::server(&authority).limits(limits), echo());
    let client = common::http3(running.https, &authority).await.unwrap();
    // x: a literal name and a value of three times 126 bytes.
    let field = [&[0x21, b'x', 126][..], &[b'x'; 126]].concat();
    for (fields, status) in [
        (2, StatusCode::OK),
        (10, StatusCode::REQUEST_HEADER_FIELDS_TOO_LARGE),
    ] {
        let head = headers_frame("GET", "/", &field.repeat(fields));
        let response = raw(&client.quic, &head).await.unwrap();
        assert!(has_status(&response, status), "{response:02x?}");
    }
    let head = headers_frame("GET", "/", &field.repeat(20));
    assert!(raw(&client.quic, &head).await.is_err());
}

/// Send on all streams until the server does not let the client send more, and count the bytes.
async fn send_until_full(streams: Vec<quinn::SendStream>, data: Bytes) -> (usize, JoinSet<()>) {
    let sent = Arc::new(AtomicUsize::new(0));
    let mut sending = JoinSet::new();
    for mut stream in streams {
        let (data, sent) = (data.clone(), sent.clone());
        sending.spawn(async move {
            let mut rest = &data[..];
            while !rest.is_empty() {
                let written = stream.write(rest).await.unwrap();
                rest = &rest[written..];
                sent.fetch_add(written, Ordering::Relaxed);
            }
            // Ending it would let the handler go on.
            std::future::pending::<()>().await;
        });
    }
    common::soon(async {
        let mut before = 0;
        loop {
            tokio::time::sleep(Duration::from_secs(1)).await;
            match sent.load(Ordering::Relaxed) {
                now if now == before => return (now, sending),
                now => before = now,
            }
        }
    })
    .await
}

#[tokio::test]
async fn unread_http3_requests_do_not_fill_the_memory() {
    let authority = Authority::new();
    let app = axum::Router::new().fallback(|request: axum::extract::Request| async {
        let _unread = request;
        std::future::pending::<()>().await;
    });
    let running = Running::start(common::server(&authority), app);
    let client = common::http3(running.https, &authority).await.unwrap();
    // Nothing reads what DATAGRAM frames would bring.
    assert_eq!(client.quic.max_datagram_size(), None);

    let (mut streams, mut responses) = (Vec::new(), Vec::new());
    for _ in 0..12 {
        let (mut send, receive) = client.quic.open_bi().await.unwrap();
        send.write_all(&headers_frame("POST", "/", &[]))
            .await
            .unwrap();
        streams.push(send);
        responses.push(receive);
    }
    let data = frame(DATA, 1 << 29, &[0; 1_200_000]);
    let (sent, _sending) = send_until_full(streams, data.into()).await;
    // All streams together hold 10 MB of these 14 MB.
    assert!((8_000_000..11_000_000).contains(&sent), "{sent}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn http3_requests_in_small_packets_do_not_fill_the_memory() {
    const STREAMS: usize = 8;

    let authority = Authority::new();
    let (release, released) = watch::channel(false);
    let (report, mut reports) = mpsc::unbounded_channel();
    let app = axum::Router::new().fallback(move |request: axum::extract::Request| {
        let (mut released, report) = (released.clone(), report.clone());
        async move {
            let mut body = request.into_body();
            released.wait_for(|released| *released).await.unwrap();
            report.send(chunk(&mut body).await.unwrap().len()).unwrap();
            std::future::pending::<()>().await;
        }
    });
    let running = Running::start(common::server(&authority), app);
    let client = common::http3(running.https, &authority).await.unwrap();

    // Quinn keeps the content of many small packets as one piece.
    let data = frame(DATA, 60, &[0; 60]).repeat(8);
    let (mut streams, mut responses, mut sent) = (Vec::new(), Vec::new(), 0);
    for _ in 0..STREAMS {
        let (mut send, receive) = client.quic.open_bi().await.unwrap();
        send.write_all(&headers_frame("POST", "/", &[]))
            .await
            .unwrap();
        for _ in 0..2_100 {
            let frames = client.quic.stats().frame_tx.stream;
            send.write_all(&data).await.unwrap();
            sent += data.len();
            while client.quic.stats().frame_tx.stream == frames {
                tokio::task::yield_now().await;
            }
        }
        streams.push(send);
        responses.push(receive);
    }
    tokio::time::sleep(Duration::from_millis(500)).await;

    // Reading a little must not make room for a lot.
    release.send(true).unwrap();
    let mut read = 0;
    for _ in 0..STREAMS {
        read += common::soon(reports.recv()).await.unwrap();
    }
    let data = frame(DATA, 1 << 29, &[0; 3_000_000]);
    let (more, _sending) = send_until_full(streams, data.into()).await;
    let unread = sent + more - read;
    assert!((8_000_000..11_000_000).contains(&unread), "{unread}");
}

#[tokio::test]
async fn a_piece_of_an_http3_body_does_not_keep_its_packet() {
    let authority = Authority::new();
    let app = axum::Router::new().fallback(|request: axum::extract::Request| async {
        let mut body = request.into_body();
        let piece = chunk(&mut body).await.unwrap();
        drop(body);
        let memory = piece.try_into_mut().unwrap().capacity();
        assert!(memory <= 64, "{memory}");
    });
    let running = Running::start(common::server(&authority), app);
    let client = common::http3(running.https, &authority).await.unwrap();
    let request = [
        headers_frame("POST", "/", &[]),
        frame(DATA, 1, b"a"),
        frame(UNKNOWN, 1_000, &[0; 1_000]),
    ];
    let response = raw(&client.quic, &request.concat()).await.unwrap();
    assert!(has_status(&response, StatusCode::OK), "{response:02x?}");
}

#[tokio::test]
async fn an_http3_client_that_abandons_request_heads_is_disconnected() {
    let authority = Authority::new();
    let (mut sender, body) = channel();
    let body = Arc::new(Mutex::new(Some(body)));
    let stream = move || {
        let body = body.lock().unwrap().take().unwrap();
        async { Response::new(body) }
    };
    let app = echo().route("/stream", axum_get(stream));
    let running = Running::start(common::server(&authority), app);
    let mut client = common::http3(running.https, &authority).await.unwrap();
    let abandon = async |count: usize| {
        for _ in 0..count {
            let (mut send, receive) = client.quic.open_bi().await?;
            send.write_all(&[1]).await.unwrap();
            send.reset(0x10c_u32.into()).unwrap();
            drop(receive);
        }
        Ok::<_, quinn::ConnectionError>(())
    };
    common::soon(abandon(900)).await.unwrap();
    let response = common::send_http3(&mut client.send, get("/stream"))
        .await
        .unwrap();
    let mut body = response.into_body();

    // The connection ends like at a shutdown.
    let _ = common::soon(abandon(200)).await;
    sender.send_data("the beginning".into()).await.unwrap();
    assert_eq!(chunk(&mut body).await.unwrap(), "the beginning");
    drop(sender);
    assert_eq!(text(Response::new(body)).await.unwrap(), "");
    let reason = common::soon(client.quic.closed()).await;
    let quinn::ConnectionError::ApplicationClosed(reason) = reason else {
        panic!("{reason:?}");
    };
    assert_eq!(reason.error_code, NO_ERROR);
}

#[tokio::test]
async fn an_http3_connection_outlives_requests_that_are_answered_without_the_application() {
    let authority = Authority::new();
    let running = Running::start(common::server(&authority), echo());
    let mut client = common::http3(running.https, &authority).await.unwrap();
    let oversized = headers_frame("GET", "/", &[0xfa; 2_000]);
    // A field name in upper case.
    let malformed = headers_frame("GET", "/", &[0x21, b'X', 1, b'x']);
    for _ in 0..1_001 {
        let response = raw(&client.quic, &oversized).await.unwrap();
        let expected = StatusCode::REQUEST_HEADER_FIELDS_TOO_LARGE;
        assert!(has_status(&response, expected), "{response:02x?}");
        assert!(raw(&client.quic, &malformed).await.is_err());
    }
    let response = common::send_http3(&mut client.send, get("/"))
        .await
        .unwrap();
    assert_eq!(text(response).await.unwrap(), "fine");
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
async fn a_client_that_still_sends_gets_the_whole_response() {
    const LENGTH: usize = 1_000_000;

    let authority = Authority::new();
    let app = axum::Router::new().fallback(|| async { vec![b'x'; LENGTH] });
    let server = common::server(&authority).serve_http(([127, 0, 0, 1], 0).into());
    let running = Running::start(server, app);
    let mut stream = tokio::net::TcpStream::connect(running.http.unwrap())
        .await
        .unwrap();
    stream
        .write_all(b"GET / HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
        .await
        .unwrap();
    // The system takes what it can of the response, and the server does not read this.
    tokio::time::sleep(Duration::from_millis(200)).await;
    stream.write_all(&[b'x'; 100_000]).await.unwrap();
    let mut response = Vec::new();
    loop {
        tokio::time::sleep(Duration::from_millis(5)).await;
        let mut piece = [0; 10_000];
        match common::soon(stream.read(&mut piece)).await.unwrap() {
            0 => break,
            length => response.extend_from_slice(&piece[..length]),
        }
    }
    let end_of_head = response.windows(4).position(|end| end == b"\r\n\r\n");
    let content = &response[end_of_head.unwrap() + 4..];
    assert!(content == [b'x'; LENGTH], "{}", content.len());
    drop(stream);
    common::soon(async {
        let mut client = Client::connect(Version::HTTP_11, running.https, &authority)
            .await
            .unwrap();
        client.get("/").await.unwrap();
    })
    .await;
}

#[tokio::test]
async fn a_client_that_does_not_close_is_disconnected() {
    let authority = Authority::new();
    let mut limits = Limits::default();
    limits.connections = 1;
    let server = common::server(&authority)
        .limits(limits)
        .serve_http(([127, 0, 0, 1], 0).into());
    let running = Running::start(server, echo());
    let connect = || tokio::net::TcpStream::connect(running.http.unwrap());
    let mut stream = connect().await.unwrap();
    let request = b"GET / HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n";
    stream.write_all(request).await.unwrap();
    let mut response = Vec::new();
    common::soon(stream.read_to_end(&mut response))
        .await
        .unwrap();
    assert!(response.starts_with(b"HTTP/1.1 200"));
    // Its slot comes back although it keeps its side open.
    common::soon(async {
        loop {
            let mut next = connect().await.unwrap();
            let mut response = Vec::new();
            let answered = next.write_all(request).await.is_ok()
                && next.read_to_end(&mut response).await.is_ok()
                && response.starts_with(b"HTTP/1.1 200");
            if answered {
                return;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    })
    .await;
    drop(stream);
}

#[tokio::test]
async fn a_response_that_does_not_match_its_length_is_not_a_complete_one() {
    let authority = Authority::new();
    let app = axum::Router::new()
        .route(
            "/short",
            axum_get(|| async { ([(header::CONTENT_LENGTH, "5")], stream()) }),
        )
        .route(
            "/long",
            axum_get(|| async { ([(header::CONTENT_LENGTH, "2")], stream()) }),
        )
        .route(
            "/right",
            axum_get(|| async { ([(header::CONTENT_LENGTH, "3")], stream()) }),
        );
    fn stream() -> Body {
        let (mut sender, body) = channel();
        tokio::spawn(async move { sender.send_data("abc".into()).await });
        body
    }
    let running = Running::start(common::server(&authority), app);
    for version in VERSIONS {
        for (path, complete) in [("/short", false), ("/long", false), ("/right", true)] {
            let mut client = Client::connect(version, running.https, &authority)
                .await
                .unwrap();
            let received = match client.get(path).await {
                Ok(response) => text(response).await.ok(),
                Err(_) => None,
            };
            let expected = complete.then(|| "abc".to_owned());
            // Hyper cuts a longer HTTP/1.1 body off at its length, which is a complete response.
            if (version, path) != (Version::HTTP_11, "/long") {
                assert_eq!(received, expected, "{version:?} {path}");
            }
        }
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
    limits.handshake_timeout = Duration::from_secs(1);
    limits.head_timeout = Duration::from_secs(1);
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
