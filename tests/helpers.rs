//! The middleware and file helpers, over real connections of each protocol.

mod common;

use axum::{
    Json,
    body::Body,
    handler::HandlerWithoutStateExt,
    http::{HeaderValue, Request, StatusCode, header},
    response::{Html, Response},
    routing::get as axum_get,
};
use common::{Authority, Client, Running, VERSIONS, get, request, soon, text};
use http_body_util::BodyExt;
use std::path::Path;
use warded::{
    Matcher, Router,
    matchers::{exact_path, get as get_matcher, head, path},
};
use warded_server::{DefaultMiddleware, SinglePageApp, StaticFiles, app};

const PAGE: &str = "<!doctype html><title>A page</title><p>Hello, hello, hello, hello</p>";

async fn page() -> Html<&'static str> {
    Html(PAGE)
}

async fn broken() -> Html<&'static str> {
    panic!("a mistake in a handler")
}

const MISSING: &str = "<p>There is no such page.</p>";

/// The site's page for a 404, with a status that `StaticFiles` replaces.
async fn missing_with_200() -> Html<&'static str> {
    Html(MISSING)
}

async fn not_found() -> (StatusCode, Html<&'static str>) {
    (StatusCode::NOT_FOUND, Html(MISSING))
}

fn with(middleware: DefaultMiddleware) -> axum::Router {
    let get_or_head = || get_matcher().or(head());
    let routes = Router::new()
        .route(get_or_head().and(exact_path("/")), page.into_service())
        .route(exact_path("/broken"), broken.into_service());
    app(routes).layer(middleware)
}

/// A request that accepts this encoding.
fn accepting(method: &str, path: &str, encoding: &'static str) -> Request<Body> {
    let mut request = request(method, path, Body::empty());
    let value = HeaderValue::from_static(encoding);
    request.headers_mut().insert(header::ACCEPT_ENCODING, value);
    request
}

