use axum::{
    Json,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde::Serialize;

/// A response that reports a problem as JSON.
pub struct Problem(pub StatusCode, pub &'static str);

impl IntoResponse for Problem {
    fn into_response(self) -> Response {
        #[derive(Serialize)]
        struct Body {
            error: &'static str,
        }

        (self.0, Json(Body { error: self.1 })).into_response()
    }
}
