//! Signals to the whole process, so this binary holds one test.
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

#[tokio::test]
async fn hangup_reloads_the_certificate_and_terminate_stops() {
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
