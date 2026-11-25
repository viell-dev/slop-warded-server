//! Signals, which reach the whole process.
#![cfg(unix)]

mod common;

use common::{Authority, Client, text};
use http::Version;
use std::{net::SocketAddr, process::Command, time::Duration};

fn signal(name: &str) {
    let process = std::process::id().to_string();
    let status = Command::new("kill").args([name, &process]).status();
    assert!(status.unwrap().success());
}

/// One test after the other, because signals reach both.
static SIGNALS: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

#[tokio::test]
async fn a_second_signal_ends_the_grace_period() {
    let _signals = SIGNALS.lock().await;
    let authority = Authority::new();
    let bound = common::server(&authority).bind().unwrap();
    let address: SocketAddr = bound.https_addr();
    drop(bound);

    let app = axum::Router::new().fallback(std::future::pending::<()>);
    let server = common::server(&authority).https(address);
    let mut serving = tokio::spawn(server.serve(app));
    let mut client = common::soon(async {
        loop {
            match Client::connect(Version::HTTP_2, address, &authority).await {
                Ok(client) => break client,
                Err(_) => tokio::time::sleep(Duration::from_millis(20)).await,
            }
        }
    })
    .await;
    tokio::spawn(async move { client.get("/").await });
    tokio::time::sleep(Duration::from_millis(200)).await;

    signal("-INT");
    let waiting = tokio::time::timeout(Duration::from_millis(500), &mut serving).await;
    assert!(
        waiting.is_err(),
        "the request in progress has time to finish"
    );
    signal("-INT");
    // Well before the client gives up on its request, which would also end the wait.
    let ended = tokio::time::timeout(Duration::from_secs(5), serving).await;
    ended
        .expect("the second signal ends the wait")
        .unwrap()
        .unwrap();
}

#[tokio::test]
async fn hangup_reloads_the_certificate_and_terminate_stops() {
    let _signals = SIGNALS.lock().await;
    let authority = Authority::new();
    let bound = common::server(&authority).bind().unwrap();
    let address: SocketAddr = bound.https_addr();
    drop(bound);

    let app = axum::Router::new().fallback(|| async { "fine" });
    let server = common::server(&authority).https(address);
    let serving = tokio::spawn(server.serve(app));
    let mut client = common::soon(async {
        loop {
            match Client::connect(Version::HTTP_2, address, &authority).await {
                Ok(client) => break client,
                Err(_) => tokio::time::sleep(Duration::from_millis(20)).await,
            }
        }
    })
    .await;
    text(client.get("/").await.unwrap()).await.unwrap();
    drop(client);

    let first = common::certificate(Version::HTTP_3, address, &authority).await;
    authority.issue(2);
    signal("-HUP");
    common::soon(async {
        while common::certificate(Version::HTTP_3, address, &authority).await == first {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await;

    signal("-TERM");
    common::soon(serving).await.unwrap().unwrap();
}

#[tokio::test]
async fn hangup_is_harmless_without_certificate_files() {
    let _signals = SIGNALS.lock().await;
    let authority = Authority::new();
    let bound = common::server(&authority).bind().unwrap();
    let address: SocketAddr = bound.https_addr();
    drop(bound);

    let app = axum::Router::new().fallback(|| async { "fine" });
    let tls = warded_server::Tls::self_signed(["localhost"]).unwrap();
    let serving = tokio::spawn(warded_server::Server::new(tls).https(address).serve(app));
    common::soon(async {
        while tokio::net::TcpStream::connect(address).await.is_err() {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await;
    // Unhandled, this would end the test run.
    signal("-HUP");
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(!serving.is_finished());
    signal("-TERM");
    common::soon(serving).await.unwrap().unwrap();
}
