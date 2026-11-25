//! Errors as JSON, also the ones from extractors.

use axum::{
    Json,
    extract::{FromRequest, FromRequestParts, rejection::JsonRejection},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde::Serialize;
use std::any::Any;
use warded_server::{Params, ParamsRejection};

/// A response that reports a problem.
pub struct Problem {
    status: StatusCode,
    message: String,
}

impl Problem {
    pub fn new(status: StatusCode, message: impl Into<String>) -> Self {
        Self {
            status,
            message: message.into(),
        }
    }
}

impl IntoResponse for Problem {
    fn into_response(self) -> Response {
        #[derive(Serialize)]
        struct Body {
            error: String,
        }

        let body = Body {
            error: self.message,
        };
        (self.status, Json(body)).into_response()
    }
}

/// What a client gets when a handler panics.
pub fn panicked(_: Box<dyn Any + Send>) -> Response {
    Problem::new(StatusCode::INTERNAL_SERVER_ERROR, "something went wrong").into_response()
}

/// A JSON request body.
#[derive(FromRequest)]
#[from_request(via(Json), rejection(Problem))]
pub struct JsonBody<T>(pub T);

impl From<JsonRejection> for Problem {
    fn from(rejection: JsonRejection) -> Self {
        Self::new(rejection.status(), rejection.body_text())
    }
}

/// The captures of a path.
#[derive(FromRequestParts)]
#[from_request(via(Params), rejection(Problem))]
pub struct PathParams<T>(pub T);

impl From<ParamsRejection> for Problem {
    fn from(rejection: ParamsRejection) -> Self {
        match rejection {
            ParamsRejection::Invalid(_) => Self::new(rejection.status(), rejection.to_string()),
            // A mistake in the routes, which the log explains.
            _ => Self::new(rejection.status(), "something went wrong"),
        }
    }
}
