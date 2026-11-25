use super::*;
use axum::{
    body::Body,
    http::{
        Request, StatusCode,
        header::{ACCEPT_LANGUAGE, CONTENT_LANGUAGE, CONTENT_TYPE, VARY},
    },
    response::Response,
};
use http_body_util::BodyExt;
use tower::ServiceExt;

async fn get(uri: &str, language: &str) -> Response {
    let request = Request::builder()
        .uri(uri)
        .header(ACCEPT_LANGUAGE, language)
        .body(Body::empty())
        .unwrap();
    app().unwrap().oneshot(request).await.unwrap()
}

#[tokio::test]
async fn the_home_page_follows_the_visitor() {
    for (language, expected) in [("sv-SE, en;q=0.5", "sv"), ("en", "en"), ("de", "en")] {
        let response = get("/", language).await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()[CONTENT_LANGUAGE], expected, "{language}");
        let mut vary = response.headers().get_all(VARY).iter();
        assert!(vary.any(|field| field == "accept-language"));
        let body = response.into_body().collect().await.unwrap().to_bytes();
        let html = String::from_utf8(body.to_vec()).unwrap();
        assert!(
            html.contains(&format!("<html lang=\"{expected}\">")),
            "{html}"
        );
    }
}

#[tokio::test]
async fn static_files_are_served() {
    let response = get("/static/style.css", "en").await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()[CONTENT_TYPE], "text/css");
    for missing in [
        "/static/none.css",
        "/static/../Cargo.toml",
        "/static/%2e%2e/Cargo.toml",
    ] {
        let response = get(missing, "en").await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND, "{missing}");
        assert_eq!(response.headers()[CONTENT_TYPE], "text/html; charset=utf-8");
    }
}

#[tokio::test]
async fn other_paths_get_the_not_found_page() {
    let response = get("/none", "en").await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    assert_eq!(response.headers()[CONTENT_TYPE], "text/html; charset=utf-8");
}
