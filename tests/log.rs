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

#[tokio::test]
async fn a_length_that_the_content_does_not_have_is_logged() {
    let authority = Authority::new();
    let app = axum::Router::new().route(
        "/",
        get(|| async { ([(axum::http::header::CONTENT_LENGTH, "6")], "hello") }),
    );
    let log = logged(|| async {
        let running = Running::start(common::server(&authority), app);
        let mut client = Client::connect(Version::HTTP_2, running.https, &authority)
            .await
            .unwrap();
        let response = client.get("/").await.unwrap();
        assert_eq!(response.headers()["content-length"], "5");
    })
    .await;
    let warning = "the application declared a Content-Length that its content does not have \
                   declared=[\"6\"] length=5";
    assert!(log.contains(warning), "{log}");
}

#[tokio::test]
async fn an_invalid_length_is_logged() {
    use axum::{
        body::Body,
        http::{HeaderValue, header::CONTENT_LENGTH},
        response::Response,
    };

    let authority = Authority::new();
    // Content of unknown length, whose two lengths cannot both be right.
    let app = tower::service_fn(|_: axum::extract::Request| async {
        let (mut sender, body) = common::channel();
        tokio::spawn(async move { sender.send_data("hello".into()).await });
        let mut response = Response::new(Body::new(body));
        for length in ["5", "6"] {
            let length = HeaderValue::from_static(length);
            response.headers_mut().append(CONTENT_LENGTH, length);
        }
        Ok::<_, std::convert::Infallible>(response)
    });
    let log = logged(|| async {
        let running = Running::start(
            common::server(&authority),
            axum::Router::new().fallback_service(app),
        );
        let mut client = Client::connect(Version::HTTP_2, running.https, &authority)
            .await
            .unwrap();
        let response = client.get("/").await.unwrap();
        assert!(!response.headers().contains_key(CONTENT_LENGTH));
        assert_eq!(common::text(response).await.unwrap(), "hello");
    })
    .await;
    let warning = "the application declared an invalid Content-Length declared=[\"5\", \"6\"]";
    assert!(log.contains(warning), "{log}");
}

#[tokio::test]
async fn an_informational_final_status_is_logged() {
    let authority = Authority::new();
    let app = axum::Router::new().route("/", get(|| async { StatusCode::EARLY_HINTS }));
    let log = logged(|| async {
        let running = Running::start(common::server(&authority), app);
        let mut client = Client::connect(Version::HTTP_2, running.https, &authority)
            .await
            .unwrap();
        let response = client.get("/").await.unwrap();
        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
    })
    .await;
    let error = "the application responded with an informational status status=103 Early Hints";
    assert!(log.contains(error), "{log}");
}
