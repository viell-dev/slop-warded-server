//! The routes, in the order in which they are tried, and the middleware around them.

use crate::{contact, pages};
use axum::{Router, handler::HandlerWithoutStateExt};
use warded_server::{
    DefaultMiddleware, StaticFiles,
    warded::{
        self, DefinitionError, Matcher,
        matchers::{exact_path, get, head, path, post},
    },
};

/// The directory that `/static/` serves, relative to where the server is started.
const STATIC_FILES: &str = "static";

/// The routes, with the middleware that every request passes through.
pub fn app() -> Result<Router, DefinitionError> {
    // Browsers and monitors also ask for pages with HEAD, which `get()` alone does not include.
    let page = |route| get().or(head()).and(exact_path(route));

    // Routes are tried in order, and the first whose conditions all hold gets the request.
    // Handlers become services with `into_service`, so that the middleware sees the whole response
    // to HEAD.
    let routes = warded::Router::new()
        .route(page("/"), pages::home.into_service())
        .route(page("/about"), pages::about.into_service())
        .route(page("/contact"), contact::form.into_service())
        .route(
            post().and(exact_path("/contact")),
            contact::send.into_service(),
        )
        .route(page("/contact/sent"), contact::sent.into_service())
        .route(
            get().or(head()).and(path("/static/{*file}")?),
            StaticFiles::new("/static", STATIC_FILES).not_found(pages::not_found.into_service()),
        )
        .fallback(pages::not_found.into_service());

    // Logging, compression, and an error page when a handler panics.
    let middleware = DefaultMiddleware::new().on_panic(pages::error);
    Ok(warded_server::app(routes).layer(middleware))
}

#[cfg(test)]
mod tests;
