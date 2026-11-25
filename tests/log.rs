//! What the server logs. A test binary of its own, because `tracing` can miss events for a
//! subscriber of one thread while other tests register the same callsites.

mod common;

use axum::{
    http::{StatusCode, Version},
    routing::get,
};
use common::{Authority, Client, Running};
use std::{
    io,
    net::{Ipv4Addr, SocketAddr},
    sync::{Arc, Mutex},
};

fn site() -> axum::Router {
    axum::Router::new().route("/", get(|| async { "Hello" }))
}

/// What the server logs while the closure runs, without colours.
async fn logged<F: Future>(run: impl FnOnce() -> F) -> String {
    #[derive(Clone, Default)]
    struct Log(Arc<Mutex<Vec<u8>>>);

    impl io::Write for Log {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(bytes);
            Ok(bytes.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    let log = Log::default();
    let writer = log.clone();
    let subscriber = tracing_subscriber::fmt()
        .with_ansi(false)
        .with_writer(move || writer.clone())
        .finish();
    // The test's runtime has one thread, which runs the server's tasks too.
    let _default = tracing::subscriber::set_default(subscriber);
    run().await;
    String::from_utf8(log.0.lock().unwrap().clone()).unwrap()
}

#[tokio::test]
async fn the_log_says_what_to_open() {
    let authority = Authority::new();
    let mut ports = None;
    let log = logged(|| async {
        let development = SocketAddr::from((Ipv4Addr::LOCALHOST, 0));
        let running = Running::start(common::server(&authority).development(development), site());
        // The server has logged once it answers.
        let address = running.development.unwrap();
        let response = Client::plain(address).await.unwrap().get("/").await;
        assert_eq!(response.unwrap().status(), StatusCode::OK);
        ports = Some((address, running.https));
    })
    .await;
    let (address, https) = ports.unwrap();
    let serving = format!("serving https={https} development={address} http3=true");
    assert!(log.contains(&serving), "{log}");
    let open = format!("open http://{address}/ in a browser");
    assert!(log.contains(&open), "{log}");

    // Without a development address, HTTPS on this computer is the one to open.
    let mut https = None;
    let log = logged(|| async {
        let running = Running::start(common::server(&authority), site());
        let mut client = Client::connect(Version::HTTP_2, running.https, &authority)
            .await
            .unwrap();
        assert_eq!(client.get("/").await.unwrap().status(), StatusCode::OK);
        https = Some(running.https);
    })
    .await;
    let open = format!(
        "open https://localhost:{}/ in a browser",
        https.unwrap().port()
    );
    assert!(log.contains(&open), "{log}");
}
