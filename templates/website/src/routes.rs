//! The routes, in the order in which they are tried, and the middleware around them.

use crate::pages;
use axum::{Router, routing::any};
use tower::ServiceBuilder;
use tower_http::{
    catch_panic::CatchPanicLayer, compression::CompressionLayer, services::ServeDir,
    trace::TraceLayer,
};
use warded_server::warded::{
    self, DefinitionError, Matcher,
    matchers::{exact_path, get, head, path},
    negotiation::{LanguageMatching, Offers},
};

/// The directory that `/static/` serves, relative to where the server is started.
const STATIC_FILES: &str = "static";

/// The routes, with the middleware that every request passes through.
pub fn app() -> Result<Router, DefinitionError> {
    let get_or_head = || get().or(head());
    let languages = Offers::languages(["en", "sv"], LanguageMatching::Lookup)?;

    // Routes are tried in order, and the first whose conditions all hold gets the request.
    let routes = warded::Router::new()
        .route(
            get_or_head().and(path("/static/{*file}")?),
            // An Axum router as a destination, here to take the prefix off the path.
            Router::new().nest_service(
                "/static",
                ServeDir::new(STATIC_FILES).fallback(any(pages::not_found)),
            ),
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

#[cfg(test)]
mod tests;