#[tokio::test]
async fn the_default_middleware_answers_a_panic_with_a_500() {
    let authority = Authority::new();
    let problem = |_| {
        let body = Json(serde_json::json!({ "error": "something went wrong" }));
        (StatusCode::INTERNAL_SERVER_ERROR, body)
    };
    let plain = Running::start(common::server(&authority), with(DefaultMiddleware::new()));
    let custom = DefaultMiddleware::new().on_panic(problem);
    let custom = Running::start(common::server(&authority), with(custom));
    for version in VERSIONS {
        let mut client = Client::connect(version, plain.https, &authority)
            .await
            .unwrap();
        let response = client.get("/broken").await.unwrap();
        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(text(response).await.unwrap(), "", "{version:?}");
        // The connection carries on.
        let response = client.get("/").await.unwrap();
        assert_eq!(text(response).await.unwrap(), PAGE, "{version:?}");

        let mut client = Client::connect(version, custom.https, &authority)
            .await
            .unwrap();
        let response = client.get("/broken").await.unwrap();
        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
        let json = text(response).await.unwrap();
        assert_eq!(json, r#"{"error":"something went wrong"}"#, "{version:?}");
    }
}

#[tokio::test]
async fn the_default_middleware_compresses_and_head_has_the_head_of_get() {
    let authority = Authority::new();
    let running = Running::start(common::server(&authority), with(DefaultMiddleware::new()));
    for version in VERSIONS {
        let mut client = Client::connect(version, running.https, &authority)
            .await
            .unwrap();
        for encoding in ["br", "gzip"] {
            let get = client.send(accepting("GET", "/", encoding)).await.unwrap();
            assert_eq!(get.status(), StatusCode::OK);
            let mut get_fields = get.headers().clone();
            soon(get.into_body().collect()).await.unwrap();
            let head = client.send(accepting("HEAD", "/", encoding)).await.unwrap();
            let mut head_fields = head.headers().clone();
            assert_eq!(text(head).await.unwrap(), "", "{version:?}");
            // Apart from when they were made, and how HTTP/1.1 frames the content of GET.
            for fields in [&mut get_fields, &mut head_fields] {
                fields.remove(header::DATE);
                fields.remove(header::TRANSFER_ENCODING);
            }
            assert_eq!(get_fields[header::CONTENT_ENCODING], encoding);
            assert_eq!(head_fields, get_fields, "{version:?} {encoding}");
        }
        let identity = client.get("/").await.unwrap();
        assert!(!identity.headers().contains_key(header::CONTENT_ENCODING));
        assert_eq!(text(identity).await.unwrap(), PAGE);
    }
}

/// A directory of files, with something beside it that must stay out of reach.
fn site_files() -> tempfile::TempDir {
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir(root.path().join("static")).unwrap();
    std::fs::write(
        root.path().join("static/style.css"),
        "main { margin: auto }",
    )
    .unwrap();
    std::fs::write(root.path().join("secret.txt"), "secret").unwrap();
    root
}

/// Paths that try to leave a directory below `prefix`, towards `secret.txt` beside it.
fn escapes(prefix: &str) -> Vec<String> {
    [
        "../secret.txt",
        "%2e%2e/secret.txt",
        "%2E%2E%2Fsecret.txt",
        "..%5csecret.txt",
        "%2e%2e%5csecret.txt",
        "....//secret.txt",
        "%2fsecret.txt",
        "C:%5csecret.txt",
    ]
    .iter()
    .map(|rest| format!("{prefix}/{rest}"))
    .collect()
}

#[tokio::test]
async fn static_files_are_served_and_stay_in_their_directory() {
    let authority = Authority::new();
    let root = site_files();
    let files = StaticFiles::new("/static", root.path().join("static"));
    let get_or_head = || get_matcher().or(head());
    let routes = Router::new()
        .route(
            get_or_head().and(path("/static/{*file}").unwrap()),
            files.clone().not_found(missing_with_200.into_service()),
        )
        .route(get_or_head().and(path("/plain/{*file}").unwrap()), files)
        .fallback(not_found.into_service());
    let running = Running::start(common::server(&authority), app(routes));
    for version in VERSIONS {
        let mut client = Client::connect(version, running.https, &authority)
            .await
            .unwrap();
        let response = client.get("/static/style.css").await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()[header::CONTENT_TYPE], "text/css");
        assert_eq!(text(response).await.unwrap(), "main { margin: auto }");

        let mut missing = escapes("/static");
        missing
            .extend(["/static/none.css", "/static/", "/static/static/style.css"].map(String::from));
        for path in missing {
            let response = client.get(&path).await.unwrap();
            assert_eq!(
                response.status(),
                StatusCode::NOT_FOUND,
                "{version:?} {path}"
            );
            assert_eq!(text(response).await.unwrap(), MISSING, "{version:?} {path}");
        }

        // Without a page of its own, below another prefix than the route's.
        for path in escapes("/plain") {
            let response = client.get(&path).await.unwrap();
            assert_eq!(
                response.status(),
                StatusCode::NOT_FOUND,
                "{version:?} {path}"
            );
            assert_eq!(text(response).await.unwrap(), "", "{version:?} {path}");
        }
    }
}

/// A build as Vite writes it, and something beside it that must stay out of reach.
fn build() -> tempfile::TempDir {
    let root = tempfile::tempdir().unwrap();
    let dist = root.path().join("dist");
    std::fs::create_dir_all(dist.join("assets/nested")).unwrap();
    std::fs::create_dir(dist.join("settings")).unwrap();
    let write = |path: &str, content: &str| std::fs::write(dist.join(path), content).unwrap();
    write("index.html", "<!doctype html><div id=app></div>");
    write("favicon.svg", "<svg></svg>");
    write("assets/app.1a2b.js", "start()");
    write("assets/index.html", "not a page of the app");
    write("assets/nested/index.html", "not a page either");
    std::fs::write(root.path().join("secret.txt"), "secret").unwrap();
    root
}

fn single_page_app(dist: &Path) -> axum::Router {
    let api = axum::Router::new().route("/api/hello", axum_get(|| async { Json("hello") }));
    SinglePageApp::new(dist).api("/api", api).into_router()
}

const BROWSER: &str = "text/html,application/xhtml+xml,*/*;q=0.8";
const INDEX: &str = "<!doctype html><div id=app></div>";
const FOREVER: &str = "public, max-age=31536000, immutable";

