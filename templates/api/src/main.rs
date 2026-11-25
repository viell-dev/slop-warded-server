mod notes;

use axum::{
    Json, Router,
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::any,
};
use notes::Notes;
use serde::Serialize;
use tower::ServiceBuilder;
use tower_http::{catch_panic::CatchPanicLayer, compression::CompressionLayer, trace::TraceLayer};
use tracing_subscriber::EnvFilter;
use warded_server::{
    Server,
    warded::{
        self, DefinitionError, Matcher,
        matchers::{delete, exact_path, get, path, post},
    },
};

/// A response that reports a problem as JSON.
pub struct Problem(StatusCode, &'static str);

impl IntoResponse for Problem {
    fn into_response(self) -> Response {
        #[derive(Serialize)]
        struct Body {
            error: &'static str,
        }

        (self.0, Json(Body { error: self.1 })).into_response()
    }
}

/// The routes, with the middleware that every request passes through.
fn app(notes: Notes) -> Result<Router, DefinitionError> {
    // Routes are tried in order, and the first whose conditions all hold gets the request.
    // Warded selects on the method too, so `any` is enough for the handlers.
    let routes = warded::Router::new()
        .route(
            get().and(exact_path("/notes")),
            any(notes::list).with_state(notes.clone()),
        )
        .route(
            post().and(exact_path("/notes")),
            any(notes::create).with_state(notes.clone()),
        )
        .route(
            get().and(path("/notes/{id}")?),
            any(notes::show).with_state(notes.clone()),
        )
        .route(
            delete().and(path("/notes/{id}")?),
            any(notes::remove).with_state(notes),
        )
        .fallback(any(|| async {
            Problem(StatusCode::NOT_FOUND, "nothing is here")
        }));

    let middleware = ServiceBuilder::new()
        .layer(TraceLayer::new_for_http())
        .layer(CompressionLayer::new())
        .layer(CatchPanicLayer::new());
    Ok(warded_server::app(routes).layer(middleware))
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| "info,tower_http=debug".into());
    tracing_subscriber::fmt().with_env_filter(filter).init();

    Server::from_env()?.serve(app(Notes::default())?).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        body::Body,
        http::{Request, header::CONTENT_TYPE},
    };
    use http_body_util::BodyExt;
    use serde_json::{Value, json};
    use tower::ServiceExt;

    async fn send(
        app: &Router,
        method: &str,
        uri: &str,
        body: Option<Value>,
    ) -> (StatusCode, Value) {
        let request = Request::builder().method(method).uri(uri);
        let request = match body {
            Some(body) => request
                .header(CONTENT_TYPE, "application/json")
                .body(body.to_string().into()),
            None => request.body(Body::empty()),
        };
        let response = app.clone().oneshot(request.unwrap()).await.unwrap();
        let status = response.status();
        let body = response.into_body().collect().await.unwrap().to_bytes();
        (status, serde_json::from_slice(&body).unwrap_or(Value::Null))
    }

    #[tokio::test]
    async fn notes_can_be_created_read_and_deleted() {
        let app = app(Notes::default()).unwrap();
        let note = json!({ "id": 1, "text": "milk" });
        let created = send(&app, "POST", "/notes", Some(json!({ "text": "milk" }))).await;
        assert_eq!(created, (StatusCode::CREATED, note.clone()));
        let listed = send(&app, "GET", "/notes", None).await;
        assert_eq!(listed, (StatusCode::OK, json!([note.clone()])));
        let shown = send(&app, "GET", "/notes/1", None).await;
        assert_eq!(shown, (StatusCode::OK, note));
        let removed = send(&app, "DELETE", "/notes/1", None).await;
        assert_eq!(removed, (StatusCode::NO_CONTENT, Value::Null));
        let (status, _) = send(&app, "GET", "/notes/1", None).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn bad_requests_are_refused() {
        let app = app(Notes::default()).unwrap();
        for (method, uri, body, expected) in [
            ("GET", "/elsewhere", None, StatusCode::NOT_FOUND),
            ("PUT", "/notes", None, StatusCode::NOT_FOUND),
            ("GET", "/notes/one", None, StatusCode::BAD_REQUEST),
            (
                "POST",
                "/notes",
                Some(json!({ "text": 1 })),
                StatusCode::UNPROCESSABLE_ENTITY,
            ),
        ] {
            let (status, _) = send(&app, method, uri, body).await;
            assert_eq!(status, expected, "{method} {uri}");
        }
    }
}
