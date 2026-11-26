//! The middleware that most applications start with, from `tower-http`.

use axum::{
    extract::Request,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use std::{any::Any, convert::Infallible, fmt, sync::Arc};
use tower::{Layer, Service, ServiceBuilder, util::BoxCloneSyncService};
use tower_http::{
    catch_panic::CatchPanicLayer,
    compression::CompressionLayer,
    trace::{DefaultMakeSpan, DefaultOnResponse, TraceLayer},
};
use tracing::Level;

/// What a panic leaves: usually a `String` or a `&'static str` with its message.
type Panic = Box<dyn Any + Send + 'static>;

/// Logging, compression and a 500 for panics: the middleware that most applications start with.
///
/// Add it to the router that `app` returns, or to any `axum::Router`, so that it sees every
/// request. From the outside in, it is these layers from
/// [`tower-http`](https://docs.rs/tower-http):
///
/// - `TraceLayer` logs each request, at the `info` level, with its method, target, version, status
///   and duration, and server errors at the `error` level.
/// - `CompressionLayer` compresses responses with Brotli or gzip for clients that accept them.
/// - `CatchPanicLayer` turns a panic in a handler into a response, an empty 500 unless
///   [`on_panic`](Self::on_panic) says otherwise, and logs it. Without it the request fails without
///   a response.
///
/// Make handlers services with `handler.into_service()` or `handler.with_state(state)`, and not
/// with `any(handler)`, so that the response to HEAD has the head of the compressed response to
/// GET. For another choice or order of layers, use them yourself.
///
/// ```
/// use warded_server::{
///     DefaultMiddleware,
///     axum::{handler::HandlerWithoutStateExt, http::StatusCode},
///     warded::{Router, matchers::exact_path},
/// };
///
/// async fn home() -> &'static str {
///     "Hello"
/// }
///
/// let routes = Router::new().route(exact_path("/"), home.into_service());
/// let app = warded_server::app(routes)
///     .layer(DefaultMiddleware::new().on_panic(|_| StatusCode::SERVICE_UNAVAILABLE));
/// # let _: warded_server::axum::Router = app;
/// ```
#[derive(Clone)]
#[must_use = "middleware does nothing until it is a layer"]
pub struct DefaultMiddleware {
    respond: Arc<dyn Fn(Panic) -> Response + Send + Sync>,
}

impl DefaultMiddleware {
    /// The middleware, with an empty 500 for panics.
    pub fn new() -> Self {
        Self {
            respond: Arc::new(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response()),
        }
    }

    /// Answer a request whose handler panicked with this response, such as an error page or a JSON
    /// error.
    ///
    /// The function gets what the panic left, which is usually its message as a `String` or a
    /// `&'static str`. The panic is logged before.
    pub fn on_panic<F, R>(mut self, respond: F) -> Self
    where
        F: Fn(Box<dyn Any + Send + 'static>) -> R + Send + Sync + 'static,
        R: IntoResponse,
    {
        self.respond = Arc::new(move |panic| respond(panic).into_response());
        self
    }
}

impl Default for DefaultMiddleware {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Debug for DefaultMiddleware {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DefaultMiddleware").finish_non_exhaustive()
    }
}

impl<S> Layer<S> for DefaultMiddleware
where
    S: Service<Request, Response = Response, Error = Infallible> + Clone + Send + Sync + 'static,
    S::Future: Send + 'static,
{
    /// A boxed service, so that the `tower-http` types stay out of this crate's API.
    type Service = BoxCloneSyncService<Request, Response, Infallible>;

    fn layer(&self, inner: S) -> Self::Service {
        let respond = self.respond.clone();
        let trace = TraceLayer::new_for_http()
            .make_span_with(DefaultMakeSpan::new().level(Level::INFO))
            .on_response(DefaultOnResponse::new().level(Level::INFO));
        let service = ServiceBuilder::new()
            .map_response(IntoResponse::into_response)
            .layer(trace)
            .layer(CompressionLayer::new())
            .layer(CatchPanicLayer::custom(move |panic: Panic| {
                let message = panic
                    .downcast_ref::<String>()
                    .map(String::as_str)
                    .or_else(|| panic.downcast_ref::<&str>().copied());
                tracing::error!(panic = message, "a handler panicked");
                respond(panic)
            }))
            .service(inner);
        BoxCloneSyncService::new(service)
    }
}