#[tokio::test]
async fn a_single_page_app_answers_each_kind_of_request() {
    let authority = Authority::new();
    let root = build();
    let app = single_page_app(&root.path().join("dist"));
    let running = Running::start(common::server(&authority), app);
    // Path, Accept, status, Cache-Control, whether it varies by Accept, and the content.
    let mut cases = vec![
        ("/", BROWSER, 200, Some("no-cache"), true, INDEX),
        (
            "/settings/profile",
            BROWSER,
            200,
            Some("no-cache"),
            true,
            INDEX,
        ),
        ("/settings", BROWSER, 200, Some("no-cache"), true, INDEX),
        ("/index.html", "*/*", 200, Some("no-cache"), true, INDEX),
        (
            "/favicon.svg",
            "image/*",
            200,
            Some("no-cache"),
            true,
            "<svg></svg>",
        ),
        // A browser that opens it gets the file, and not the app.
        (
            "/favicon.svg",
            BROWSER,
            200,
            Some("no-cache"),
            true,
            "<svg></svg>",
        ),
        (
            "/assets/app.1a2b.js",
            "*/*",
            200,
            Some(FOREVER),
            false,
            "start()",
        ),
        // HTML keeps its name, so it is not kept.
        (
            "/assets/index.html",
            BROWSER,
            200,
            None,
            false,
            "not a page of the app",
        ),
        ("/assets/nested/", BROWSER, 404, None, false, ""),
        ("/assets/old.js", "*/*", 404, None, false, ""),
        ("/assets/", BROWSER, 404, None, false, ""),
        ("/assets", BROWSER, 404, None, false, ""),
        (
            "/api/hello",
            "application/json",
            200,
            None,
            false,
            "\"hello\"",
        ),
        // A mistake in the app's requests must not look like a page.
        ("/api/unknown", "application/json", 404, None, false, ""),
        ("/api", BROWSER, 404, None, false, ""),
        ("/apis", BROWSER, 200, Some("no-cache"), true, INDEX),
        (
            "/settings",
            "application/json",
            404,
            Some("no-cache"),
            true,
            "",
        ),
        ("/settings", "*/*", 404, Some("no-cache"), true, ""),
    ];
    let (top, assets) = (escapes(""), escapes("/assets"));
    for path in &top {
        cases.push((path, "*/*", 404, Some("no-cache"), true, ""));
    }
    for path in &assets {
        cases.push((path, "*/*", 404, None, false, ""));
    }
    for version in VERSIONS {
        let mut client = Client::connect(version, running.https, &authority)
            .await
            .unwrap();
        for &(path, accept, status, cache, varies, content) in &cases {
            let mut request = get(path);
            let accept = HeaderValue::from_str(accept).unwrap();
            request.headers_mut().insert(header::ACCEPT, accept);
            let response = client.send(request).await.unwrap();
            let case = format!("{version:?} {path} {:?}", response.headers());
            assert_eq!(response.status(), status, "{case}");
            let found = response.headers().get(header::CACHE_CONTROL);
            assert_eq!(found.map(|value| value.to_str().unwrap()), cache, "{case}");
            let vary: Vec<_> = response.headers().get_all(header::VARY).iter().collect();
            assert_eq!(vary == ["accept"], varies, "{case}");
            assert_eq!(text(response).await.unwrap(), content, "{case}");
        }

        // Only GET and HEAD reach the files.
        let response = client.send(request("POST", "/", Body::empty())).await;
        assert_eq!(response.unwrap().status(), StatusCode::NOT_FOUND);
        let mut head = request("HEAD", "/", Body::empty());
        head.headers_mut()
            .insert(header::ACCEPT, HeaderValue::from_static(BROWSER));
        let response: Response<Body> = client.send(head).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers()[header::CONTENT_LENGTH],
            INDEX.len().to_string()
        );
        assert_eq!(text(response).await.unwrap(), "");
    }
}

#[test]
#[should_panic(expected = "a path prefix starts with `/`")]
fn a_prefix_ends_without_a_slash() {
    let _ = StaticFiles::new("/static/", "static");
}

#[test]
#[should_panic(expected = "the assets are a directory inside the build")]
fn the_assets_are_inside_the_build() {
    let _ = SinglePageApp::new("dist").assets("../other");
}
