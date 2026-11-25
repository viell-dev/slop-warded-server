mod pages;

use axum::{Router, routing::any};
use tower::ServiceBuilder;
use tower_http::{
    catch_panic::CatchPanicLayer, compression::CompressionLayer, services::ServeDir,
    trace::TraceLayer,
};
use tracing_subscriber::EnvFilter;
use warded_server::{
    Server,
    warded::{
        self, DefinitionError, Matcher,
        matchers::{exact_path, get, head, path},
        negotiation::{LanguageMatching, Offers},
    },
};

/// The directory that `/static/` serves, relative to where the server is started.
const STATIC_FILES: &str = "static";

/// The routes, with the middleware that every request passes through.
fn app() -> Result<Router, DefinitionError> {
    let get_or_head = || get().or(head());
    let languages = Offers::languages(["en", "sv"], LanguageMatching::Lookup)?;

    // Routes are tried in order, and the first whose conditions all hold gets the request.
    let routes = warded::Router::new()
        .route(
            get_or_head().and(path("/static/{*file}")?),
            // An Axum router as a destination, here to take the prefix off the path.
            Router::new().nest_service("/static", ServeDir::new(STATIC_FILES)),
        )
        // One page in two languages. Swedish is served when the visitor prefers it, and English
        // otherwise.
        .route(
            get_or_head()
                .and(exact_path("/"))
                .and(languages.selected("sv")?),
            any(pages::home_in_swedish),
        )
        .route(get_or_head().and(exact_path("/")), any(pages::home))
        .fallback(any(pages::not_found));

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

    Server::from_env()?.serve(app()?).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
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
        }
    }

    #[tokio::test]
    async fn other_paths_get_the_not_found_page() {
        let response = get("/none", "en").await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        assert_eq!(response.headers()[CONTENT_TYPE], "text/html; charset=utf-8");
    }
}
