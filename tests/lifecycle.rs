//! Starting, reloading and stopping.

mod common;

use axum::{
    http::Version,
    response::Response,
    routing::{get as axum_get, post as axum_post},
};
use common::{Authority, Client, Running, VERSIONS, channel, chunk, get, text};
use std::{
    net::{TcpListener, UdpSocket},
    time::{Duration, Instant},
};
use tokio::sync::{mpsc, oneshot};
use warded_server::{Error, Limits, Server, Tls};

fn fine() -> axum::Router {
    axum::Router::new().fallback(|| async { "fine" })
}

#[tokio::test]
async fn a_failed_bind_leaves_nothing_bound() {
    let authority = Authority::new();
    let listener_of = |result: Result<_, Error>| match result {
        Err(Error::Bind { listener, .. }) => listener,
        other => panic!("expected a bind failure, got {other:?}"),
    };

    let taken = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = taken.local_addr().unwrap();
    let result = common::server(&authority).https(address).bind();
    assert_eq!(listener_of(result), "HTTPS over TCP");
    let result = common::server(&authority).redirect_http(address).bind();
    assert_eq!(listener_of(result), "HTTP over TCP");

    // Windows can reserve a port for TCP that is free for UDP.
    let (_taken, address) = std::iter::repeat_with(|| UdpSocket::bind("127.0.0.1:0").unwrap())
        .take(2_000)
        .map(|taken| (taken.local_addr().unwrap(), taken))
        .find(|(address, _)| TcpListener::bind(address).is_ok())
        .map(|(address, taken)| (taken, address))
        .expect("there is a UDP port that is free for TCP");
    let result = common::server(&authority).https(address).bind();
    assert_eq!(listener_of(result), "HTTP/3 over UDP");
    TcpListener::bind(address).expect("the TCP listener was closed");
    // Without HTTP/3 the same address is free.
    let server = common::server(&authority).https(address).http3(false);
    assert!(server.bind().is_ok());
}

#[tokio::test]
async fn unusable_limits_are_reported() {
    let authority = Authority::new();
    let mut limits = Limits::default();
    limits.idle_timeout = Duration::ZERO;
    let result = common::server(&authority).limits(limits).bind();
    assert!(matches!(
        result,
        Err(Error::Limit {
            field: "idle_timeout"
        })
    ));
}

#[tokio::test]
async fn an_unusable_certificate_is_reported() {
    let authority = Authority::new();
    let missing = authority.chain().with_file_name("missing.pem");
    let error = Tls::pem_files(&missing, authority.key()).unwrap_err();
    assert!(error.to_string().contains("missing.pem"), "{error}");
    let error = Tls::pem_files(authority.chain(), &missing).unwrap_err();
    assert!(error.to_string().contains("missing.pem"), "{error}");
    // A file without a certificate.
    let error = Tls::pem_files(authority.key(), authority.key()).unwrap_err();
    assert!(error.to_string().contains("key.pem"), "{error}");
    // A key that belongs to another certificate.
    let other = Authority::new();
    let result = Tls::pem_files(authority.chain(), other.key());
    assert!(matches!(result, Err(Error::Certificate(_))));
}

#[tokio::test]
async fn new_connections_present_a_reloaded_certificate() {
    let authority = Authority::new();
    let tls = authority.tls();
    let server = Server::new(tls.clone()).https(([127, 0, 0, 1], 0).into());
    let running = Running::start(server, fine());
    let presented = async || {
        let mut certificates = Vec::new();
        for version in [Version::HTTP_2, Version::HTTP_3] {
            certificates.push(common::certificate(version, running.https, &authority).await);
        }
        assert_eq!(certificates[0], certificates[1]);
        certificates.remove(0)
    };
    let mut established = Vec::new();
    for version in VERSIONS {
        let client = Client::connect(version, running.https, &authority).await;
        established.push(client.unwrap());
    }

    let first = presented().await;
    authority.issue(2);
    assert_eq!(presented().await, first, "not reloaded yet");
    tls.reload().unwrap();
    let second = presented().await;
    assert_ne!(second, first);

    // A failed reload keeps the certificate in use.
    std::fs::write(authority.key(), "not a key").unwrap();
    assert!(tls.reload().is_err());
    assert_eq!(presented().await, second);

    for client in &mut established {
        let response = client.get("/").await.unwrap();
        assert_eq!(text(response).await.unwrap(), "fine");
    }
}

