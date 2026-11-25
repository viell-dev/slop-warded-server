//! The API, below `/api`. Add yours here.

use axum::{Json, Router, extract::Path, routing::get};
use serde::Serialize;

#[derive(Serialize)]
struct Greeting {
    text: String,
}

pub fn routes() -> Router {
    Router::new().route("/api/greeting/{name}", get(greeting))
}

async fn greeting(Path(name): Path<String>) -> Json<Greeting> {
    Json(Greeting {
        text: format!("Hello {name}, from Rust"),
    })
}
