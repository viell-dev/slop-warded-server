//! The routes, in the order in which they are tried, and the middleware around them.

use crate::pages;
use axum::{Router, handler::HandlerWithoutStateExt};
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

    // Routes are tried in order, and the first whose conditions all hold gets the request. Handlers
    // become services with `into_service`, so that the middleware sees the whole response to HEAD.
    let routes = warded::Router::new()
        .route(
            get_or_head().and(path("/static/{*file}")?),
            // An Axum router as a destination, here to take the prefix off the path.
            Router::new().nest_service(
                "/static",
                ServeDir::new(STATIC_FILES).fallback(pages::not_found.into_service()),
            ),
        )
        // One page in two languages. Swedish is served when the visitor prefers it, and English
        // otherwise.
        .route(
            get_or_head()
                .and(exact_path("/"))
                .and(languages.selected("sv")?),
            pages::home_in_swedish.into_service(),
        )
        .route(
            get_or_head().and(exact_path("/")),
            pages::home.into_service(),
        )
        .fallback(pages::not_found.into_service());

    let middleware = ServiceBuilder::new()
        .layer(TraceLayer::new_for_http())
        .layer(CompressionLayer::new())
        .layer(CatchPanicLayer::new());
    Ok(warded_server::app(routes).layer(middleware))
}

#[cfg(test)]
mod tests;
