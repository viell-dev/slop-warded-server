//! Files from a directory: a website's static files, and the build of a single-page app.

use axum::{
    extract::Request,
    response::{IntoResponse, Response},
};
use std::{
    convert::Infallible,
    path::PathBuf,
    task::{Context, Poll},
};
use tower::Service;
use tower_http::services::ServeDir;

/// The files in a directory, as a destination for a route below a path such as `/static`.
///
/// Files get a content type from their name, `Last-Modified`, and answers to conditional and range
/// requests, from `tower-http`'s `ServeDir`. Paths that would leave the directory, such as ones
/// with `..`, find no file. Responses to HEAD have the head of an uncompressed response, also where
/// GET gets a compressed one.
///
/// ```
/// use warded_server::{
///     StaticFiles,
///     axum::handler::HandlerWithoutStateExt,
///     warded::{Matcher, Router, matchers::{get, head, path}},
/// };
///
/// async fn not_found() -> (warded_server::axum::http::StatusCode, &'static str) {
///     (warded_server::axum::http::StatusCode::NOT_FOUND, "There is no such page.")
/// }
///
/// # fn main() -> Result<(), warded_server::warded::DefinitionError> {
/// let files = StaticFiles::new("/static", "static").not_found(not_found.into_service());
/// let routes = Router::new().route(get().or(head()).and(path("/static/{*file}")?), files);
/// # let _ = warded_server::app(routes);
/// # Ok(())
/// # }
/// ```
#[derive(Clone, Debug)]
pub struct StaticFiles {
    prefix: String,
    directory: PathBuf,
    router: axum::Router,
}

impl StaticFiles {
    /// Serve the files in `directory` below the path `prefix`.
    ///
    /// With `StaticFiles::new("/static", "static")`, `/static/style.css` is the file
    /// `static/style.css`. A relative directory is relative to where the program is started.
    /// Missing files get an empty 404.
    ///
    /// # Panics
    ///
    /// If `prefix` does not start with `/`, is only `/`, or ends with `/`.
    pub fn new(prefix: &str, directory: impl Into<PathBuf>) -> Self {
        check_prefix(prefix);
        let (prefix, directory) = (prefix.to_owned(), directory.into());
        let router = axum::Router::new().nest_service(&prefix, ServeDir::new(&directory));
        Self {
            prefix,
            directory,
            router,
        }
    }

    /// Answer with this service where there is no file, such as the site's page for a 404.
    ///
    /// Its responses get the status 404, whatever it answers.
    pub fn not_found<S>(mut self, page: S) -> Self
    where
        S: Service<Request, Error = Infallible> + Clone + Send + Sync + 'static,
        S::Response: IntoResponse,
        S::Future: Send + 'static,
    {
        let page = axum::Router::new().fallback_service(page);
        let files = ServeDir::new(&self.directory).not_found_service(page.clone());
        self.router = axum::Router::new()
            .nest_service(&self.prefix, files)
            // For a route whose path does not begin with the prefix.
            .fallback_service(page);
        self
    }
}

impl Service<Request> for StaticFiles {
    type Response = Response;
    type Error = Infallible;
    type Future = <axum::Router as Service<Request>>::Future;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Infallible>> {
        <axum::Router as Service<Request>>::poll_ready(&mut self.router, cx)
    }

    fn call(&mut self, request: Request) -> Self::Future {
        self.router.call(request)
    }
}

/// A path prefix, such as `/static`, below which something is served.
fn check_prefix(prefix: &str) {
    assert!(
        prefix.starts_with('/') && prefix.len() > 1 && !prefix.ends_with('/'),
        "a path prefix starts with `/` and does not end with one, as `/static` does: {prefix:?}"
    );
}

#[cfg(feature = "warded")]
pub use spa::SinglePageApp;

#[cfg(feature = "warded")]
mod spa {
    use super::check_prefix;
    use axum::{
        extract::Request,
        http::{
            HeaderValue, StatusCode,
            header::{CACHE_CONTROL, VARY},
        },
        middleware::{Next, from_fn},
        response::{IntoResponse, Response},
        routing::any,
    };
    use std::{convert::Infallible, path::PathBuf};
    use tower::Service;
    use tower_http::{
        services::{ServeDir, ServeFile},
        set_header::SetResponseHeaderLayer,
    };
    use warded::{
        Matcher, match_fn,
        matchers::{get, head},
        negotiation::Offers,
    };

    /// A single-page app: the files that its build tool wrote, and its API, as one application.
    ///
    /// Tools such as Vite, Angular and Trunk build an `index.html`, and scripts, styles and images,
    /// some with a hash of their content in their names. The browser runs the app, which has
    /// routes of its own and asks the API for data. Requests are answered in this order:
    ///
    /// 1. Below the prefix of the [API](Self::api), by the API, whose 404s stay 404s.
    /// 2. Below `/assets`, by the files with hashed names in `assets/`, which may be kept for a
    ///    year. The name is the [`assets`](Self::assets) setting.
    /// 3. Other files at the top of the build, such as `favicon.svg`.
    /// 4. Pages that a browser opens, which prefer HTML, by `index.html`.
    /// 5. A 404 for everything else, also for scripts that ask for a wrong address.
    ///
    /// Only GET and HEAD reach the files. Browsers must ask whether anything outside `assets/` has
    /// changed before they use it again (`Cache-Control: no-cache`), so that they find the new
    /// names after an update. Responses that depend on what the client prefers say so with
    /// `Vary: accept`.
    ///
    /// ```
    /// use warded_server::{
    ///     SinglePageApp,
    ///     axum::{Json, Router, routing::get},
    /// };
    ///
    /// let api = Router::new().route("/api/hello", get(|| async { Json("hello") }));
    /// let app: Router = SinglePageApp::new("web/dist").api("/api", api).into_router();
    /// ```
    #[derive(Clone, Debug)]
    #[must_use = "a single-page app does nothing until it is served"]
    pub struct SinglePageApp {
        directory: PathBuf,
        assets: String,
        api: Option<(String, axum::Router)>,
    }

