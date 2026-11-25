//! The development address: the application over cleartext HTTP on loopback, with live reload.

use axum::{
    body::{Body, Bytes, HttpBody},
    extract::{Request, State},
    http::{HeaderValue, header::ACCEPT_ENCODING},
    middleware::{Next, from_fn_with_state},
    response::Response,
};
use http_body::Frame;
use std::{
    future::Future,
    pin::Pin,
    task::{Context, Poll},
};
use tokio_util::sync::{CancellationToken, WaitForCancellationFutureOwned};
use tower::util::MapRequestLayer;
use tower_livereload::LiveReloadLayer;

/// Where the reload script waits for the server: tower-livereload's default.
pub(crate) const EVENTS: &str = "/_tower-livereload/event-stream";

/// The application as the development address serves it.
///
/// With live reload, each HTML page that a browser opens gets a script that waits on an event
/// stream for the server to go away and come back, and then reloads the page. The stream ends when
/// the server begins to shut down, so that it does not hold up the shutdown.
pub(crate) fn app(
    app: axum::Router,
    live_reload: bool,
    stopping: CancellationToken,
) -> axum::Router {
    if !live_reload {
        return app;
    }
    app.layer(LiveReloadLayer::new().request_predicate(is_page as fn(&Request) -> bool))
        .layer(from_fn_with_state(stopping, end_at_shutdown))
        .layer(MapRequestLayer::new(uncompressed))
}

/// Whether the browser opens the response as a page, and does not fetch part of one, such as an
/// HTML fragment for a script. Clients that do not say get the script.
fn is_page(request: &Request) -> bool {
    request
        .headers()
        .get("sec-fetch-dest")
        .is_none_or(|destination| destination == "document")
}

/// Ask the application for uncompressed responses, because the script cannot be added to
/// compressed ones. Compression gains nothing on loopback.
fn uncompressed(mut request: Request) -> Request {
    let identity = HeaderValue::from_static("identity");
    request.headers_mut().insert(ACCEPT_ENCODING, identity);
    request
}

async fn end_at_shutdown(
    State(stopping): State<CancellationToken>,
    request: Request,
    next: Next,
) -> Response {
    let events = request.uri().path() == EVENTS;
    let response = next.run(request).await;
    if !events {
        return response;
    }
    response.map(|body| {
        Body::new(UntilStopped {
            body,
            stopped: Box::pin(stopping.cancelled_owned()),
        })
    })
}

/// A body that ends when the server begins to shut down.
///
/// The reload script then tries again until the server is back, and reloads the page.
struct UntilStopped {
    body: Body,
    stopped: Pin<Box<WaitForCancellationFutureOwned>>,
}

impl HttpBody for UntilStopped {
    type Data = Bytes;
    type Error = axum::Error;

    fn poll_frame(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Bytes>, axum::Error>>> {
        if self.stopped.as_mut().poll(cx).is_ready() {
            return Poll::Ready(None);
        }
        Pin::new(&mut self.body).poll_frame(cx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_pages_get_the_script() {
        for (destination, expected) in [
            (None, true),
            (Some("document"), true),
            (Some("empty"), false),
            (Some("iframe"), false),
            (Some("script"), false),
        ] {
            let mut request = Request::new(Body::empty());
            if let Some(destination) = destination {
                let value = HeaderValue::from_static(destination);
                request.headers_mut().insert("sec-fetch-dest", value);
            }
            assert_eq!(is_page(&request), expected, "{destination:?}");
        }
    }
}
