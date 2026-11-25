---
description: "Server-sent events, uploads, downloads and WebSockets: what streams on each protocol, and how to keep it bounded."
---

# Streaming and WebSockets

Bodies stream in both directions on every protocol. A handler can answer before it has read the
request, and read the request while it answers. Nothing is collected in memory unless an extractor
such as `String`, `Bytes` or `Json` does it.

## Server-sent events

```sh
cargo add futures-util tokio-util
```

A response that stays open and sends events to the browser as they happen, until the server stops:

```rust
use axum::{
    body::Body,
    extract::State,
    http::{Request, header::CONTENT_TYPE},
    response::sse::{Event, KeepAlive, Sse},
    routing::any,
};
use futures_util::{
    StreamExt,
    stream::{self, Stream},
};
use http_body_util::BodyExt;
use std::convert::Infallible;
use tokio_util::sync::CancellationToken;
use tower::ServiceExt;
use warded_server::warded::{
    Matcher, Router,
    matchers::{exact_path, get},
};

async fn events(
    State(stopping): State<CancellationToken>,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let numbers = stream::iter(1..=3).map(|n| Ok(Event::default().data(n.to_string())));
    let events = numbers.take_until(stopping.cancelled_owned());
    // Comments in between tell the client, and everything on the way, that it is still open.
    Sse::new(events).keep_alive(KeepAlive::default())
}

fn app(stopping: CancellationToken) -> axum::Router {
    let routes = Router::new().route(
        get().and(exact_path("/events")),
        any(events).with_state(stopping),
    );
    warded_server::app(routes)
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let stopping = CancellationToken::new();
    // `serve` listens for the same signals, and any number of tasks can.
    tokio::spawn({
        let stopping = stopping.clone();
        async move {
            warded_server::shutdown().await;
            stopping.cancel();
        }
    });
    let app = app(stopping);
    // Then: Server::from_env()?.serve(app).await

    let request = Request::builder().uri("/events").body(Body::empty())?;
    let response = app.oneshot(request).await?;
    assert_eq!(response.headers()[CONTENT_TYPE], "text/event-stream");
    let body = response.into_body().collect().await?.to_bytes();
    assert_eq!(body, "data: 1\n\ndata: 2\n\ndata: 3\n\n");
    Ok(())
}
```

Without the token, an open event stream keeps a shutdown waiting for the whole grace period.

`CompressionLayer` leaves event streams alone. A timeout for whole responses would end them; see
[middleware](./middleware.md#timeouts).

When the client leaves, the stream is dropped: over HTTP/1.1 and HTTP/2 at once, and over HTTP/3
when something is sent next, or the connection ends.

## Uploads

Read the body piece by piece, and decide how much you accept:

```sh
cargo add http-body-util tempfile
cargo add tokio --features fs,io-util
```

```rust
use axum::{
    body::Body,
    extract::Request,
    http::StatusCode,
    routing::any,
};
use http_body_util::{BodyExt, Limited};
use tokio::io::AsyncWriteExt;
use tower::ServiceExt;
use warded_server::warded::{
    Matcher, Router,
    matchers::{exact_path, post},
};

const LARGEST: usize = 1_000;

async fn upload(request: Request) -> Result<String, StatusCode> {
    let file = tempfile::tempfile().map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let mut file = tokio::fs::File::from_std(file);
    let mut body = Limited::new(request.into_body(), LARGEST);
    let mut received = 0;
    while let Some(frame) = body.frame().await {
        // Too large, or the client left before the end.
        let frame = frame.map_err(|_| StatusCode::PAYLOAD_TOO_LARGE)?;
        if let Some(piece) = frame.data_ref() {
            received += piece.len();
            file.write_all(piece)
                .await
                .map_err(|_| StatusCode::INSUFFICIENT_STORAGE)?;
        }
    }
    Ok(format!("{received} bytes"))
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let routes = Router::new().route(post().and(exact_path("/upload")), any(upload));
    let app = warded_server::app(routes);
    for (size, expected) in [(1_000, StatusCode::OK), (1_001, StatusCode::PAYLOAD_TOO_LARGE)] {
        let request = Request::builder()
            .method("POST")
            .uri("/upload")
            .body(Body::from(vec![0; size]))?;
        assert_eq!(app.clone().oneshot(request).await?.status(), expected);
    }
    Ok(())
}
```

A body that ends early is an error from `frame`, never a normal end, so an incomplete upload is not
mistaken for a complete one. On HTTP/3 that also holds for a body that does not match its
`Content-Length`. Browsers send files in forms as multipart bodies; Axum's `Multipart` extractor
reads those as streams too.

## Downloads

`ServeDir` and `ServeFile` from `tower-http` stream files, and support ranges so that clients can
resume. For content from elsewhere, return `Body::from_stream(…)`. If it fails in the middle, return
an error from the stream: the client then sees a failed response on every protocol, rather than a
short one.

## WebSockets

They work over HTTP/1.1, which is what browsers use for them on this server. Use Axum's
`WebSocketUpgrade` in a route:

```sh
cargo add axum --features ws
```

```rust
use axum::{
    extract::ws::{WebSocket, WebSocketUpgrade},
    response::Response,
    routing::any,
};
use warded_server::warded::{
    Matcher, Router,
    matchers::{exact_path, get},
};

async fn echo(upgrade: WebSocketUpgrade) -> Response {
    upgrade.on_upgrade(|mut socket: WebSocket| async move {
        while let Some(Ok(message)) = socket.recv().await {
            if socket.send(message).await.is_err() {
                return;
            }
        }
    })
}

fn main() {
    let routes = Router::new().route(get().and(exact_path("/ws")), any(echo));
    let _app = warded_server::app(routes);
}
```

An open WebSocket counts towards the [connection limit](./configuration.md#limits). It does not
delay a shutdown: once all other requests have finished, it fails when its handler next uses it, and
the client sees no close frame. To say goodbye properly, use
[`serve_until`](./existing-axum.md#decide-when-to-stop) and tell the handlers, for example through a
`tokio::sync::watch` channel, when you decide to stop.
