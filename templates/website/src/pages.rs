//! The pages, from the Askama templates in `templates/`.
//!
//! Each page is a type with the values that its template shows. `base.html` is the frame around
//! them, which needs a `title`, and the path of the menu entry to mark as `current`.

use askama::Template;
use axum::{
    http::StatusCode,
    response::{Html, IntoResponse, Response},
};
use std::any::Any;

#[derive(Template)]
#[template(path = "home.html")]
struct Home {
    title: &'static str,
    current: &'static str,
}

pub async fn home() -> Response {
    let page = Home {
        title: "Welcome",
        current: "/",
    };
    render(StatusCode::OK, &page)
}

#[derive(Template)]
#[template(path = "about.html")]
struct About {
    title: &'static str,
    current: &'static str,
}

pub async fn about() -> Response {
    let page = About {
        title: "About",
        current: "/about",
    };
    render(StatusCode::OK, &page)
}

#[derive(Template)]
#[template(path = "not-found.html")]
struct NotFound {
    title: &'static str,
    current: &'static str,
}

pub async fn not_found() -> Response {
    let page = NotFound {
        title: "Page not found",
        current: "",
    };
    render(StatusCode::NOT_FOUND, &page)
}

#[derive(Template)]
#[template(path = "error.html")]
struct Error {
    title: &'static str,
    current: &'static str,
}

/// The page for a request whose handler panicked. The panic is in the log.
pub fn error(_: Box<dyn Any + Send>) -> Response {
    let page = Error {
        title: "Something went wrong",
        current: "",
    };
    render(StatusCode::INTERNAL_SERVER_ERROR, &page)
}

/// Answer with a page, or with an empty 500 if it cannot be rendered.
pub fn render(status: StatusCode, page: &impl Template) -> Response {
    match page.render() {
        Ok(html) => (status, Html(html)).into_response(),
        Err(error) => {
            tracing::error!(%error, "could not render a page");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}
