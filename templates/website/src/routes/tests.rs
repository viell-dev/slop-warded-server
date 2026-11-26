use super::*;
use axum::{
    body::Body,
    http::{
        Request, StatusCode,
        header::{ACCEPT_ENCODING, CONTENT_ENCODING, CONTENT_TYPE, LOCATION},
    },
    response::Response,
};
use http_body_util::BodyExt;
use tower::ServiceExt;

async fn send(method: &str, uri: &str, form: Option<&str>) -> Response {
    let request = Request::builder().method(method).uri(uri);
    let request = match form {
        Some(form) => request
            .header(CONTENT_TYPE, "application/x-www-form-urlencoded")
            .body(Body::from(form.to_owned())),
        None => request.body(Body::empty()),
    };
    app().unwrap().oneshot(request.unwrap()).await.unwrap()
}

async fn html(response: Response) -> String {
    let body = response.into_body().collect().await.unwrap().to_bytes();
    String::from_utf8(body.to_vec()).unwrap()
}

#[tokio::test]
async fn the_pages_mark_where_the_visitor_is() {
    for (uri, current) in [
        ("/", r#"<a href="/" aria-current="page">"#),
        ("/about", r#"<a href="/about" aria-current="page">"#),
        ("/contact", r#"<a href="/contact" aria-current="page">"#),
    ] {
        let response = send("GET", uri, None).await;
        assert_eq!(response.status(), StatusCode::OK, "{uri}");
        assert_eq!(response.headers()[CONTENT_TYPE], "text/html; charset=utf-8");
        let html = html(response).await;
        assert!(html.contains(current), "{uri}: {html}");
        assert_eq!(html.matches("aria-current").count(), 1, "{uri}");
    }
}

#[tokio::test]
async fn head_has_the_head_of_get() {
    for uri in ["/", "/about", "/none"] {
        let send = async |method: &str| {
            let request = Request::builder()
                .method(method)
                .uri(uri)
                .header(ACCEPT_ENCODING, "br")
                .body(Body::empty())
                .unwrap();
            app().unwrap().oneshot(request).await.unwrap()
        };
        let (get, head) = (send("GET").await, send("HEAD").await);
        assert_eq!(get.headers()[CONTENT_ENCODING], "br", "{uri}");
        assert_eq!(get.headers(), head.headers(), "{uri}");
    }
}

#[tokio::test]
async fn the_form_shows_its_problems_until_it_is_right() {
    let response = send("POST", "/contact", Some("name=&email=someone&text=Hello")).await;
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let html = html(response).await;
    assert!(html.contains("<title>Error: Contact</title>"), "{html}");
    assert!(html.contains("Enter your name."), "{html}");
    assert!(html.contains("Enter an e-mail address"), "{html}");
    // What the visitor entered stays, and the fields with problems say so.
    assert!(
        html.contains(r#"value="someone" aria-invalid="true""#),
        "{html}"
    );
    assert!(html.contains(">Hello</textarea>"), "{html}");
    assert_eq!(html.matches(r#"aria-invalid="true""#).count(), 2, "{html}");

    // Not a form at all.
    let response = send("POST", "/contact", None).await;
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);

    let form = "name=Ada&email=ada%40example.com&text=Hello";
    let response = send("POST", "/contact", Some(form)).await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    assert_eq!(response.headers()[LOCATION], "/contact/sent");
    let response = send("GET", "/contact/sent", None).await;
    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn values_are_escaped() {
    let form = "name=%3Cscript%3E&email=x&text=%22%3E%3Cb%3E";
    let html = html(send("POST", "/contact", Some(form)).await).await;
    assert!(
        !html.contains("<script>") && !html.contains("<b>"),
        "{html}"
    );
    assert!(html.contains("&#60;script&#62;"), "{html}");
}

#[tokio::test]
async fn static_files_are_served() {
    let response = send("GET", "/static/style.css", None).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()[CONTENT_TYPE], "text/css");
    for missing in [
        "/static/none.css",
        "/static/../Cargo.toml",
        "/static/%2e%2e/Cargo.toml",
    ] {
        let response = send("GET", missing, None).await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND, "{missing}");
        assert!(html(response).await.contains("Page not found"), "{missing}");
    }
}

#[tokio::test]
async fn other_paths_get_the_not_found_page() {
    let response = send("GET", "/none", None).await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    assert_eq!(response.headers()[CONTENT_TYPE], "text/html; charset=utf-8");
    assert!(html(response).await.contains("<h1>Page not found</h1>"));
}

#[tokio::test]
async fn a_panic_gets_the_error_page() {
    let response = pages::error(Box::new("a mistake"));
    assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
    assert!(
        html(response)
            .await
            .contains("<h1>Something went wrong</h1>")
    );
}