#[tokio::test]
async fn requests_in_progress_finish_during_shutdown() {
    let authority = Authority::new();
    for version in VERSIONS {
        let (started, mut has_started) = mpsc::channel(1);
        let app = axum::Router::new().fallback(move || {
            let started = started.clone();
            async move {
                let (mut sender, body) = channel();
                let (release, released) = oneshot::channel::<()>();
                started.send(release).await.unwrap();
                tokio::spawn(async move {
                    sender.send_data("first,".into()).await.unwrap();
                    released.await.unwrap();
                    for _ in 0..100 {
                        sender.send_data(vec![b'x'; 10_000].into()).await.unwrap();
                    }
                    sender.send_data(",last".into()).await.unwrap();
                });
                Response::new(body)
            }
        });
        let mut running = Running::start(common::server(&authority), app);
        let mut client = Client::connect(version, running.https, &authority)
            .await
            .unwrap();
        let mut body = client.get("/").await.unwrap().into_body();
        assert_eq!(chunk(&mut body).await.unwrap(), "first,");
        let release = has_started.recv().await.unwrap();

        running.stop();
        tokio::time::sleep(Duration::from_millis(100)).await;
        for refused in VERSIONS {
            // Once QUIC has drained, its port is closed and attempts go unanswered.
            let connection = Client::connect(refused, running.https, &authority);
            let connection = tokio::time::timeout(Duration::from_millis(500), connection).await;
            assert!(
                !matches!(connection, Ok(Ok(_))),
                "{refused:?} during shutdown"
            );
            if [version, refused] == [Version::HTTP_3; 2] {
                let answered = matches!(connection, Ok(Err(_)));
                assert!(answered, "QUIC refuses connections while it drains");
            }
        }
        // The client was told, so it can repeat a new request elsewhere. HTTP/1.1 has one request
        // at a time on a connection.
        if version != Version::HTTP_11 {
            let response = client.get("/").await;
            assert!(response.is_err(), "{version:?} during shutdown");
        }
        assert!(!running.task.is_finished());

        release.send(()).unwrap();
        let rest = text(Response::new(body)).await.unwrap();
        assert_eq!(rest.len(), 100 * 10_000 + ",last".len(), "{version:?}");
        assert!(rest.ends_with(",last"));
        running.ended().await.unwrap();
    }
}

