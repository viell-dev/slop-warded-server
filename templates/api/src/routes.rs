//! The routes, in the order in which they are tried, and the middleware around them.

use crate::{
    notes::{self, Notes},
    problem::{self, Problem},
};
use axum::{Json, Router, http::StatusCode, routing::any};
use serde::Serialize;
use warded_server::{
    DefaultMiddleware,
    warded::{
        self, DefinitionError, Matcher,
        matchers::{delete, exact_path, get, path, post},
    },
};

/// The routes, with the middleware that every request passes through.
pub fn app(notes: Notes) -> Result<Router, DefinitionError> {
    // Routes are tried in order, and the first whose conditions all hold gets the request.
    // Warded selects on the method too, so `any` is enough for the handlers.
    let routes = warded::Router::new()
        .route(get().and(exact_path("/")), any(index))
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
            any(notes::remove).with_state(notes.clone()),
        )
        .fallback(any(|| async {
            Problem::new(StatusCode::NOT_FOUND, "nothing is here")
        }));

    // Logging, compression, and a JSON error when a handler panics.
    let middleware = DefaultMiddleware::new().on_panic(problem::panicked);
    Ok(warded_server::app(routes).layer(middleware))
}

/// What the API offers, for someone who opens it in a browser.
#[derive(Serialize)]
struct Index {
    endpoints: &'static [&'static str],
}

async fn index() -> Json<Index> {
    Json(Index {
        endpoints: &[
            "GET /notes",
            "POST /notes",
            "GET /notes/{id}",
            "DELETE /notes/{id}",
        ],
    })
}

#[cfg(test)]
mod tests;
