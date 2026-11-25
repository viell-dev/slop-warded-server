use axum::{
    body::Body,
    error_handling::HandleError,
    extract::{FromRequestParts, Request},
    http::{StatusCode, request::Parts},
    response::{IntoResponse, Response},
};
use percent_encoding::percent_decode_str;
use serde::de::DeserializeOwned;
use std::convert::Infallible;
use tower::Service;
use warded::{Captures, DispatchError, MatchError};

/// Turn Warded routes into an Axum router that a [`Server`](crate::Server) can serve.
///
/// A routing error becomes a response: 400 when a route declared a malformed header to be an
/// error, and 500 otherwise, with the reason logged. Add layers to the returned router for
/// middleware that should also see unmatched requests.
///
/// ```
/// use warded_server::{axum::{Router, routing::any}, warded::{self, Matcher, matchers}};
///
/// async fn home() -> &'static str {
///     "Hello"
/// }
///
/// let routes = warded::Router::new()
///     .route(matchers::get().and(matchers::exact_path("/")), any(home));
/// let app: Router = warded_server::app(routes);
/// ```
pub fn app<R>(routes: R) -> axum::Router
where
    R: Service<Request, Response = Response<Body>, Error = DispatchError<Infallible>>
        + Clone
        + Send
        + Sync
        + 'static,
    R::Future: Send + 'static,
{
    axum::Router::new().fallback_service(HandleError::new(routes, respond))
}

async fn respond(error: DispatchError<Infallible>) -> StatusCode {
    match error {
        DispatchError::Match(error @ MatchError::InvalidHeader { .. }) => {
            tracing::debug!(%error, "refused a request");
            StatusCode::BAD_REQUEST
        }
        error => {
            tracing::error!(%error, "could not select a route");
            StatusCode::INTERNAL_SERVER_ERROR
        }
    }
}

/// An extractor for the path captures of the selected Warded route.
///
/// The fields of `T` are the capture names. Values are percent-decoded and then parsed, so a
/// number in the path can be a number in `T`. A value that does not parse gets a 400 response.
/// Axum's `Path` extractor does not see Warded's captures. `T` is a struct or a map, also for a
/// single capture. If its fields are not the names in the route's pattern, every request gets a 400
/// response, and the reason is logged at the `debug` level.
///
/// ```
/// use serde::Deserialize;
/// use warded_server::{
///     Params,
///     axum::routing::any,
///     warded::{Router, matchers::path},
/// };
///
/// #[derive(Deserialize)]
/// struct OrderParams {
///     id: u64,
/// }
///
/// async fn show_order(Params(order): Params<OrderParams>) -> String {
///     format!("order {}", order.id)
/// }
///
/// # fn main() -> Result<(), warded_server::warded::DefinitionError> {
/// let routes = Router::new().route(path("/orders/{id}")?, any(show_order));
/// # let _ = warded_server::app(routes);
/// # Ok(())
/// # }
/// ```
#[derive(Clone, Copy, Debug)]
pub struct Params<T>(pub T);

impl<T, S> FromRequestParts<S> for Params<T>
where
    T: DeserializeOwned,
    S: Sync,
{
    type Rejection = ParamsRejection;

    async fn from_request_parts(parts: &mut Parts, _: &S) -> Result<Self, ParamsRejection> {
        let Some(captures) = parts.extensions.get::<Captures>() else {
            tracing::error!("Params needs a handler that a Warded route selected");
            return Err(ParamsRejection::NoRoute);
        };
        // The query-string deserializer parses numbers and other types from text.
        let pairs: Vec<_> = captures
            .iter()
            .map(|(name, value)| {
                percent_decode_str(value)
                    .decode_utf8()
                    .map(|value| (name, value))
                    .map_err(|_| ParamsRejection::Invalid(format!("`{name}` is not UTF-8")))
            })
            .collect::<Result<_, _>>()?;
        let query =
            serde_urlencoded::to_string(&pairs).expect("pairs of strings are a valid query string");
        let query = serde_urlencoded::Deserializer::new(form_urlencoded::parse(query.as_bytes()));
        serde_path_to_error::deserialize(query)
            .map(Self)
            .map_err(|error| {
                // Fields of `T` that are no captures of the route also end up here.
                tracing::debug!(%error, ?pairs, "Params could not read the captures");
                ParamsRejection::Invalid(error.to_string())
            })
    }
}

/// Why [`Params`] refused a request.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum ParamsRejection {
    /// No Warded route selected this handler. This is a mistake in the application.
    #[error("no Warded route selected this handler")]
    NoRoute,
    /// The captured values do not form the expected type.
    #[error("invalid path: {0}")]
    Invalid(String),
}

impl ParamsRejection {
    /// The status of the response to the request.
    #[must_use]
    pub fn status(&self) -> StatusCode {
        match self {
            Self::NoRoute => StatusCode::INTERNAL_SERVER_ERROR,
            Self::Invalid(_) => StatusCode::BAD_REQUEST,
        }
    }
}

impl IntoResponse for ParamsRejection {
    fn into_response(self) -> Response {
        match self {
            Self::NoRoute => self.status().into_response(),
            Self::Invalid(_) => (self.status(), format!("{self}\n")).into_response(),
        }
    }
}
