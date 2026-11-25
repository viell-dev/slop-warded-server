use super::*;
use axum::{
    body::Body,
    http::{Request, header::CONTENT_TYPE},
};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use tower::ServiceExt;

async fn send(app: &Router, method: &str, uri: &str, body: Option<Value>) -> (StatusCode, Value) {
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
        let (status, body) = send(&app, method, uri, body).await;
        assert_eq!(status, expected, "{method} {uri}");
        assert!(body["error"].is_string(), "{method} {uri}: {body}");
    }
}
