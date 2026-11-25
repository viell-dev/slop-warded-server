//! Serves the built app and its API. Experiment: this belongs in the library.

use axum::{
    Router,
    http::{HeaderValue, StatusCode, header},
    response::Response,
    routing::any,
};
use std::path::Path;
use tower_http::{
    services::{ServeDir, ServeFile},
    set_header::SetResponseHeaderLayer,
};
use warded_server::warded::{
    self, DefinitionError, Matcher, match_fn,
    matchers::{get, head},
    negotiation::Offers,
};

fn under(prefix: &'static str) -> impl Matcher + Clone {
    match_fn(move |head| {
        let rest = head.uri().path().strip_prefix(prefix);
        rest.is_some_and(|rest| rest.is_empty() || rest.starts_with('/'))
    })
}

pub fn app(api: Router, dist: &Path) -> Result<Router, DefinitionError> {
    let get_or_head = || get().or(head());
    let forever = HeaderValue::from_static("public, max-age=31536000, immutable");
    let assets = Router::new()
        .nest_service("/assets", ServeDir::new(dist.join("assets")))
        .layer(SetResponseHeaderLayer::if_not_present(
            header::CACHE_CONTROL,
            move |response: &Response| response.status().is_success().then(|| forever.clone()),
        ));
    let index = Router::new()
        .fallback_service(ServeFile::new(dist.join("index.html")))
        .layer(SetResponseHeaderLayer::overriding(
            header::CACHE_CONTROL,
            HeaderValue::from_static("no-cache"),
        ))
        .layer(SetResponseHeaderLayer::appending(
            header::VARY,
            HeaderValue::from_static("accept"),
        ));
    // Browsers prefer HTML when they open a page; scripts, styles and images do not.
    let preferred = Offers::media_types(["application/octet-stream", "text/html"])?;
    let routes = warded::Router::new()
        .route(under("/api"), api)
        .route(get_or_head().and(under("/assets")), assets)
        .route(get_or_head().and(preferred.selected("text/html")?), index)
        // Files at the top of the build, such as `favicon.svg`.
        .route(
            get_or_head(),
            Router::new().fallback_service(ServeDir::new(dist)),
        )
        .fallback(any(|| async { StatusCode::NOT_FOUND }));
    Ok(warded_server::app(routes))
}