#[tokio::test]
async fn a_body_that_is_not_read_can_be_finished_during_shutdown() {
    let authority = Authority::new();
    // One body is dropped before the response, and the other after it, during the shutdown.
    let app = fine().route(
        "/later",
        axum_post(|request: axum::extract::Request| async {
            tokio::spawn(async move {
                tokio::time::sleep(Duration::from_secs(1)).await;
                drop(request);
            });
        }),
    );
    let mut running = Running::start(common::server(&authority), app);
    let client = common::http3(running.https, &authority).await.unwrap();
    let mut streams = Vec::new();
    for path in ["/", "/later"] {
        let (mut send, mut receive) = client.quic.open_bi().await.unwrap();
        send.write_all(&common::headers_frame("POST", path, &[]))
            .await
            .unwrap();
        let response = common::soon(receive.read_to_end(1024)).await.unwrap();
        assert!(!response.is_empty());
        streams.push(send);
    }

    running.stop();
    // Longer than the server waits when it has nothing to send.
    let data = common::frame(common::DATA, 1_000, &[0; 1_000]);
    for _ in 0..3 {
        streams[0].write_all(&data).await.unwrap();
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    streams[0].finish().unwrap();
    assert_eq!(common::soon(streams[0].stopped()).await.unwrap(), None);
    for _ in 0..8 {
        streams[1].write_all(&data).await.unwrap();
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    streams[1].finish().unwrap();
    assert_eq!(common::soon(streams[1].stopped()).await.unwrap(), None);
    running.ended().await.unwrap();
}

/// Small enough for Quinn to take at once, and large enough to spend seconds on the slow link.
const SLOW_RESPONSE: usize = 600_000;

#[tokio::test]
async fn closing_waits_for_a_response_on_a_slow_link() {
    let authority = Authority::new();
    for goodbye in [true, false] {
        let app = axum::Router::new().fallback(|| async { vec![b'x'; SLOW_RESPONSE] });
        let mut running = Running::start(common::server(&authority), app);
        let address = common::slow_link(running.https, 150_000).await;
        let (_endpoint, quic) = common::quic(address, &authority).await.unwrap();
        let transport = h3_quinn::Connection::new(quic.clone());
        let (mut connection, mut send) = h3::client::new(transport).await.unwrap();
        let mut stream = send.send_request(get("/").map(|_| ())).await.unwrap();
        stream.finish().await.unwrap();
        common::soon(stream.recv_response()).await.unwrap();
        // Either side can announce the end.
        if goodbye {
            connection.shutdown(0).await.unwrap();
        } else {
            running.stop();
        }
        let driver =
            tokio::spawn(async move { std::future::poll_fn(|cx| connection.poll_close(cx)).await });
        let mut length = 0;
        while let Some(data) = common::soon(stream.recv_data()).await.unwrap() {
            length += bytes::Buf::remaining(&data);
        }
        assert_eq!(length, SLOW_RESPONSE);
        // The server does not wait for longer than it has to.
        common::soon(quic.closed()).await;
        driver.abort();
    }
}

#[tokio::test]
async fn the_goodbye_does_not_disown_requests_that_are_served() {
    let authority = Authority::new();
    let (started, mut has_started) = mpsc::channel(8);
    let app = axum::Router::new().fallback(move || {
        let started = started.clone();
        async move {
            let (release, released) = oneshot::channel::<()>();
            started.send(release).await.unwrap();
            released.await.unwrap();
            "served"
        }
    });
    let mut running = Running::start(common::server(&authority), app);
    let (_endpoint, quic) = common::quic(running.https, &authority).await.unwrap();
    let mut requests = Vec::new();
    for _ in 0..2 {
        let (mut send, receive) = quic.open_bi().await.unwrap();
        send.write_all(&common::headers_frame("GET", "/", &[]))
            .await
            .unwrap();
        send.finish().unwrap();
        requests.push((send.id(), receive));
    }
    let releases = [
        common::soon(has_started.recv()).await.unwrap(),
        common::soon(has_started.recv()).await.unwrap(),
    ];
    running.stop();

    // The control stream ends with the GOAWAY frame: its type, its length and a request's number.
    let mut control = common::soon(async {
        loop {
            let mut stream = quic.accept_uni().await.unwrap();
            let mut kind = [0];
            stream.read_exact(&mut kind).await.unwrap();
            if kind == [0] {
                break stream;
            }
        }
    })
    .await;
    let mut received = Vec::new();
    let number = common::soon(async {
        loop {
            received.push(0);
            let last = received.len() - 1;
            control.read_exact(&mut received[last..]).await.unwrap();
            if let [.., 7, 1, number] = received[..] {
                break u64::from(number);
            }
        }
    })
    .await;
    for (id, _) in &requests {
        assert!(
            u64::from(*id) < number,
            "{id} is served and GOAWAY says {number}"
        );
    }

    // Requests from that number on are refused in a way that allows repeating them.
    let (mut send, mut refused) = quic.open_bi().await.unwrap();
    assert_eq!(u64::from(send.id()), number);
    send.write_all(&common::headers_frame("GET", "/", &[]))
        .await
        .unwrap();
    let error = common::soon(refused.read_to_end(1024)).await.unwrap_err();
    let rejected = h3::error::Code::H3_REQUEST_REJECTED.value();
    assert!(
        matches!(error, quinn::ReadToEndError::Read(quinn::ReadError::Reset(code))
            if code.into_inner() == rejected),
        "{error:?}"
    );

    for release in releases {
        release.send(()).unwrap();
    }
    for (_, mut receive) in requests {
        let response = common::soon(receive.read_to_end(1024)).await.unwrap();
        assert!(response.windows(6).any(|window| window == b"served"));
    }
    quic.close(0x100_u32.into(), b"done");
    running.ended().await.unwrap();
}

#[tokio::test]
async fn websockets_end_with_the_server() {
    use axum::extract::ws::WebSocketUpgrade;
    use futures_util::StreamExt;

    let authority = Authority::new();
    let (ended, mut has_ended) = mpsc::channel::<()>(1);
    let app = axum::Router::new().fallback(axum::routing::any(move |upgrade: WebSocketUpgrade| {
        let ended = ended.clone();
        async {
            upgrade.on_upgrade(|mut socket| async move {
                while let Some(Ok(_)) = socket.recv().await {}
                ended.send(()).await.unwrap();
            })
        }
    }));
    let mut running = Running::start(common::server(&authority), app);
    let stream = common::tls(running.https, &authority, b"http/1.1")
        .await
        .unwrap();
    let (mut socket, _) = tokio_tungstenite::client_async("wss://localhost/ws", stream)
        .await
        .unwrap();
    running.stop();
    running.ended().await.unwrap();
    common::soon(has_ended.recv()).await.unwrap();
    assert!(!matches!(common::soon(socket.next()).await, Some(Ok(_))));
}

#[tokio::test]
async fn idle_connections_do_not_delay_shutdown() {
    let authority = Authority::new();
    let mut running = Running::start(common::server(&authority), fine());
    let mut idle = Vec::new();
    for version in VERSIONS {
        let mut client = Client::connect(version, running.https, &authority)
            .await
            .unwrap();
        text(client.get("/").await.unwrap()).await.unwrap();
        idle.push(client);
    }
    let began = Instant::now();
    running.stop();
    running.ended().await.unwrap();
    assert!(
        began.elapsed() < Duration::from_secs(8),
        "{:?}",
        began.elapsed()
    );
}

#[tokio::test]
async fn the_grace_period_ends_requests_that_do_not() {
    let authority = Authority::new();
    for version in VERSIONS {
        let (started, mut has_started) = mpsc::channel::<()>(1);
        let app = axum::Router::new().route(
            "/",
            axum_get(move || {
                let started = started.clone();
                async move {
                    started.send(()).await.unwrap();
                    std::future::pending::<()>().await;
                }
            }),
        );
        let server = common::server(&authority).grace_period(Duration::from_millis(300));
        let mut running = Running::start(server, app);
        let mut client = Client::connect(version, running.https, &authority)
            .await
            .unwrap();
        let waiting = tokio::spawn(async move { client.get("/").await });
        common::soon(has_started.recv()).await.unwrap();
        let began = Instant::now();
        running.stop();
        running.ended().await.unwrap();
        assert!(began.elapsed() >= Duration::from_millis(300));
        assert!(began.elapsed() < Duration::from_secs(5));
        let result = waiting.await.unwrap();
        assert!(result.is_err(), "{version:?}");
    }
}

#[tokio::test]
async fn dropping_the_server_closes_everything() {
    let authority = Authority::new();
    let running = Running::start(common::server(&authority), fine());
    let address = running.https;
    let mut clients = Vec::new();
    for version in VERSIONS {
        let mut client = Client::connect(version, address, &authority).await.unwrap();
        text(client.get("/").await.unwrap()).await.unwrap();
        clients.push(client);
    }
    drop(running);
    common::soon(async {
        while TcpListener::bind(address).is_err() || UdpSocket::bind(address).is_err() {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await;
    for mut client in clients {
        assert!(client.send(get("/")).await.is_err());
    }
}
