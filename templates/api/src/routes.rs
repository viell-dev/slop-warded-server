//! The routes, in the order in which they are tried, and the middleware around them.

use crate::{
    notes::{self, Notes},
    problem::{self, Problem},
};
use axum::{Router, http::StatusCode, routing::any};
use tower::ServiceBuilder;
use tower_http::{catch_panic::CatchPanicLayer, compression::CompressionLayer, trace::TraceLayer};
use warded_server::warded::{
    self, DefinitionError, Matcher,
    matchers::{delete, exact_path, get, path, post},
};

/// The routes, with the middleware that every request passes through.
pub fn app(notes: Notes) -> Result<Router, DefinitionError> {
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
            Problem::new(StatusCode::NOT_FOUND, "nothing is here")
        }));

    let middleware = ServiceBuilder::new()
        .layer(TraceLayer::new_for_http())
        .layer(CompressionLayer::new())
        .layer(CatchPanicLayer::custom(problem::panicked));
    Ok(warded_server::app(routes).layer(middleware))
}

#[cfg(test)]
mod tests;
