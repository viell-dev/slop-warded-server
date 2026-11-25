//! Starting, reloading and stopping.

mod common;

use axum::{http::Version, response::Response, routing::get as axum_get};
use common::{Authority, Client, Running, VERSIONS, channel, chunk, get, text};
use std::{
    net::{TcpListener, UdpSocket},
    time::{Duration, Instant},
};
use tokio::sync::{mpsc, oneshot};
use warded_server::{Error, Server, Tls};

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

    let taken = UdpSocket::bind("127.0.0.1:0").unwrap();
    let address = taken.local_addr().unwrap();
    let result = common::server(&authority).https(address).bind();
    assert_eq!(listener_of(result), "HTTP/3 over UDP");
    TcpListener::bind(address).expect("the TCP listener was closed");
    // Without HTTP/3 the same address is free.
    let server = common::server(&authority).https(address).http3(false);
    assert!(server.bind().is_ok());
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
    let result = Tls::pem_files(authority.key(), authority.key());
    assert!(matches!(
        result,
        Err(Error::Certificate(_) | Error::Read { .. })
    ));
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