    impl SinglePageApp {
        /// Serve the build in `directory`, such as `dist`, without an API.
        ///
        /// A relative directory is relative to where the program is started.
        pub fn new(directory: impl Into<PathBuf>) -> Self {
            Self {
                directory: directory.into(),
                assets: "assets".to_owned(),
                api: None,
            }
        }

        /// Let `api` answer every request whose path is `prefix` or below it, such as `/api`.
        ///
        /// The API answers its own 404s, so that a wrong address in the app's code is never
        /// answered with a page. Its paths include the prefix.
        ///
        /// # Panics
        ///
        /// If `prefix` does not start with `/`, is only `/`, or ends with `/`.
        pub fn api<S>(mut self, prefix: &str, api: S) -> Self
        where
            S: Service<Request, Error = Infallible> + Clone + Send + Sync + 'static,
            S::Response: IntoResponse,
            S::Future: Send + 'static,
        {
            check_prefix(prefix);
            let api = axum::Router::new().fallback_service(api);
            self.api = Some((prefix.to_owned(), api));
            self
        }

        /// The directory in the build whose files have hashed names, served below the same path.
        ///
        /// Default: `assets`, as Vite names it. Its files may be kept for a year, except HTML.
        ///
        /// # Panics
        ///
        /// If the name is empty, or not a relative path inside the build.
        pub fn assets(mut self, directory: &str) -> Self {
            let inside = !directory.is_empty()
                && directory
                    .split('/')
                    .all(|part| !matches!(part, "" | "." | ".."));
            assert!(
                inside,
                "the assets are a directory inside the build, such as `assets`: {directory:?}"
            );
            self.assets = directory.to_owned();
            self
        }

        /// The application, to [serve](crate::Server::serve) or to add layers to.
        ///
        /// Logs a warning when the directory has no `index.html` yet.
        pub fn into_router(self) -> axum::Router {
            let index = self.directory.join("index.html");
            if !index.is_file() {
                tracing::warn!(
                    path = %index.display(),
                    "the single-page app has no index.html; build it first"
                );
            }
            let get_or_head = || get().or(head());
            let assets_path = format!("/{}", self.assets);
            let assets = axum::Router::new()
                .nest_service(
                    &assets_path,
                    ServeDir::new(self.directory.join(&self.assets))
                        .append_index_html_on_directories(false),
                )
                .layer(from_fn(cache_assets));

            // Browsers prefer HTML when they open a page. Scripts, and requests for scripts,
            // styles and images, take anything, so they choose the first offer.
            let preferred = Offers::media_types(["application/octet-stream", "text/html"])
                .expect("both are media types");
            let html = preferred
                .selected("text/html")
                .expect("text/html is offered");
            let pages = warded::Router::new()
                .route(
                    html,
                    axum::Router::new().fallback_service(ServeFile::new(&index)),
                )
                .fallback(any(|| async { StatusCode::NOT_FOUND }));
            let files = ServeDir::new(&self.directory)
                .append_index_html_on_directories(false)
                .fallback(crate::app(pages));
            let others = axum::Router::new()
                .fallback_service(files)
                .layer(SetResponseHeaderLayer::if_not_present(
                    CACHE_CONTROL,
                    HeaderValue::from_static("no-cache"),
                ))
                // The answer depends on whether the client prefers HTML.
                .layer(SetResponseHeaderLayer::appending(
                    VARY,
                    HeaderValue::from_static("accept"),
                ));

            let mut routes = warded::Router::new();
            if let Some((prefix, api)) = self.api {
                routes = routes.route(under(prefix), api);
            }
            let routes = routes
                .route(get_or_head().and(under(assets_path)), assets)
                .route(get_or_head(), others)
                .fallback(any(|| async { StatusCode::NOT_FOUND }));
            crate::app(routes)
        }
    }

    impl From<SinglePageApp> for axum::Router {
        fn from(app: SinglePageApp) -> Self {
            app.into_router()
        }
    }

    /// Whether the path is this one or below it.
    fn under(prefix: String) -> impl Matcher + Clone {
        match_fn(move |head| {
            let rest = head.uri().path().strip_prefix(prefix.as_str());
            rest.is_some_and(|rest| rest.is_empty() || rest.starts_with('/'))
        })
    }

    /// Let browsers keep the files with hashed names, which never change.
    ///
    /// Not missing files, which a later build may bring, and not HTML, which keeps its name.
    async fn cache_assets(request: Request, next: Next) -> Response {
        let path = request.uri().path().to_ascii_lowercase();
        let html = path.ends_with(".html") || path.ends_with(".htm");
        let mut response = next.run(request).await;
        let found = response.status().is_success() || response.status() == StatusCode::NOT_MODIFIED;
        if found && !html {
            let forever = HeaderValue::from_static("public, max-age=31536000, immutable");
            response
                .headers_mut()
                .entry(CACHE_CONTROL)
                .or_insert(forever);
        }
        response
    }
}
